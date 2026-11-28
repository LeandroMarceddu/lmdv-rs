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
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lmdv_core::{DocumentId, Draft, Eol, FileMeta, JobId, Prefs, TabId, WindowId};
use lmdv_edit::spell::{Availability, DictionaryFiles, Spell};
use lmdv_io::AppDirs;
use lmdv_io::store::{
    DraftIssue, PositionTable, Recent, StoreHandle, StoreIssue, UiPrefs, load_draft_reported, load_positions_reported,
    load_prefs_reported, load_recents_reported, load_ui_reported,
};
use lmdv_io::{PathPolicy, ReadError, SaveOutcome, WriteError, read_document, save_document};
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
    /// A crash draft read finished (E-17). `None` when no draft exists; an
    /// `issue` means the file is unreadable and must not be overwritten (OI-169).
    DraftLoaded { doc: DocumentId, key: String, draft: Option<Draft>, issue: Option<DraftIssue> },
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
    /// A wiki-style local link probe finished (L-03/L-11).
    ///
    /// `tab`, `doc`, `dest`, `fragment` and `placement` are the click's own
    /// context, so the completion can honor the active-tab guard and place the
    /// document exactly as the gesture asked.
    LinkResolved {
        job: JobId,
        tab: TabId,
        doc: DocumentId,
        dest: String,
        fragment: Option<String>,
        placement: crate::links::LinkPlacement,
        result: Result<Option<lmdv_io::LinkResolution>, String>,
    },
}

/// The four state files read once at startup, on a worker (OI-001).
///
/// The UI thread starts this job before the first frame and waits for it at the
/// point where the saved locale must be applied, so headless scenes and the
/// first interactive frame see exactly the values the synchronous loader used
/// to provide. Only the file reads move off the thread.
#[derive(Debug, Default)]
pub struct StartupState {
    pub prefs: Prefs,
    pub ui: UiPrefs,
    pub recents: Vec<Recent>,
    pub positions: PositionTable,
    /// Corrupt/quarantined and future-schema/foreign files the writer must
    /// respect, in the loader's original order.
    pub issues: Vec<StoreIssue>,
}

/// What one image path-policy probe asked for and found (OI-049).
///
/// The key is the normalized source the view also uses, so a hit is exact and
/// never re-runs `PathPolicy`'s stat/canonicalize on the UI thread.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ImageProbeKey {
    pub(crate) root: PathBuf,
    pub(crate) source: String,
    pub(crate) allow_outside: bool,
    pub(crate) url_path: bool,
}

/// The answer to one [`ImageProbeKey`], posted by the worker to
/// `crate::images`' probe hub.
#[derive(Debug)]
pub(crate) struct ImageProbe {
    pub(crate) key: ImageProbeKey,
    pub(crate) path: Option<PathBuf>,
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

/// A spellcheck completion (ADR-008, E-SPELL).
///
/// The dictionary is discovered and loaded on workers, and so is every check:
/// a frame never reads a Hunspell file or walks a paragraph.
#[derive(Debug)]
pub(crate) enum SpellMsg {
    /// The folders were scanned. `None` means no usable dictionary exists, so
    /// the settings row stays hidden.
    Discovered { generation: u64, found: Option<Availability> },
    /// A dictionary pair was parsed, or could not be.
    Loaded { generation: u64, result: Result<Arc<Spell>, String> },
    /// One text's misspelled ranges, in whole-source byte coordinates.
    Checked { job: JobId, tab: TabId, revision: u64, window: Range<usize>, whole: bool, ranges: Vec<Range<usize>> },
}

/// The spellchecker's own worker mailbox.
///
/// It is separate from [`WorkerMailbox`] so adding spelling never has to teach
/// the window's job dispatcher a new [`JobMsg`]: the editor drains it in
/// `poll_spell`.
#[derive(Default)]
pub(crate) struct SpellMailbox {
    jobs: Mutex<VecDeque<SpellMsg>>,
    waker: Mutex<Option<Waker>>,
}

impl SpellMailbox {
    /// An empty mailbox.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Installs the UI waker, or replaces a stale one after a host change.
    pub(crate) fn attach(&self, waker: Waker) {
        if let Ok(mut slot) = self.waker.lock() {
            *slot = Some(waker);
        }
    }

