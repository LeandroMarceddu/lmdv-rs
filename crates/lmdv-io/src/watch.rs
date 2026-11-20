//! Folder-based file watchers.
//!
//! Documents are watched through their parent folders, not through the files
//! themselves: an atomic save replaces the inode and would kill a file watch.
//! The debounced batch is classified by `stat` after it settles. The tree
//! watcher watches a folder recursively and reports only listing changes;
//! content writes are ignored.
//!
//! A lost batch — the kernel queue overflowed, or the backend errored — is
//! reported as [`FileEvent::Overflow`] instead of being dropped, and every
//! followed document is re-probed in the same batch (OI-178).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, NoCache, RecommendedCache, new_debouncer, new_debouncer_opt,
};

/// Document debounce: 300 ms.
pub const DOC_DEBOUNCE: Duration = Duration::from_millis(300);
/// Folder-tree debounce: 400 ms.
pub const TREE_DEBOUNCE: Duration = Duration::from_millis(400);

/// What became of a document after a batch of filesystem events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileEvent {
    /// The path still exists (including an atomic save over it).
    Changed { path: PathBuf },
    /// The path was renamed to a file that exists now.
    Renamed { from: PathBuf, to: PathBuf },
    /// The path was deleted or moved somewhere that is not a file.
    Removed { path: PathBuf },
    /// Events were lost: the kernel queue overflowed or the backend errored.
    ///
    /// Not tied to a document. It precedes the re-probe of every followed
    /// document in the same batch, so an owner that reloads on `Changed` and
    /// `Removed` needs no further recovery; it should surface one notice and
    /// reinstall the watch with [`DocWatcher::rearm`] (OI-178).
    Overflow,
}

/// A change that may affect a folder listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeEvent {
    /// A create, remove or rename under the watched root; re-list it.
    Changed,
}

