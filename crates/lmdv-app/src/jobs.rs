//! Generation-tagged background jobs and their per-window mailbox.
//!
//! Every completion carries the document id and the generation it was started
//! under. A window accepts a completion only while that generation is still
//! current, so a slow load can never overwrite a tab that was closed, reloaded
//! or reopened in the meantime.
//!
//! P1 spawns one short-lived Rune task per load; P2 adds the Markdown parse
//! job. Save, probe and store flush are wired here for the following slices;
//! they are not dispatched yet. P3 replaces the per-call spawns with bounded IO
//! and parse workers of 2 each, as the architecture requires for
//! typing-adjacent work; the mailbox contract does not change.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lmdv_core::{DocumentId, Draft, Eol, FileMeta, JobId, WindowId};
use lmdv_io::AppDirs;
use lmdv_io::store::{StoreHandle, load_draft};
use lmdv_io::{ReadError, SaveOutcome, WriteError, read_document, save_document};
use lmdv_md::MdDoc;
use rune::Waker;

/// A finished job handed back to its window.
///
/// `job` is unique process-wide; `generation` is the per-document stale guard
/// from [`GenerationTracker`].
#[derive(Debug)]
pub enum JobMsg {
    /// A document read finished.
    Loaded { job: JobId, doc: DocumentId, generation: u64, result: Result<lmdv_io::LoadedFile, ReadError> },
    /// A save finished (P3).
    Saved { job: JobId, doc: DocumentId, generation: u64, result: Result<SaveOutcome, WriteError> },
    /// A metadata probe finished (P1-03 watcher/reload).
    Probed { job: JobId, doc: DocumentId, generation: u64, result: Result<FileMeta, ReadError> },
    /// A Markdown parse finished. `revision` is the buffer revision the source
    /// came from; the receiver drops it when a newer revision exists.
    Parsed { doc: DocumentId, revision: u64, result: Result<Arc<MdDoc>, String> },
    /// A crash draft read finished (E-17). `None` when no draft exists.
    DraftLoaded { doc: DocumentId, key: String, draft: Option<Draft> },
    /// An HTML/DOCX/PDF export finished (P5-04).
    Exported {
        job: JobId,
        doc: DocumentId,
        format: crate::share::ExportFormat,
        result: Result<crate::share::ExportOutcome, String>,
    },
    /// A whole-document rich copy finished (X-02).
    RichCopied { job: JobId, doc: DocumentId, result: Result<crate::share::RichCopy, String> },
    /// A user-initiated external open (Show in folder, Print…'s viewer).
    Opened { job: JobId, path: PathBuf, result: Result<(), String> },
    /// A store flush finished (close/quit).
    Stored { job: JobId, ok: bool },
    /// A file-browser folder listing finished (F-03–F-06).
    TreeListed { job: JobId, generation: u64, root: PathBuf, result: Result<lmdv_io::TreeListing, lmdv_io::TreeError> },
    /// A folder-wide search finished (F-13/F-14).
    Searched {
        job: JobId,
        generation: u64,
        root: PathBuf,
        query: String,
        result: Result<Vec<lmdv_io::SearchHit>, lmdv_io::TreeError>,
    },
    /// A create, remove or rename under the watched root (F-08).
    TreeChanged { root: PathBuf },
    /// The recursive folder watch could not be installed.
    TreeWatchFailed { root: PathBuf, error: String },
}

/// One window's worker→UI mailbox.
///
/// `post` is callable from any thread; the waker installed by
/// `AppSpec::setup` wakes the event loop so the next `poll` drains it.
pub struct WorkerMailbox {
    id: WindowId,
    jobs: Mutex<VecDeque<JobMsg>>,
    waker: Mutex<Option<Waker>>,
}

impl WorkerMailbox {
    /// An empty mailbox for `id`.
    pub fn new(id: WindowId) -> Self {
        Self { id, jobs: Mutex::new(VecDeque::new()), waker: Mutex::new(None) }
    }

