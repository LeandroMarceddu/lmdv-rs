//! Folder-based file watchers.
//!
//! Documents are watched through their parent folders, not through the files
//! themselves: an atomic save replaces the inode and would kill a file watch.
//! The debounced batch is classified by `stat` after it settles. The tree
//! watcher watches a folder recursively and reports only listing changes;
//! content writes are ignored.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
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
/// document per batch.
pub struct DocWatcher {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    dirs: Vec<PathBuf>,
    files: Arc<Mutex<Vec<PathBuf>>>,
}

impl DocWatcher {
    /// Creates the watcher with no files yet.
    pub fn new<F>(on_event: F) -> Result<Self, WatchError>
    where
        F: FnMut(FileEvent) + Send + 'static,
    {
        let files: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
        let watched = Arc::clone(&files);
        let mut on_event = on_event;
        let debouncer = new_debouncer(DOC_DEBOUNCE, None, move |result: DebounceEventResult| {
            let Ok(events) = result else {
                return;
            };
            let files = lock(&watched).clone();
            let mut reported = HashSet::new();
            for file in files {
                if !reported.insert(file.clone()) || !touched(&file, &events) {
                    continue;
                }
                on_event(classify(&file, &events));
            }
        })
        .map_err(WatchError::Create)?;
        Ok(Self { debouncer, dirs: Vec::new(), files })
    }

    /// Follows exactly these documents now. Each distinct parent folder is
    /// watched once and non-recursively. A folder that cannot be watched is
    /// returned as an error, while the remaining folders stay watched.
    pub fn set_files(&mut self, files: &[PathBuf]) -> Result<(), WatchError> {
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

    /// The folders watched now.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
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
    use notify::event::{AccessKind, AccessMode};
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