/// Why a watch could not be installed.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("cannot create the filesystem watcher: {0}")]
    Create(#[source] notify::Error),
    #[error("cannot watch {path}: {source}")]
    Watch {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
}

/// A watch on the parent folders of a set of documents.
///
/// The callback runs on the debouncer thread and must be quick; the app should
/// forward the event to its own channel. It is called once per touched
/// document per batch, preceded by [`FileEvent::Overflow`] when the batch lost
/// events.
pub struct DocWatcher {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    dirs: Vec<PathBuf>,
    files: Arc<Mutex<Vec<PathBuf>>>,
    /// Set on the debouncer thread when a batch lost events; the next
    /// `set_files` or `rearm` reinstalls every folder watch (OI-178).
    stale: Arc<AtomicBool>,
    reinstalls: u64,
}

impl DocWatcher {
    /// Creates the watcher with no files yet.
    pub fn new<F>(on_event: F) -> Result<Self, WatchError>
    where
        F: FnMut(FileEvent) + Send + 'static,
    {
        let files: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
        let watched = Arc::clone(&files);
        let stale: Arc<AtomicBool> = Arc::default();
        let lost = Arc::clone(&stale);
        let mut on_event = on_event;
        let debouncer = new_debouncer(DOC_DEBOUNCE, None, move |result: DebounceEventResult| {
            let files = lock(&watched).clone();
            deliver(&files, &result, &lost, &mut on_event);
        })
        .map_err(WatchError::Create)?;
        Ok(Self { debouncer, dirs: Vec::new(), files, stale, reinstalls: 0 })
    }

    /// Follows exactly these documents now. Each distinct parent folder is
    /// watched once and non-recursively. A folder that cannot be watched is
    /// returned as an error, while the remaining folders stay watched.
    ///
    /// A list that arrives after an overflow reinstalls every folder: the
    /// overflow can drop the watch itself as well as its events (OI-178).
    pub fn set_files(&mut self, files: &[PathBuf]) -> Result<(), WatchError> {
        if self.stale.swap(false, Ordering::SeqCst) {
            self.drop_watches();
        }

        let mut wanted: Vec<PathBuf> = Vec::new();
        for dir in files.iter().filter_map(|file| file.parent()) {
            if !wanted.iter().any(|existing| existing == dir) {
                wanted.push(dir.to_path_buf());
            }
        }

        for gone in self.dirs.iter().filter(|dir| !wanted.iter().any(|wanted| wanted == *dir)) {
            let _ = self.debouncer.unwatch(gone);
        }

        let mut failed = None;
        let mut active = Vec::new();
        for dir in wanted {
            if self.dirs.iter().any(|existing| existing == &dir) {
                active.push(dir);
                continue;
            }
            match self.debouncer.watch(&dir, RecursiveMode::NonRecursive) {
                Ok(()) => active.push(dir),
                Err(source) => {
                    if failed.is_none() {
                        failed = Some(WatchError::Watch { path: dir, source });
                    }
                }
            }
        }
        self.dirs = active;
        *lock(&self.files) = files.to_vec();
        match failed {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Reinstalls every folder watch, dropping and re-adding each descriptor.
    ///
    /// Call this after [`FileEvent::Overflow`]: the lost events cannot be
    /// replayed and some platforms drop the watch along with them. The
    /// documents themselves were already re-probed by the overflow batch, so
    /// this only makes sure later writes are seen again. A folder that cannot
    /// be reinstalled is dropped from [`Self::dirs`] and reported, while the
    /// rest stay watched; a later `set_files` adds it back.
    pub fn rearm(&mut self) -> Result<(), WatchError> {
        self.stale.store(false, Ordering::SeqCst);
        self.reinstalls = self.reinstalls.saturating_add(1);
        let dirs = std::mem::take(&mut self.dirs);
        let mut failed = None;
        for dir in dirs {
            let _ = self.debouncer.unwatch(&dir);
            match self.debouncer.watch(&dir, RecursiveMode::NonRecursive) {
                Ok(()) => self.dirs.push(dir),
                Err(source) => {
                    if failed.is_none() {
                        failed = Some(WatchError::Watch { path: dir, source });
                    }
                }
            }
        }
        match failed {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// The folders watched now.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// How many times every folder watch was dropped and installed again, for
    /// diagnostics.
    pub fn reinstalls(&self) -> u64 {
        self.reinstalls
    }

    /// Drops every folder watch; the next install adds the wanted ones again.
    fn drop_watches(&mut self) {
        self.reinstalls = self.reinstalls.saturating_add(1);
        for dir in std::mem::take(&mut self.dirs) {
            let _ = self.debouncer.unwatch(&dir);
        }
    }
}

/// A recursive watch on one folder tree.
pub struct TreeWatcher {
    debouncer: Debouncer<RecommendedWatcher, NoCache>,
    root: PathBuf,
}

impl TreeWatcher {
    /// Watches `root` recursively. Only creates, removes and renames under
    /// the root emit [`TreeEvent::Changed`]; content writes do not.
    pub fn new<F>(root: &Path, mut on_change: F) -> Result<Self, WatchError>
    where
        F: FnMut(TreeEvent) + Send + 'static,
    {
        let base = root.to_path_buf();
        let watched = base.clone();
        let mut debouncer = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
            TREE_DEBOUNCE,
            None,
            move |result: DebounceEventResult| {
                let Ok(events) = result else {
                    return;
                };
                let relevant = events.iter().any(|event| {
                    matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
                    ) && event.paths.iter().any(|path| path.starts_with(&watched))
                });
                if relevant {
                    on_change(TreeEvent::Changed);
                }
            },
            NoCache,
            notify::Config::default(),
        )
        .map_err(WatchError::Create)?;
        debouncer
            .watch(&base, RecursiveMode::Recursive)
            .map_err(|source| WatchError::Watch { path: base.clone(), source })?;
        Ok(Self { debouncer, root: base })
    }

    /// The watched root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reinstalls the recursive watch. Windows can silently drop a watch;
    /// call this when the window regains focus. A failed unwatch of a watch
    /// that is already gone is ignored.
    pub fn rearm(&mut self) -> Result<(), WatchError> {
        let _ = self.debouncer.unwatch(&self.root);
        self.debouncer
            .watch(&self.root, RecursiveMode::Recursive)
            .map_err(|source| WatchError::Watch { path: self.root.clone(), source })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One of the events names the document.
fn touched(file: &Path, events: &[DebouncedEvent]) -> bool {
    events
        .iter()
        .filter(|event| !is_access(&event.kind))
        .any(|event| event.paths.iter().any(|path| same_path(path, file)))
}

/// Whether `kind` only reports an open, read or close.
///
/// A load reads the file, and Linux reports that as `Access(Open(Any))` on the
/// watched folder — with no read/write mode. Counting an access as a change
/// reloads the document after every load; the reload reads it again and the
/// loop never ends, the reader restoring its old anchor while the outline and
/// hairline flicker with each restore. Content changes arrive as
/// `Create`/`Modify`/`Remove`, so every access event is safe to ignore (a
/// write's own access events accompany its `Modify`).
fn is_access(kind: &EventKind) -> bool {
    matches!(kind, EventKind::Access(_))
}

/// One settled batch, decided for the followed documents.
struct Batch {
    /// Events were lost: report it and re-probe every document (OI-178).
    overflow: bool,
    /// What the owner hears, in order.
    events: Vec<FileEvent>,
}

/// Decides one debounced batch.
///
/// notify reports a lost inotify queue as a batch carrying the rescan flag and
/// no path, and reports a backend read failure as an error batch; both mean the
/// events that would have named the changed documents are gone. Each becomes
/// one [`FileEvent::Overflow`] followed by a re-probe of every followed
/// document, so nothing waits for a later event that may never come. Ordinary
/// batches keep the access filter (OI-168); a re-probe is driven by `stat`, not
/// by the dropped events.
fn batch(files: &[PathBuf], result: &DebounceEventResult) -> Batch {
    let (events, overflow) = match result {
        Ok(events) => (events.as_slice(), events.iter().any(|event| event.need_rescan())),
        Err(_) => (&[][..], true),
    };
    let mut decided = Batch { overflow, events: Vec::new() };
    if overflow {
        decided.events.push(FileEvent::Overflow);
    }
    let mut reported = HashSet::new();
    for file in files {
        if !reported.insert(file.clone()) || !(overflow || touched(file, events)) {
            continue;
        }
        decided.events.push(classify(file, events));
    }
    decided
}

/// Forwards one decided batch to the owner and marks the watch stale when it
/// lost events, so the next file list or rearm reinstalls the folders.
fn deliver(files: &[PathBuf], result: &DebounceEventResult, stale: &AtomicBool, on_event: &mut impl FnMut(FileEvent)) {
    let decided = batch(files, result);
    if decided.overflow {
        stale.store(true, Ordering::SeqCst);
    }
    for event in decided.events {
        on_event(event);
    }
}

/// A path still there was changed, whatever the events say: an atomic save
/// renames a temp file over it, and some editors rename it away and create it
/// again. A gone path follows a rename chain in the batch; if the final target
/// is a file, it was renamed, otherwise it was removed.
fn classify(watched: &Path, events: &[DebouncedEvent]) -> FileEvent {
    if watched.exists() {
        return FileEvent::Changed { path: watched.to_path_buf() };
    }

    let mut at = watched.to_path_buf();
    let mut to = None;
    for event in events {
        if let EventKind::Modify(ModifyKind::Name(RenameMode::Both)) = event.kind
            && event.paths.len() == 2
            && same_path(&event.paths[0], &at)
        {
            at = event.paths[1].clone();
            to = Some(at.clone());
        }
    }

    match to.filter(|path| path.is_file()) {
        Some(to) => FileEvent::Renamed { from: watched.to_path_buf(), to },
        None => FileEvent::Removed { path: watched.to_path_buf() },
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};
    use std::time::Instant;

    use notify::Event;
    use notify::event::{AccessKind, AccessMode, Flag};
    use tempfile::TempDir;

    use super::*;

    fn write_event(path: &Path) -> DebouncedEvent {
        DebouncedEvent::new(
            Event::new(EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)))
                .add_path(path.to_path_buf()),
            Instant::now(),
        )
    }

    fn rename(from: &Path, to: &Path) -> DebouncedEvent {
        let kind = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
        DebouncedEvent::new(Event::new(kind).add_path(from.to_path_buf()).add_path(to.to_path_buf()), Instant::now())
    }

    /// notify's inotify backend turns a queue overflow (`IN_Q_OVERFLOW`) into
    /// this: an `Other` event with the rescan flag and no path.
    fn rescan() -> DebouncedEvent {
        DebouncedEvent::new(Event::new(EventKind::Other).set_flag(Flag::Rescan), Instant::now())
    }

    fn read_event(path: &Path) -> DebouncedEvent {
        DebouncedEvent::new(
            Event::new(EventKind::Access(AccessKind::Read)).add_path(path.to_path_buf()),
            Instant::now(),
        )
    }

    fn recv_matching(
        rx: &Receiver<FileEvent>,
        predicate: impl Fn(&FileEvent) -> bool,
        timeout: Duration,
    ) -> Option<FileEvent> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(event) if predicate(&event) => return Some(event),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }

    #[test]
    fn classify_tells_changed_renamed_removed() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        let doc = dir.join("doc.md");
        fs::write(&doc, "x").unwrap();

        assert_eq!(
            classify(&doc, &[rename(&dir.join("doc.md.tmp"), &doc)]),
            FileEvent::Changed { path: doc.clone() },
            "still there is changed"
        );

        let final_path = dir.join("final.md");
        fs::rename(&doc, &final_path).unwrap();
        assert_eq!(
            classify(&doc, &[rename(&doc, &dir.join("mid.md")), rename(&dir.join("mid.md"), &final_path)]),
            FileEvent::Renamed { from: doc.clone(), to: final_path.clone() },
            "rename chains are followed"
        );

        assert_eq!(classify(&doc, &[rename(&doc, &dir.join("nowhere.md"))]), FileEvent::Removed { path: doc.clone() });
        assert_eq!(classify(&doc, &[]), FileEvent::Removed { path: doc.clone() });

        let other = dir.join("sub");
        fs::create_dir_all(&other).unwrap();
        assert_eq!(
            classify(&doc, &[rename(&other.join("doc.md"), &other.join("final.md"))]),
            FileEvent::Removed { path: doc.clone() },
            "the same name in another folder is another document"
        );
        assert!(touched(&doc, &[rename(&doc, &dir.join("x.md"))]), "the exact path is touched");
        assert!(!touched(&doc, &[rename(&other.join("doc.md"), &other.join("x.md"))]));
        assert!(!touched(&doc, &[rename(&dir.join("DOC.md"), &dir.join("x.md"))]), "Linux is case-sensitive");
    }

    #[test]
    fn reading_a_document_is_not_a_change() {
        let temp = TempDir::new().unwrap();
        let doc = temp.path().join("doc.md");
        fs::write(&doc, "one").unwrap();

        let read = |path: &Path| {
            DebouncedEvent::new(
                Event::new(EventKind::Access(AccessKind::Read)).add_path(path.to_path_buf()),
                Instant::now(),
            )
        };
        let open = |path: &Path| {
            DebouncedEvent::new(
                Event::new(EventKind::Access(AccessKind::Open(AccessMode::Read))).add_path(path.to_path_buf()),
                Instant::now(),
            )
        };
        let close = |path: &Path| {
            DebouncedEvent::new(
                Event::new(EventKind::Access(AccessKind::Close(AccessMode::Read))).add_path(path.to_path_buf()),
                Instant::now(),
            )
        };
        assert!(!touched(&doc, &[read(&doc)]), "a read is not a change");
        assert!(!touched(&doc, &[open(&doc), close(&doc)]), "opening and closing for a read are not changes");
        assert!(touched(&doc, &[write_event(&doc)]), "a content write still is");
    }

    #[test]
    fn the_live_watch_ignores_its_own_reads() {
        let temp = TempDir::new().unwrap();
        let doc = temp.path().join("doc.md");
        fs::write(&doc, "one").unwrap();
        let (tx, rx) = mpsc::channel();
        let mut watcher = DocWatcher::new(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        watcher.set_files(std::slice::from_ref(&doc)).unwrap();

        // Installing and settling the watch may produce events of its own.
        std::thread::sleep(Duration::from_millis(500));
        while rx.try_recv().is_ok() {}

        // A load reads the document; that must not ask for another load.
        assert_eq!(fs::read(&doc).unwrap(), b"one");
        if let Some(event) = recv_matching(&rx, |_| true, Duration::from_millis(900)) {
            panic!("reading a watched document reported {event:?}");
        }

        fs::write(&doc, "two").unwrap();
        let event = recv_matching(
            &rx,
            |event| matches!(event, FileEvent::Changed { path } if path == &doc),
            Duration::from_secs(5),
        )
        .expect("a real write is still reported");
        assert_eq!(event, FileEvent::Changed { path: doc });
    }

    #[test]
    fn one_watch_follows_the_folders_of_its_documents() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let (a, b) = (root.join("a"), root.join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut watcher = DocWatcher::new(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();

        let docs = [a.join("one.md"), a.join("two.md"), b.join("three.md")];
        watcher.set_files(&docs).unwrap();
        assert_eq!(watcher.dirs().len(), 2, "two folders for three documents");

        fs::write(&docs[2], "x").unwrap();
        let event = recv_matching(
            &rx,
            |event| matches!(event, FileEvent::Changed { path } if path == &docs[2]),
            Duration::from_secs(5),
        )
        .expect("a change in the second folder");
        assert_eq!(event, FileEvent::Changed { path: docs[2].clone() });

        watcher.set_files(&docs[..2]).unwrap();
        assert_eq!(watcher.dirs().len(), 1);
        fs::write(&docs[2], "y").unwrap();
        fs::write(&docs[1], "y").unwrap();
        recv_matching(
            &rx,
            |event| matches!(event, FileEvent::Changed { path } if path == &docs[1]),
            Duration::from_secs(5),
        )
        .expect("a change in the first folder");
        assert!(
            recv_matching(
                &rx,
                |event| matches!(event, FileEvent::Changed { path } if path == &docs[2]),
                Duration::from_millis(500)
            )
            .is_none(),
            "a folder that was let go reports nothing"
        );
    }

    #[test]
    fn renames_and_removals_are_reported() {
        let temp = TempDir::new().unwrap();
        let doc = temp.path().join("doc.md");
        fs::write(&doc, "x").unwrap();
        let (tx, rx) = mpsc::channel();
        let mut watcher = DocWatcher::new(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        watcher.set_files(std::slice::from_ref(&doc)).unwrap();

        let renamed = temp.path().join("renamed.md");
        fs::rename(&doc, &renamed).unwrap();
        let event = recv_matching(&rx, |event| matches!(event, FileEvent::Renamed { .. }), Duration::from_secs(5))
            .expect("a rename");
        assert_eq!(event, FileEvent::Renamed { from: doc.clone(), to: renamed.clone() });

        watcher.set_files(std::slice::from_ref(&renamed)).unwrap();
        fs::remove_file(&renamed).unwrap();
        let event = recv_matching(&rx, |event| matches!(event, FileEvent::Removed { .. }), Duration::from_secs(5))
            .expect("a removal");
        assert_eq!(event, FileEvent::Removed { path: renamed });
    }

    #[test]
    fn an_overflow_is_reported_once_and_reprobes_every_document() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        let kept = dir.join("kept.md");
        let gone = dir.join("gone.md");
        fs::write(&kept, "x").unwrap();

        let decided = batch(&[kept.clone(), gone.clone(), kept.clone()], &Ok(vec![rescan()]));
        assert!(decided.overflow, "the rescan flag is an overflow");
        assert_eq!(
            decided.events,
            vec![
                FileEvent::Overflow,
                FileEvent::Changed { path: kept.clone() },
                FileEvent::Removed { path: gone.clone() },
            ],
            "one overflow, then every document re-probed once"
        );

        let errors = vec![notify::Error::new(notify::ErrorKind::Generic("queue overflow".to_owned()))];
        let decided = batch(std::slice::from_ref(&kept), &Err(errors));
        assert!(decided.overflow, "a backend error lost events too, and is no longer dropped silently");
        assert_eq!(decided.events, vec![FileEvent::Overflow, FileEvent::Changed { path: kept }]);
    }

    #[test]
    fn ordinary_batches_are_unchanged_after_an_overflow() {
        let temp = TempDir::new().unwrap();
        let doc = temp.path().join("doc.md");
        fs::write(&doc, "x").unwrap();

        let overflow = batch(std::slice::from_ref(&doc), &Ok(vec![rescan()]));
        assert_eq!(overflow.events.first(), Some(&FileEvent::Overflow));

        let next = batch(std::slice::from_ref(&doc), &Ok(vec![write_event(&doc)]));
        assert!(!next.overflow, "the overflow is not sticky");
        assert_eq!(next.events, vec![FileEvent::Changed { path: doc.clone() }], "a normal write is still reported");

        let quiet = batch(std::slice::from_ref(&doc), &Ok(vec![read_event(&doc)]));
        assert!(quiet.events.is_empty(), "OI-168: a read is still not a change");
    }

    #[test]
    fn an_overflow_reaches_the_owner_and_marks_the_watch_stale() {
        let temp = TempDir::new().unwrap();
        let doc = temp.path().join("doc.md");
        fs::write(&doc, "x").unwrap();
        let stale = AtomicBool::new(false);

        let mut seen = Vec::new();
        deliver(std::slice::from_ref(&doc), &Ok(vec![rescan()]), &stale, &mut |event| seen.push(event));
        assert!(stale.load(Ordering::SeqCst), "the next file list reinstalls the folders");
        assert_eq!(
            seen,
            vec![FileEvent::Overflow, FileEvent::Changed { path: doc.clone() }],
            "the overflow is reported, not dropped, and the document re-probed"
        );

        // The same seam keeps working normally once the overflow is over.
        stale.store(false, Ordering::SeqCst);
        let mut seen = Vec::new();
        deliver(std::slice::from_ref(&doc), &Ok(vec![write_event(&doc)]), &stale, &mut |event| seen.push(event));
        assert!(!stale.load(Ordering::SeqCst), "a normal batch does not ask for a reinstall");
        assert_eq!(seen, vec![FileEvent::Changed { path: doc }]);
    }

    #[test]
    fn a_file_list_after_an_overflow_reinstalls_and_later_writes_flow() {
        let temp = TempDir::new().unwrap();
        let doc = temp.path().join("doc.md");
        fs::write(&doc, "one").unwrap();
        let (tx, rx) = mpsc::channel();
        let mut watcher = DocWatcher::new(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        watcher.set_files(std::slice::from_ref(&doc)).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        while rx.try_recv().is_ok() {}

        // The debouncer thread decides an overflow and marks the watch stale.
        deliver(std::slice::from_ref(&doc), &Ok(vec![rescan()]), &watcher.stale, &mut |_| {});
        assert!(watcher.stale.load(Ordering::SeqCst));
        watcher.set_files(std::slice::from_ref(&doc)).unwrap();
        assert_eq!(watcher.reinstalls(), 1, "the list after an overflow drops the watches first");
        assert!(!watcher.stale.load(Ordering::SeqCst));
        assert_eq!(watcher.dirs().len(), 1);

        std::thread::sleep(Duration::from_millis(500));
        while rx.try_recv().is_ok() {}
        fs::write(&doc, "two").unwrap();
        let event = recv_matching(
            &rx,
            |event| matches!(event, FileEvent::Changed { path } if path == &doc),
            Duration::from_secs(5),
        )
        .expect("a write after the reinstall is still reported");
        assert_eq!(event, FileEvent::Changed { path: doc });
    }

    #[test]
    fn a_rearm_reinstalls_the_watch_and_later_writes_flow() {
        let temp = TempDir::new().unwrap();
        let doc = temp.path().join("doc.md");
        fs::write(&doc, "one").unwrap();
        let (tx, rx) = mpsc::channel();
        let mut watcher = DocWatcher::new(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        watcher.set_files(std::slice::from_ref(&doc)).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        while rx.try_recv().is_ok() {}

        watcher.rearm().unwrap();
        assert_eq!(watcher.reinstalls(), 1);
        assert_eq!(watcher.dirs().len(), 1, "the folder stays watched");

        std::thread::sleep(Duration::from_millis(500));
        while rx.try_recv().is_ok() {}
        fs::write(&doc, "two").unwrap();
        let event = recv_matching(
            &rx,
            |event| matches!(event, FileEvent::Changed { path } if path == &doc),
            Duration::from_secs(5),
        )
        .expect("a write after the rearm is still reported");
        assert_eq!(event, FileEvent::Changed { path: doc });
    }

    #[test]
    fn the_tree_watch_ignores_content_writes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let existing = root.join("note.md");
        fs::write(&existing, "one").unwrap();
        let (tx, rx) = mpsc::channel();
        let _watcher = TreeWatcher::new(root, move |_| {
            let _ = tx.send(());
        })
        .unwrap();

        // Settle the initial watch before asserting that a write is quiet.
        std::thread::sleep(Duration::from_millis(500));
        while rx.try_recv().is_ok() {}
        fs::write(&existing, "two").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(700)).is_err(), "content writes do not change the listing");

        fs::write(root.join("new.md"), "x").unwrap();
        rx.recv_timeout(Duration::from_secs(5)).expect("a created file changes the listing");
    }
}