    /// Queues a completion and wakes the UI.
    pub(crate) fn post(&self, msg: SpellMsg) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push_back(msg);
        }
        let waker = self.waker.lock().ok().and_then(|slot| slot.clone());
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Takes every queued completion in order.
    pub(crate) fn drain(&self) -> Vec<SpellMsg> {
        match self.jobs.lock() {
            Ok(mut jobs) => jobs.drain(..).collect(),
            Err(_) => Vec::new(),
        }
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

    /// Reads the four startup state files on a worker and returns a one-shot
    /// receiver for the result (OI-001).
    ///
    /// The reported loaders do quarantine and schema checks too, so keeping
    /// them together here means the UI thread never stats a state file. A
    /// worker that cannot start sends a default value, so waiting always ends.
    pub fn load_startup(&self, dirs: AppDirs) -> Receiver<StartupState> {
        let (tx, rx) = channel();
        let worker_tx = tx.clone();
        let spawned = rune::task::spawn("lmdv-startup", move || {
            let prefs = load_prefs_reported(&dirs);
            let recents = load_recents_reported(&dirs);
            let positions = load_positions_reported(&dirs);
            let ui = load_ui_reported(&dirs);
            let issues = [prefs.issue, recents.issue, positions.issue, ui.issue].into_iter().flatten().collect();
            let _ = worker_tx.send(StartupState {
                prefs: prefs.value,
                ui: ui.value,
                recents: recents.value,
                positions: positions.value,
                issues,
            });
        });
        if spawned.is_err() {
            // Match the other job helpers: a spawn failure is an empty answer,
            // never a hang. The app degrades to a fresh session.
            let _ = tx.send(StartupState::default());
        }
        rx
    }

    /// Resolves one image path on a worker and sends its answer to `tx`
    /// (OI-049).
    ///
    /// The view has already applied URL/fragment/scheme policy to `key.source`;
    /// this worker only runs `PathPolicy`'s metadata probe and canonicalize,
    /// which are the expensive part. `None` is a valid answer (missing,
    /// refused or too large) and still completes the waiter.
    pub(crate) fn probe_image(&self, tx: Sender<ImageProbe>, key: ImageProbeKey) -> JobId {
        let job = self.allocate();
        let worker_tx = tx.clone();
        let worker_key = key.clone();
        let spawned = rune::task::spawn("lmdv-image-probe", move || {
            let path = PathPolicy::new(&worker_key.root)
                .with_allow_outside(worker_key.allow_outside)
                .resolve_asset(Path::new(&worker_key.source))
                .ok()
                .map(|file| file.path);
            let _ = worker_tx.send(ImageProbe { key: worker_key, path });
        });
        if spawned.is_err() {
            let _ = tx.send(ImageProbe { key, path: None });
        }
        job
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
            let result = crate::export_io::parse_snapshot(&source, revision);
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
            let loaded = load_draft_reported(&dirs, &worker_key);
            worker_mailbox.post(JobMsg::DraftLoaded { doc, key: worker_key, draft: loaded.value, issue: loaded.issue });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::DraftLoaded { doc, key, draft: None, issue: None });
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

    /// Probes a local link's wiki-style spellings on a worker and posts
    /// [`JobMsg::LinkResolved`] (L-03).
    ///
    /// The candidate order and canonicalization live in
    /// [`lmdv_io::resolve_document_link`]; this method only carries the click's
    /// placement context back so the UI can honor its active-tab guard.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_link(
        &self,
        mailbox: Arc<WorkerMailbox>,
        tab: TabId,
        doc: DocumentId,
        dest: String,
        fragment: Option<String>,
        placement: crate::links::LinkPlacement,
        root: PathBuf,
        path: PathBuf,
    ) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let worker_dest = dest.clone();
        let worker_fragment = fragment.clone();
        let spawned = rune::task::spawn("lmdv-link", move || {
            let result = lmdv_io::resolve_document_link(&root, &path).map_err(|error| error.to_string());
            worker_mailbox.post(JobMsg::LinkResolved {
                job,
                tab,
                doc,
                dest: worker_dest,
                fragment: worker_fragment,
                placement,
                result,
            });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::LinkResolved {
                job,
                tab,
                doc,
                dest,
                fragment,
                placement,
                result: Err("the link worker could not start".to_owned()),
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

    /// Reveals a path in the desktop's file manager on a worker and posts
    /// [`JobMsg::Opened`]. The file manager selects the file where it can
    /// (X-05, OI-104); the handler never spawns a process.
    pub fn reveal(&self, mailbox: Arc<WorkerMailbox>, path: PathBuf) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let worker_path = path.clone();
        let spawned = rune::task::spawn("lmdv-reveal", move || {
            let result = crate::export_io::reveal_path(&worker_path);
            worker_mailbox.post(JobMsg::Opened { job, path: worker_path, result });
        });
        if spawned.is_err() {
            mailbox.post(JobMsg::Opened { job, path, result: Err("the reveal worker could not start".to_owned()) });
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

    /// Scans the Hunspell folders and posts [`SpellMsg::Discovered`].
    ///
    /// Reading a folder is file I/O, so it never runs on the UI thread
    /// (ADR-008). The scan only looks for `.aff`/`.dic` pairs; parsing happens
    /// in [`Self::load_spelling`] when the editor actually needs the checker.
    pub(crate) fn discover_spelling(
        &self,
        mailbox: Arc<SpellMailbox>,
        generation: u64,
        roots: Vec<PathBuf>,
        tags: Vec<String>,
    ) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let spawned = rune::task::spawn("lmdv-spell-dict", move || {
            let found = lmdv_edit::spell::discover(&roots, &tags);
            worker_mailbox.post(SpellMsg::Discovered { generation, found });
        });
        if spawned.is_err() {
            mailbox.post(SpellMsg::Discovered { generation, found: None });
        }
        job
    }

    /// Parses one dictionary pair and posts [`SpellMsg::Loaded`].
    ///
    /// spellbook's own documentation asks for this to stay off the main
    /// thread: a large dictionary takes tens to hundreds of milliseconds.
    pub(crate) fn load_spelling(&self, mailbox: Arc<SpellMailbox>, generation: u64, files: DictionaryFiles) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let spawned = rune::task::spawn("lmdv-spell-load", move || {
            let result = Spell::open(&files).map(Arc::new).map_err(|error| error.to_string());
            worker_mailbox.post(SpellMsg::Loaded { generation, result });
        });
        if spawned.is_err() {
            mailbox
                .post(SpellMsg::Loaded { generation, result: Err("the spellcheck worker could not start".to_owned()) });
        }
        job
    }

    /// Checks one text slice against a loaded dictionary and posts
    /// [`SpellMsg::Checked`].
    ///
    /// `window` is the slice's whole-source range; the returned ranges are
    /// shifted into it. `whole` says the slice is the entire buffer, so the
    /// single field may paint every range.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn check_spelling(
        &self,
        mailbox: Arc<SpellMailbox>,
        tab: TabId,
        revision: u64,
        spell: Arc<Spell>,
        window: Range<usize>,
        text: String,
        whole: bool,
    ) -> JobId {
        let job = self.allocate();
        let worker_mailbox = mailbox.clone();
        let base = window.start;
        let returned = window.clone();
        let spawned = rune::task::spawn("lmdv-spell-check", move || {
            let ranges =
                spell.check(&text).into_iter().map(|range| base + range.start..base + range.end).collect::<Vec<_>>();
            worker_mailbox.post(SpellMsg::Checked { job, tab, revision, window: returned, whole, ranges });
        });
        if spawned.is_err() {
            mailbox.post(SpellMsg::Checked { job, tab, revision, window, whole, ranges: Vec::new() });
        }
        job
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;
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
    fn an_image_probe_resolves_and_refuses_paths_on_the_worker() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("docs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.png"), b"not decoded here").unwrap();
        std::fs::write(temp.path().join("secret.png"), b"outside").unwrap();
        let (tx, rx) = channel();
        let hub = JobHub::new();

        hub.probe_image(
            tx.clone(),
            ImageProbeKey { root: root.clone(), source: "a.png".to_owned(), allow_outside: false, url_path: true },
        );
        let answer = rx.recv_timeout(Duration::from_secs(5)).expect("the probe answered");
        assert_eq!(answer.path, Some(root.join("a.png")));

        hub.probe_image(
            tx,
            ImageProbeKey {
                root: root.clone(),
                source: "../secret.png".to_owned(),
                allow_outside: false,
                url_path: true,
            },
        );
        let answer = rx.recv_timeout(Duration::from_secs(5)).expect("the refused probe answered");
        assert_eq!(answer.path, None, "the path policy still refuses files outside the root");
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

    /// Waits for one spellcheck completion.
    fn wait_spell(mailbox: &SpellMailbox) -> SpellMsg {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut drained = mailbox.drain();
            if let Some(message) = drained.pop() {
                assert!(drained.is_empty(), "one job, one completion");
                return message;
            }
            assert!(Instant::now() < deadline, "the spellcheck job never reported");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_spell_job_discovers_loads_and_checks_off_the_ui_thread() {
        let mailbox = Arc::new(SpellMailbox::new());
        let hub = JobHub::new();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/spell/system");
        hub.discover_spelling(mailbox.clone(), 4, vec![root], vec!["it".to_owned()]);
        let SpellMsg::Discovered { generation, found } = wait_spell(&mailbox) else {
            panic!("the discovery completion");
        };
        assert_eq!(generation, 4);
        let found = found.expect("the Italian fixture");
        assert_eq!(found.language(), "it_IT");

        hub.load_spelling(mailbox.clone(), 5, found.files);
        let SpellMsg::Loaded { generation, result } = wait_spell(&mailbox) else {
            panic!("the load completion");
        };
        assert_eq!(generation, 5);
        let spell = result.expect("the fixture dictionary parses");

        // A slice at a non-zero window start returns whole-source ranges.
        let tab: TabId = 2;
        hub.check_spelling(mailbox.clone(), tab, 9, spell, 10..20, "ciao wrld".to_owned(), false);
        let SpellMsg::Checked { tab: checked, revision, window, whole, ranges, .. } = wait_spell(&mailbox) else {
            panic!("the check completion");
        };
        assert_eq!((checked, revision, window, whole), (tab, 9, 10..20, false));
        assert_eq!(ranges, vec![15..19], "`wrld` is shifted into the source");
    }

    #[test]
    fn a_failed_spell_load_reports_itself() {
        let mailbox = Arc::new(SpellMailbox::new());
        let hub = JobHub::new();
        let files = DictionaryFiles {
            language: "xx_XX".to_owned(),
            aff: PathBuf::from("/nonexistent/lmdv/xx_XX.aff"),
            dic: PathBuf::from("/nonexistent/lmdv/xx_XX.dic"),
        };
        hub.load_spelling(mailbox.clone(), 1, files);
        let SpellMsg::Loaded { result, .. } = wait_spell(&mailbox) else { panic!("the load completion") };
        assert!(result.is_err(), "a missing pair is an error, not a panic");
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