    /// The owning window.
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// Installs the UI waker (`AppSpec::setup`, or a test that wants no wake).
    pub fn attach(&self, waker: Waker) {
        if let Ok(mut slot) = self.waker.lock() {
            *slot = Some(waker);
        }
    }

    /// Queues a completion and wakes the UI.
    pub fn post(&self, msg: JobMsg) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push_back(msg);
        }
        let waker = self.waker.lock().ok().and_then(|slot| slot.clone());
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Takes every queued completion in order.
    pub fn drain(&self) -> Vec<JobMsg> {
        match self.jobs.lock() {
            Ok(mut jobs) => jobs.drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Number of queued completions.
    pub fn len(&self) -> usize {
        self.jobs.lock().map_or(0, |jobs| jobs.len())
    }

    /// Whether no completion is queued.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The per-document generation counter behind job stale guards.
#[derive(Clone, Debug, Default)]
pub struct GenerationTracker {
    next: u64,
    current: HashMap<DocumentId, u64>,
}

impl GenerationTracker {
    /// Starts a new generation for `doc`, invalidating every older job.
    pub fn begin(&mut self, doc: DocumentId) -> u64 {
        self.next = self.next.wrapping_add(1);
        let generation = self.next;
        self.current.insert(doc, generation);
        generation
    }

    /// Whether `generation` is still the current one for `doc`.
    pub fn is_current(&self, doc: DocumentId, generation: u64) -> bool {
        self.current.get(&doc) == Some(&generation)
    }

    /// Accepts a completion: true only for the current generation, which is
    /// then consumed. A stale completion changes nothing.
    pub fn finish(&mut self, doc: DocumentId, generation: u64) -> bool {
        if self.is_current(doc, generation) {
            self.current.remove(&doc);
            true
        } else {
            false
        }
    }

    /// Forgets a document entirely, so every in-flight completion is stale.
    pub fn forget(&mut self, doc: DocumentId) {
        self.current.remove(&doc);
    }

    /// Forgets every document.
    pub fn clear(&mut self) {
        self.current.clear();
    }

    /// Number of documents with an in-flight generation.
    pub fn len(&self) -> usize {
        self.current.len()
    }

    /// Whether no job is in flight.
    pub fn is_empty(&self) -> bool {
        self.current.is_empty()
    }
}

/// Starts background jobs and posts their completions.
///
/// Cloning shares the job counter. The hub itself is `Send + Sync`; it holds no
/// `Rc` values.
#[derive(Clone, Debug, Default)]
pub struct JobHub {
    next: Arc<AtomicU64>,
}

impl JobHub {
    /// A hub with job ids starting at 1.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserves the next process-wide job id.
    pub fn allocate(&self) -> JobId {
        self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1)
    }

    /// Reads `path` and posts [`JobMsg::Loaded`] to `mailbox`.
    pub fn load(&self, mailbox: Arc<WorkerMailbox>, doc: DocumentId, path: PathBuf, generation: u64) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let worker_path = path.clone();
        let spawned = rune::task::spawn("lmdv-load", move || {
            let result = read_document(&worker_path);
            worker_mailbox.post(JobMsg::Loaded { job, doc, generation, result });
        });
        if let Err(error) = spawned {
            mailbox.post(JobMsg::Loaded { job, doc, generation, result: Err(ReadError::Io { path, source: error }) });
        }
        job
    }

    /// Saves `text` to `path` and posts [`JobMsg::Saved`] (P3 wires the UI).
    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        mailbox: Arc<WorkerMailbox>,
        doc: DocumentId,
        generation: u64,
        path: PathBuf,
        text: String,
        eol: Eol,
        expected: Option<FileMeta>,
        create: bool,
    ) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let worker_path = path.clone();
        let spawned = rune::task::spawn("lmdv-save", move || {
            let result = save_document(&worker_path, &text, eol, expected, create);
            worker_mailbox.post(JobMsg::Saved { job, doc, generation, result });
        });
        if let Err(error) = spawned {
            mailbox.post(JobMsg::Saved { job, doc, generation, result: Err(WriteError::Io { path, source: error }) });
        }
        job
    }

    /// Reads `path` metadata and posts [`JobMsg::Probed`].
    pub fn probe(&self, mailbox: Arc<WorkerMailbox>, doc: DocumentId, path: PathBuf, generation: u64) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let worker_path = path.clone();
        let spawned = rune::task::spawn("lmdv-probe", move || {
            let result = lmdv_io::probe(&worker_path);
            worker_mailbox.post(JobMsg::Probed { job, doc, generation, result });
        });
        if let Err(error) = spawned {
            mailbox.post(JobMsg::Probed { job, doc, generation, result: Err(ReadError::Io { path, source: error }) });
        }
        job
    }

    /// Parses `source` and posts [`JobMsg::Parsed`].
    ///
    /// Parsing is pure (`lmdv-md` has no Rune dependency) and `MdDoc` is
    /// `Send`, so the whole parse runs on the worker. A panic in the parser is
    /// caught here and reported as a render failure, so a malformed input never
    /// takes the process down (M-34).
    pub fn parse(&self, mailbox: Arc<WorkerMailbox>, doc: DocumentId, revision: u64, source: Arc<str>) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let spawned = rune::task::spawn("lmdv-parse", move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lmdv_md::parse_with_revision(&source, revision)
            }))
            .map(Arc::new)
            .map_err(|panic| {
                let why = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("no message");
                format!("the Markdown parser failed: {why}")
            });
            worker_mailbox.post(JobMsg::Parsed { doc, revision, result });
        });
        if let Err(error) = spawned {
            mailbox.post(JobMsg::Parsed {
                doc,
                revision,
                result: Err(format!("the parse worker could not start: {error}")),
            });
        }
        job
    }

    /// Reads one crash draft and posts [`JobMsg::DraftLoaded`].
    pub fn load_draft(&self, mailbox: Arc<WorkerMailbox>, doc: DocumentId, key: String, dirs: AppDirs) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let worker_key = key.clone();
        let spawned = rune::task::spawn("lmdv-draft", move || {
            let draft = load_draft(&dirs, &worker_key);
            worker_mailbox.post(JobMsg::DraftLoaded { doc, key: worker_key, draft });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::DraftLoaded { doc, key, draft: None });
        }
        job
    }

    /// Flushes the store on a worker and posts [`JobMsg::Stored`].
    pub fn flush(&self, mailbox: Arc<WorkerMailbox>, store: StoreHandle, timeout: Duration) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let spawned = rune::task::spawn("lmdv-store-flush", move || {
            let ok = store.flush(timeout);
            worker_mailbox.post(JobMsg::Stored { job, ok });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::Stored { job, ok: false });
        }
        job
    }

    /// Runs one whole-document rich copy and posts [`JobMsg::RichCopied`].
    ///
    /// The worker resolves pictures, formulas and diagrams and serializes the
    /// inline-styled fragment; only the UI handler writes the clipboard.
    pub fn copy_rich(&self, mailbox: Arc<WorkerMailbox>, spec: crate::share::RichCopySpec) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let doc = spec.doc;
        let spawned = rune::task::spawn("lmdv-rich-copy", move || {
            let result = crate::export_io::run_rich_copy(&spec);
            worker_mailbox.post(JobMsg::RichCopied { job, doc, result });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::RichCopied { job, doc, result: Err("the copy worker could not start".to_owned()) });
        }
        job
    }

    /// Runs one HTML/DOCX/PDF export and posts [`JobMsg::Exported`].
    ///
    /// The model, options and destination are owned by the worker; the UI
    /// keeps only the job id. Building and writing happen on the worker, so a
    /// large document or a slow disk never blocks a frame (ADR-017).
    pub fn export(&self, mailbox: Arc<WorkerMailbox>, spec: crate::share::ExportSpec) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let doc = spec.doc;
        let format = spec.format;
        let spawned = rune::task::spawn("lmdv-export", move || {
            let result = crate::export_io::run_export(&spec);
            worker_mailbox.post(JobMsg::Exported { job, doc, format, result });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::Exported {
                job,
                doc,
                format,
                result: Err("the export worker could not start".to_owned()),
            });
        }
        job
    }

    /// Opens a path with the desktop handler on a worker and posts
    /// [`JobMsg::Opened`].
    pub fn open_external(&self, mailbox: Arc<WorkerMailbox>, path: PathBuf) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let worker_path = path.clone();
        let spawned = rune::task::spawn("lmdv-open", move || {
            let result = crate::export_io::open_external_path(&worker_path);
            worker_mailbox.post(JobMsg::Opened { job, path: worker_path, result });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::Opened { job, path, result: Err("the open worker could not start".to_owned()) });
        }
        job
    }

    /// Lists one folder for the file browser and posts [`JobMsg::TreeListed`].
    ///
    /// The walk happens on a worker (F-03): a slow FUSE folder must never block
    /// a frame. `generation` is the window's listing token; a completion for an
    /// older root is dropped when it lands.
    pub fn list_tree(&self, mailbox: Arc<WorkerMailbox>, root: PathBuf, generation: u64) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let worker_root = root.clone();
        let spawned = rune::task::spawn("lmdv-tree", move || {
            let result = lmdv_io::tree::list_tree(&worker_root);
            worker_mailbox.post(JobMsg::TreeListed { job, generation, root: worker_root, result });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::TreeListed {
                job,
                generation,
                root,
                result: Err(lmdv_io::TreeError::Io {
                    path: PathBuf::new(),
                    source: std::io::Error::other("the listing worker could not start"),
                }),
            });
        }
        job
    }

    /// Searches one folder for the file browser and posts [`JobMsg::Searched`].
    ///
    /// The search reads every Markdown file under the root, so it is a worker
    /// job like the listing. `generation` invalidates a superseded query.
    pub fn search_tree(&self, mailbox: Arc<WorkerMailbox>, root: PathBuf, query: String, generation: u64) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let worker_root = root.clone();
        let worker_query = query.clone();
        let spawned = rune::task::spawn("lmdv-search", move || {
            let result = lmdv_io::tree::search_tree(&worker_root, &worker_query);
            worker_mailbox.post(JobMsg::Searched { job, generation, root: worker_root, query: worker_query, result });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::Searched {
                job,
                generation,
                root,
                query,
                result: Err(lmdv_io::TreeError::Io {
                    path: PathBuf::new(),
                    source: std::io::Error::other("the search worker could not start"),
                }),
            });
        }
        job
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tempfile::{NamedTempFile, TempDir};

    use super::*;
    use crate::test_support;
    use crate::window::{CloseOutcome, WindowState};

    /// Polls until the active document satisfies `done`.
    ///
    /// Opening a file also queues a folder-listing job for the browser, so a
    /// test that waited for "any mailbox message" could consume `TreeListed`
    /// and inspect a still-loading document. Wait for the load outcome itself.
    fn wait_for_load(state: &mut WindowState, done: impl Fn(&WindowState) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(state) {
            assert!(Instant::now() < deadline, "the load job never reported");
            if state.worker().is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            } else {
                let mut tx = rune::Tx::default();
                state.poll(0.0, &mut tx);
            }
        }
    }

    #[test]
    fn generations_are_latest_wins() {
        let mut tracker = GenerationTracker::default();
        let doc = DocumentId(1);
        let first = tracker.begin(doc);
        let second = tracker.begin(doc);

        assert!(!tracker.is_current(doc, first));
        assert!(tracker.is_current(doc, second));
        assert!(!tracker.finish(doc, first), "the older job is dropped");
        assert!(tracker.finish(doc, second), "the newest job applies");
        assert!(tracker.is_empty(), "finishing consumes the generation");
        assert!(!tracker.finish(doc, second), "a duplicate completion changes nothing");
    }

    #[test]
    fn generations_are_independent_per_document() {
        let mut tracker = GenerationTracker::default();
        let first = tracker.begin(DocumentId(1));
        let second = tracker.begin(DocumentId(2));

        assert!(tracker.is_current(DocumentId(1), first));
        assert!(tracker.is_current(DocumentId(2), second));
        tracker.forget(DocumentId(1));
        assert!(!tracker.finish(DocumentId(1), first));
        assert!(tracker.finish(DocumentId(2), second));
    }

    #[test]
    fn a_load_job_reports_a_missing_file() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared);
        state.open_path(temp.path().join("missing.md"));

        wait_for_load(&mut state, |state| {
            state
                .active_tab()
                .and_then(|tab| tab.document())
                .is_some_and(|doc| matches!(*doc.status.borrow(), lmdv_core::DocStatus::Error(_)))
        });

        let doc = state.active_tab().and_then(|tab| tab.document()).expect("the file tab");
        assert!(matches!(*doc.status.borrow(), lmdv_core::DocStatus::Error(_)));
        assert!(state.ui.notice.is_some(), "the failure is visible");
    }

    #[test]
    fn a_load_job_fills_the_document_and_records_a_recent() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut file = NamedTempFile::new_in(temp.path()).unwrap();
        file.write_all(b"# Hello\n\nBody text\n").unwrap();
        file.flush().unwrap();

        let mut state = WindowState::new(shared.clone());
        state.open_path(file.path().to_path_buf());
        wait_for_load(&mut state, |state| {
            state
                .active_tab()
                .and_then(|tab| tab.document())
                .is_some_and(|doc| matches!(*doc.status.borrow(), lmdv_core::DocStatus::Ready))
        });

        let doc = state.active_tab().and_then(|tab| tab.document()).expect("the file tab");
        assert_eq!(doc.text().text(), "# Hello\n\nBody text\n");
        assert!(matches!(*doc.status.borrow(), lmdv_core::DocStatus::Ready));
        assert!(!doc.is_dirty(), "a fresh load is the saved base");
        assert_eq!(shared.recents().len(), 1);
    }

    #[test]
    fn a_completion_after_close_is_dropped() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared);
        let tab = state.open_synthetic("Note", "text");
        let doc = state.tab(tab).and_then(|tab| tab.document()).cloned().expect("the synthetic doc");
        let generation = state.tracker_begin(doc.id);

        assert_eq!(state.close_tab(tab), CloseOutcome::Closed);
        assert!(!state.tracker_finish(doc.id, generation), "the closed tab's generation is forgotten");
    }

    #[test]
    fn a_parse_job_reports_a_model_with_its_revision() {
        let mailbox = Arc::new(WorkerMailbox::new(WindowId(1)));
        let hub = JobHub::new();
        hub.parse(mailbox.clone(), DocumentId(3), 9, Arc::from("# Hi\n\nBody\n"));

        let deadline = Instant::now() + Duration::from_secs(5);
        while mailbox.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let drained = mailbox.drain();
        let Some(JobMsg::Parsed { doc, revision, result }) = drained.into_iter().next() else {
            panic!("the parse job never reported");
        };
        assert_eq!(doc, DocumentId(3));
        assert_eq!(revision, 9);
        let model = result.expect("the fixture parses");
        assert_eq!(model.revision, 9);
        assert_eq!(model.headings[0].text, "Hi");
    }

    #[test]
    fn mailbox_drains_in_post_order() {
        let mailbox = Arc::new(WorkerMailbox::new(WindowId(1)));
        mailbox.post(JobMsg::Stored { job: 1, ok: true });
        mailbox.post(JobMsg::Stored { job: 2, ok: false });
        let drained = mailbox.drain();
        assert_eq!(drained.len(), 2);
        assert!(mailbox.is_empty());
    }

    #[test]
    fn posting_wakes_the_attached_waker() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mailbox = WorkerMailbox::new(WindowId(1));
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        mailbox.attach(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        mailbox.post(JobMsg::Stored { job: 1, ok: true });
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }
}
