//! Process-wide state shared by every window: documents, preferences, recents,
//! the store writer, the window registry and launch routing.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lmdv_core::{DocumentId, DocumentKey, DocumentRegistry, PasteId, Prefs, ScrollAnchor, WindowId};
use lmdv_edit::spell::{Availability, Spell};
use lmdv_io::instance::{InstanceServer, LaunchRequest};
use lmdv_io::store::{
    PositionTable, RECENTS_MAX, Recent, SessionSnapshot, StateFile, StoreHandle, StoreIssue, StorePatch, UiPrefs,
    WindowSession, load_positions_reported, load_prefs_reported, load_recents_reported, load_ui_reported,
};
use lmdv_io::watch::{DocWatcher, FileEvent};
use lmdv_io::{AppDirs, DirsIssue, Launch};
use lmdv_md::MdDoc;
use rune::{Waker, WindowPlacement};

use crate::jobs::{JobHub, WorkerMailbox};
use crate::transfer::{TransferMsg, TransferToken};
use crate::windows::{
    OpenedWindow, RouteAction, RouteDecision, RouteQuery, WakeSet, WindowEntry, WindowHandles, WindowKind,
    WindowOpenRequest, WindowOpener, WindowRegistry, WindowSnapshot, close_title,
};

/// A test seam that records a window's always-on-top request.
type PinSink = Box<dyn Fn(WindowId, bool)>;
/// A test seam that records a desktop-open request.
type ExternalSink = Box<dyn Fn(&Path)>;

/// The application id reported to the desktop (Wayland `app_id`, X11 `WM_CLASS`).
/// It matches the desktop-file basename (`lmdv.desktop`), not the XDG data root.
pub const APP_ID: &str = "lmdv";

/// State every window reads and writes through `Rc`.
///
/// Created once on the UI thread by [`AppShared::start`]. Workers only ever see
/// `Send` values (paths, buffers, store handles, job mailboxes), never this
/// type.
pub struct AppShared {
    dirs: AppDirs,
    prefs: RefCell<Prefs>,
    /// File-browser root and collapsed folders (`ui.json`, F-07).
    ui: RefCell<UiPrefs>,
    docs: RefCell<DocumentRegistry>,
    windows: RefCell<WindowRegistry>,
    recents: RefCell<Vec<Recent>>,
    positions: RefCell<PositionTable>,
    /// The latest parse per document, shared by every window showing it.
    parse: RefCell<ParseCache>,
    /// `None` when the XDG directory could not be created; the app still runs
    /// and shows `store_error` instead of persisting.
    store: Option<StoreHandle>,
    store_error: Option<String>,
    jobs: JobHub,
    /// The UI wakers of every live window.
    wake_set: WakeSet,
    /// Opens a window at runtime; installed by the app entry point only, so
    /// unit tests never call `rune::open_window`.
    opener: Cell<Option<WindowOpener>>,
    /// The single window that drains the shared launch queue.
    router: Cell<Option<WindowId>>,
    /// The documents claimed by a window before their tab appears.
    claims: RefCell<BTreeMap<DocumentId, WindowId>>,
    /// Monotonic focus stamps for the routing order.
    next_focus: Cell<u64>,
    /// Launches forwarded by later `lmdv` processes; drained by the router's `poll`.
    launch: Arc<LaunchInbox>,
    /// Kept alive for the process lifetime; dropping it removes the socket.
    instance: RefCell<Option<InstanceServer>>,
    next_window: Cell<u64>,
    next_transfer: Cell<u64>,
    next_paste: Cell<u64>,
    /// The latest restorable state of every live window, by process id.
    session_windows: RefCell<BTreeMap<WindowId, WindowSession>>,
    quitting: Cell<bool>,
    /// A headless run (`--snap`, `--tree`, `--script`): parsing and picture
    /// decoding happen before the first frame so the gate is deterministic.
    headless: Cell<bool>,
    /// Whether the platform exposes global window positions (X11), resolved
    /// once from [`crate::session::positions_available`].
    screen_positions: Cell<Option<bool>>,
    /// Whether this desktop honours always-on-top, resolved once. Wayland has
    /// no protocol for it, so it answers false there.
    pin_support: Cell<Option<bool>>,
    /// The test seam for `view.pin`'s native call; `None` uses the live handle.
    pin_sink: RefCell<Option<PinSink>>,
    /// The test seam for opening a path with the desktop; `None` runs the job.
    external_sink: RefCell<Option<ExternalSink>>,
    /// The watcher service thread's handle; it owns the `DocWatcher`, so no
    /// watch setup runs on the UI thread (D7).
    watcher: RefCell<Option<WatcherService>>,
    /// Watcher failures are reported once, not retried every open.
    watch_error: Arc<Mutex<Option<String>>>,
    /// Reference counts for watched folders' files.
    watch_counts: RefCell<BTreeMap<PathBuf, usize>>,
    /// Events pushed by the watcher thread, with a sequence for each window.
    watch_inbox: Arc<FileEventInbox>,
    /// Documents whose draft has already been looked at (E-17 once per doc).
    draft_checked: RefCell<BTreeSet<DocumentId>>,
    /// Startup fallbacks and ignored state files; the first window shows them.
    startup: RefCell<StartupWarnings>,
    /// The document watcher's setup error is shown once, not every frame (OI-073).
    watch_error_shown: Cell<bool>,
    /// Documents whose large-document warning has already been shown (OI-010).
    large_warned: RefCell<BTreeSet<DocumentId>>,
    /// The Linux spellchecker's dictionary choice and loaded engine (ADR-008).
    spell: RefCell<SpellRuntime>,
}

/// What the process knows about the Linux spellchecker (ADR-008, E-SPELL).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SpellDiscovery {
    /// No window has asked yet.
    #[default]
    Unknown,
    /// A worker is scanning the folders.
    Pending,
    /// The scan finished; `None` means no usable dictionary exists, and the
    /// settings row stays hidden.
    Ready(Option<Availability>),
}

/// The process-wide checker: one dictionary, chosen at the first scan from the
/// UI locale and kept until the next start (E-32, P-11).
#[derive(Default)]
struct SpellRuntime {
    discovery: SpellDiscovery,
    dictionary: Option<Arc<Spell>>,
    discovery_generation: u64,
    load_generation: u64,
    loading: bool,
    /// When the running scan/load started. A window that closed before its
    /// worker answered would otherwise leave the process without a checker.
    discovery_started: Option<std::time::Instant>,
    load_started: Option<std::time::Instant>,
}

/// A scan or load whose window disappeared is asked again after this long.
const SPELL_RETRY: Duration = Duration::from_secs(10);

/// What startup found wrong with the resolved folders and the state files.
///
/// Both kinds are reported once by the process's first window: a directory
/// fallback and a quarantined or protected state file are facts about the
/// launch, not about each window.
#[derive(Default)]
struct StartupWarnings {
    dirs: Vec<DirsIssue>,
    store: Vec<StoreIssue>,
    shown: bool,
}

impl AppShared {
    /// Loads preferences and recents and starts the store writer.
    ///
    /// This runs before the first frame, not in a handler or a view. All
    /// subsequent disk work goes through `crate::jobs`. A directory that
    /// cannot be created degrades the app to a session without persistence; it
    /// never fails startup.
    pub fn start(dirs: AppDirs) -> Rc<Self> {
        Self::start_reported(dirs, Vec::new())
    }

    /// [`Self::start`] with the folder-resolution issues the launch saw.
    ///
    /// The reported loaders keep a future-schema or foreign-app state file on
    /// disk: the writer starts with those files read-only, and the first
    /// window shows one notice for every fallback or ignored file (OI-007,
    /// OI-012).
    pub fn start_reported(dirs: AppDirs, dirs_issues: Vec<DirsIssue>) -> Rc<Self> {
        let prefs = load_prefs_reported(&dirs);
        let recents = load_recents_reported(&dirs);
        let positions = load_positions_reported(&dirs);
        let ui = load_ui_reported(&dirs);
        let store_issues: Vec<StoreIssue> =
            [prefs.issue, recents.issue, positions.issue, ui.issue].into_iter().flatten().collect();
        let (store, store_error) = match StoreHandle::start_with(dirs.clone(), &store_issues) {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(format!("settings cannot be saved: {error}"))),
        };
        Rc::new(Self {
            dirs,
            prefs: RefCell::new(prefs.value),
            ui: RefCell::new(ui.value),
            docs: RefCell::new(DocumentRegistry::new()),
            windows: RefCell::new(WindowRegistry::new()),
            recents: RefCell::new(recents.value),
            positions: RefCell::new(positions.value),
            parse: RefCell::new(ParseCache::default()),
            store,
            store_error,
            jobs: JobHub::new(),
            wake_set: WakeSet::new(),
            opener: Cell::new(None),
            router: Cell::new(None),
            claims: RefCell::new(BTreeMap::new()),
            next_focus: Cell::new(0),
            launch: Arc::new(LaunchInbox::default()),
            instance: RefCell::new(None),
            next_window: Cell::new(1),
            next_transfer: Cell::new(1),
            next_paste: Cell::new(1),
            session_windows: RefCell::new(BTreeMap::new()),
            quitting: Cell::new(false),
            headless: Cell::new(false),
            screen_positions: Cell::new(None),
            pin_support: Cell::new(None),
            pin_sink: RefCell::new(None),
            external_sink: RefCell::new(None),
            watcher: RefCell::new(None),
            watch_error: Arc::new(Mutex::new(None)),
            watch_counts: RefCell::new(BTreeMap::new()),
            watch_inbox: Arc::new(FileEventInbox::default()),
            draft_checked: RefCell::new(BTreeSet::new()),
            startup: RefCell::new(StartupWarnings { dirs: dirs_issues, store: store_issues, shown: false }),
            watch_error_shown: Cell::new(false),
            large_warned: RefCell::new(BTreeSet::new()),
            spell: RefCell::new(SpellRuntime::default()),
        })
    }

    /// Marks this process as Rune's headless tooling.
    ///
    /// Headless gates capture a single settled frame, so the parse and picture
    /// decode run inline before it. Interactive windows never take this path.
    pub fn set_headless(&self) {
        self.headless.set(true);
        // The tear-out rule depends on both facts; resolve it again.
        self.screen_positions.set(None);
    }

    /// Whether this is a headless tooling run.
    pub fn is_headless(&self) -> bool {
        self.headless.get()
    }

    /// Whether this process exposes global window positions (X11), so a
    /// torn-out tab can be placed from its release point.
    ///
    /// Wayland hides window positions and a headless run has no desktop; both
    /// answer false, and the tab strip then leaves tear-out off so the tab
    /// menu remains the substitute (T-20/T-21, `docs/ARCHITECTURE_DETAIL.md`
    /// §7.2). Resolved once: the window system does not change mid-process.
    pub fn screen_positions(&self) -> bool {
        if let Some(known) = self.screen_positions.get() {
            return known;
        }
        let available = !self.is_headless() && crate::session::positions_available();
        self.screen_positions.set(Some(available));
        available
    }

    /// Whether this process's window system honours always-on-top.
    ///
    /// Wayland has no such protocol: the request is a no-op there, so the pin
    /// command refuses instead of showing a check that does nothing. Resolved
    /// once, like [`Self::screen_positions`] (the same platform fact).
    pub fn pin_available(&self) -> bool {
        if let Some(known) = self.pin_support.get() {
            return known;
        }
        let available = crate::session::positions_available();
        self.pin_support.set(Some(available));
        available
    }

    /// What this process knows about the Linux checker (ADR-008, E-SPELL).
    pub fn spell_discovery(&self) -> SpellDiscovery {
        self.spell.borrow().discovery.clone()
    }

    /// The dictionary the checker will use, once discovery found one.
    ///
    /// The settings row is shown only while this answers `Some`; a machine
    /// without a dictionary hides the toggle entirely (E-SPELL).
    pub fn spell_availability(&self) -> Option<Availability> {
        match &self.spell.borrow().discovery {
            SpellDiscovery::Ready(found) => found.clone(),
            SpellDiscovery::Unknown | SpellDiscovery::Pending => None,
        }
    }

    /// Marks discovery as running and returns its generation, or `None` when
    /// another window already asked or the scan is done.
    pub fn begin_spell_discovery(&self) -> Option<u64> {
        let mut spell = self.spell.borrow_mut();
        let stale = spell.discovery == SpellDiscovery::Pending
            && spell.discovery_started.is_some_and(|started| started.elapsed() > SPELL_RETRY);
        if spell.discovery != SpellDiscovery::Unknown && !stale {
            return None;
        }
        spell.discovery_generation = spell.discovery_generation.wrapping_add(1);
        spell.discovery = SpellDiscovery::Pending;
        spell.discovery_started = Some(std::time::Instant::now());
        Some(spell.discovery_generation)
    }

    /// Accepts a discovery answer. A stale generation changes nothing.
    pub fn finish_spell_discovery(&self, generation: u64, found: Option<Availability>) -> bool {
        let mut spell = self.spell.borrow_mut();
        if spell.discovery != SpellDiscovery::Pending || spell.discovery_generation != generation {
            return false;
        }
        spell.discovery = SpellDiscovery::Ready(found);
        spell.discovery_started = None;
        true
    }

    /// The loaded dictionary, shared by every window.
    pub fn spell_dictionary(&self) -> Option<Arc<Spell>> {
        self.spell.borrow().dictionary.clone()
    }

    /// Starts one dictionary load, if a dictionary exists and none is loading.
    pub fn begin_spell_load(&self) -> Option<u64> {
        self.spell_availability()?;
        let mut spell = self.spell.borrow_mut();
        let stale = spell.loading && spell.load_started.is_some_and(|started| started.elapsed() > SPELL_RETRY);
        if spell.dictionary.is_some() || (spell.loading && !stale) {
            return None;
        }
        spell.load_generation = spell.load_generation.wrapping_add(1);
        spell.loading = true;
        spell.load_started = Some(std::time::Instant::now());
        Some(spell.load_generation)
    }

    /// Accepts a load answer. `None` means the pair could not be parsed, so the
    /// toggle is hidden for the rest of the process.
    pub fn finish_spell_load(&self, generation: u64, dictionary: Option<Arc<Spell>>) -> bool {
        let mut spell = self.spell.borrow_mut();
        if !spell.loading || spell.load_generation != generation {
            return false;
        }
        spell.loading = false;
        spell.load_started = None;
        match dictionary {
            Some(dictionary) => {
                spell.dictionary = Some(dictionary);
            }
            None => spell.discovery = SpellDiscovery::Ready(None),
        }
        true
    }

    /// Seeds the checker for a unit test, skipping the worker round trip.
    #[cfg(test)]
    pub(crate) fn seed_spell(&self, availability: Option<Availability>, dictionary: Option<Arc<Spell>>) {
        let mut spell = self.spell.borrow_mut();
        spell.discovery = SpellDiscovery::Ready(availability);
        spell.dictionary = dictionary;
        spell.loading = false;
        spell.discovery_started = None;
        spell.load_started = None;
    }

    /// Sends one window's always-on-top state, through the native handle or a
    /// test seam. False when the window has no native host yet.
    pub fn set_always_on_top(&self, id: WindowId, on: bool) -> bool {
        if let Some(sink) = self.pin_sink.borrow().as_ref() {
            sink(id, on);
            return true;
        }
        let native = self
            .windows
            .borrow()
            .get(id)
            .and_then(|entry| entry.handles.as_ref())
            .map(|handles| handles.native.clone());
        let Some(native) = native else { return false };
        native.set_always_on_top(on);
        true
    }

    /// Records `view.pin`'s requests instead of calling a window manager.
    #[cfg(test)]
    pub(crate) fn set_pin_sink(&self, sink: Box<dyn Fn(WindowId, bool)>) {
        *self.pin_sink.borrow_mut() = Some(sink);
    }

    /// Forces [`Self::pin_available`] for a test (the Wayland refusal).
    #[cfg(test)]
    pub(crate) fn set_pin_available(&self, available: bool) {
        self.pin_support.set(Some(available));
    }

    /// Opens a path with the desktop's handler on a worker, or hands it to the
    /// test sink. Returns the job, or `None` when a test recorded the request.
    pub fn open_external(&self, mailbox: Arc<WorkerMailbox>, path: PathBuf) -> Option<lmdv_core::JobId> {
        if let Some(sink) = self.external_sink.borrow().as_ref() {
            sink(&path);
            return None;
        }
        Some(self.jobs.open_external(mailbox, path))
    }

    /// Records desktop-open requests instead of spawning a handler.
    #[cfg(test)]
    pub(crate) fn set_external_sink(&self, sink: Box<dyn Fn(&Path)>) {
        *self.external_sink.borrow_mut() = Some(sink);
    }

    /// The resolved XDG (or portable/`LMDV_HOME`) directories.
    pub fn dirs(&self) -> &AppDirs {
        &self.dirs
    }

    /// The loaded preferences. The P1 reader follows the system palette; the
    /// reader typography and theme preferences are wired in P2/P4.
    pub fn prefs(&self) -> Ref<'_, Prefs> {
        self.prefs.borrow()
    }

    /// Replaces the preferences after a settings change.
    ///
    /// This is the one preference writer: it normalizes the set, keeps the
    /// in-memory copy current, queues one coalesced `prefs.json` patch, applies
    /// a changed language to the shared translator, and tells every open
    /// window to redraw ([`UiMsg::PrefsChanged`]). Returns whether anything
    /// changed, so a no-op control does not touch the store or wake windows.
    pub fn set_prefs(&self, mut prefs: Prefs) -> bool {
        self.replace_prefs(&mut prefs, true)
    }

    /// Applies preferences without queueing `prefs.json`.
    ///
    /// Headless scenes preview a scheme or a reading size (`reader-sepia`,
    /// `editor-size`) without changing the user's stored settings. The app's
    /// settings dialog never takes this path: it always goes through
    /// [`Self::set_prefs`].
    pub(crate) fn preview_prefs(&self, mut prefs: Prefs) -> bool {
        self.replace_prefs(&mut prefs, false)
    }

    /// The one preference writer behind [`Self::set_prefs`] and
    /// [`Self::preview_prefs`].
    fn replace_prefs(&self, prefs: &mut Prefs, persist: bool) -> bool {
        prefs.sanitize();
        let previous = self.prefs.borrow().clone();
        if previous == *prefs {
            return false;
        }
        let locale_changed = previous.locale != prefs.locale;
        *self.prefs.borrow_mut() = prefs.clone();
        if persist {
            self.queue_store(StorePatch::Prefs(prefs.clone()));
        }
        if locale_changed {
            // Unit tests share one process-wide translator; switching it inside
            // a parallel test would make another test's English assertions
            // locale-dependent. The app and every headless run apply the
            // language live.
            if !cfg!(test) {
                crate::settings::apply_locale(prefs);
            }
        }
        self.announce_prefs_changed();
        true
    }

    /// Tells every registered window that the preferences changed.
    ///
    /// The sending window is included: its handlers already redraw, and the
    /// message makes the invalidation single-sourced. A window with no host
    /// yet simply keeps the message for its first `poll`.
    fn announce_prefs_changed(&self) {
        let ids: Vec<WindowId> = self.windows.borrow().iter().map(|entry| entry.id).collect();
        for id in ids {
            let _ = self.try_post_window(id, UiMsg::PrefsChanged);
        }
    }

    /// The window/file-browser state that survives a restart.
    pub fn ui(&self) -> Ref<'_, UiPrefs> {
        self.ui.borrow()
    }

    /// Replaces the UI state, normalized, and queues `ui.json`.
    ///
    /// The file browser owns the root and collapsed folders; pane geometry is
    /// kept alongside them so one writer replaces the whole file.
    pub fn set_ui(&self, mut ui: UiPrefs) {
        ui.normalize();
        *self.ui.borrow_mut() = ui.clone();
        self.queue_store(StorePatch::Ui(ui));
    }

    /// The one document per file.
    pub fn docs(&self) -> Ref<'_, DocumentRegistry> {
        self.docs.borrow()
    }

    /// Mutable registry access for insertion, rekeying and collection.
    pub fn docs_mut(&self) -> RefMut<'_, DocumentRegistry> {
        self.docs.borrow_mut()
    }

    /// Recent documents, newest first, capped by the store.
    pub fn recents(&self) -> Ref<'_, [Recent]> {
        Ref::map(self.recents.borrow(), Vec::as_slice)
    }

    /// Remembered reading positions, keyed by canonical document path.
    pub fn positions(&self) -> Ref<'_, PositionTable> {
        self.positions.borrow()
    }

    /// The remembered anchor for `path`, when the document is not at the top.
    pub fn position_for(&self, path: &Path) -> Option<ScrollAnchor> {
        self.positions.borrow().get(path)
    }

    /// Remembers `anchor` for `path` and queues one coalesced store patch.
    ///
    /// An at-top anchor forgets the entry (R-14). Called from the reading
    /// timers only, never on a frame that does no reading work.
    pub fn remember_position(&self, path: &Path, anchor: &ScrollAnchor) {
        let mut positions = self.positions.borrow_mut();
        positions.remember(path, anchor, unix_millis());
        self.queue_store(StorePatch::Positions(positions.clone()));
    }

    /// The parse for exactly `revision`, when it has landed.
    pub fn parsed(&self, doc: DocumentId, revision: u64) -> Option<Result<Arc<MdDoc>, String>> {
        self.parse
            .borrow()
            .entries
            .get(&doc)
            .filter(|parsed| parsed.revision == revision)
            .map(|parsed| parsed.result.clone())
    }

    /// The newest parse held for `doc`, even when it predates the buffer.
    ///
    /// This is the “last good model”: a reader keeps drawing it while the parse
    /// for a newer revision is still on a worker.
    pub fn latest_parsed(&self, doc: DocumentId) -> Option<ParsedDoc> {
        self.parse.borrow().entries.get(&doc).cloned()
    }

    /// Stores a parse result, keeping the newest revision per document.
    ///
    /// A late completion for an older revision never replaces a newer one, and
    /// a successful model is never replaced by a failure of the same revision.
    /// Returns whether the cache changed.
    pub fn store_parsed(&self, doc: DocumentId, revision: u64, result: Result<Arc<MdDoc>, String>) -> bool {
        let mut cache = self.parse.borrow_mut();
        if let Some(existing) = cache.entries.get(&doc) {
            if existing.revision > revision {
                return false;
            }
            if existing.revision == revision && existing.result.is_ok() && result.is_err() {
                return false;
            }
        }
        if cache.entries.len() >= PARSE_CACHE_DOCS && !cache.entries.contains_key(&doc) {
            // Keep the cache bounded by the documents that are still open.
            let docs = self.docs.borrow();
            cache.entries.retain(|id, _| docs.get(*id).is_some());
        }
        cache.entries.insert(doc, ParsedDoc { revision, result });
        true
    }

    /// Drops a closed document's parse.
    pub fn forget_parsed(&self, doc: DocumentId) {
        self.parse.borrow_mut().entries.remove(&doc);
    }

    /// The process-wide window registry.
    pub fn windows(&self) -> Ref<'_, WindowRegistry> {
        self.windows.borrow()
    }

    /// The store writer, when the state directory is writable.
    pub fn store(&self) -> Option<&StoreHandle> {
        self.store.as_ref()
    }

    /// Why persistence is unavailable, when it is.
    pub fn store_error(&self) -> Option<&str> {
        self.store_error.as_deref()
    }

    /// Keeps a state file that a late load found unreadable (the session).
    ///
    /// Must be called before the first patch of that file; the writer then
    /// never overwrites a newer version's or another application's file
    /// (OI-007).
    pub fn protect_store_files(&self, issues: &[StoreIssue]) {
        if let Some(store) = &self.store {
            store.protect_files(issues);
        }
    }

    /// Records a state issue found after [`Self::start`], for the startup
    /// notice and the writer's protection.
    pub fn note_store_issue(&self, issue: StoreIssue) {
        let mut startup = self.startup.borrow_mut();
        if !startup.store.contains(&issue) {
            startup.store.push(issue);
        }
    }

    /// The startup warnings, as localized sentences, for the first window
    /// that asks. Every later window gets an empty list.
    ///
    /// A portable root that fell back to XDG and every state file that was
    /// quarantined or left read-only are reported here, once (OI-007/OI-012).
    pub fn take_startup_notices(&self) -> Vec<String> {
        let mut startup = self.startup.borrow_mut();
        if startup.shown {
            return Vec::new();
        }
        startup.shown = true;
        let mut parts = Vec::with_capacity(startup.dirs.len() + startup.store.len());
        for issue in &startup.dirs {
            parts.push(dirs_issue_notice(issue));
        }
        for issue in &startup.store {
            parts.push(store_issue_notice(issue));
        }
        parts
    }

    /// Whether this process has not warned about `doc`'s size yet (OI-010).
    pub fn mark_large_warned(&self, doc: DocumentId) -> bool {
        self.large_warned.borrow_mut().insert(doc)
    }

    /// Every live window except `except` that publishes `doc`, for the E-12
    /// release of a clean duplicate view.
    pub fn windows_showing(&self, doc: DocumentId, except: WindowId) -> Vec<WindowId> {
        self.windows
            .borrow()
            .iter()
            .filter(|entry| entry.id != except && entry.has_document(doc))
            .map(|entry| entry.id)
            .collect()
    }

    /// The shared job hub.
    pub fn jobs(&self) -> &JobHub {
        &self.jobs
    }

    /// Installs one window's UI waker so cross-window messages, launches and
    /// watcher events can wake exactly the windows that need a frame.
    ///
    /// The launch and watcher inboxes live on other threads and cannot reach
    /// `AppShared`, so they get a composite waker: every live window's waker.
    pub fn attach_window_waker(&self, id: WindowId, waker: Waker) {
        self.wake_set.attach(id, waker);
        let composite = self.wake_set.composite();
        self.launch.set_waker(composite.clone());
        self.watch_inbox.set_waker(composite);
    }

    /// The launch queue shared with the single-instance listener thread.
    pub fn launch_inbox(&self) -> Arc<LaunchInbox> {
        Arc::clone(&self.launch)
    }

    /// Keeps the instance listener alive for the process lifetime.
    pub fn set_instance(&self, server: InstanceServer) {
        *self.instance.borrow_mut() = Some(server);
    }

    /// Wakes every window so the event loop drains mailboxes.
    pub fn wake(&self) {
        self.wake_set.wake_all();
    }

    /// The window that owns launch routing (`RunOptions`-style single router).
    pub fn router(&self) -> Option<WindowId> {
        self.router.get()
    }

    /// Whether `id` is the process's launch router.
    pub fn is_router(&self, id: WindowId) -> bool {
        self.router.get() == Some(id)
    }

    /// Allocates the next process-wide window id.
    pub fn allocate_window(&self) -> WindowId {
        let id = WindowId(self.next_window.get());
        self.next_window.set(self.next_window.get().saturating_add(1));
        id
    }

    /// Allocates a paste id for untitled synthetic content.
    pub fn allocate_paste(&self) -> PasteId {
        let id = PasteId(self.next_paste.get());
        self.next_paste.set(self.next_paste.get().saturating_add(1));
        id
    }

    /// Keeps a restored paste id from being handed out again.
    ///
    /// A session brings its own paste ids; a new paste after a restore must
    /// not take one and share its draft key.
    pub fn reserve_paste(&self, id: PasteId) {
        self.next_paste.set(self.next_paste.get().max(id.0.saturating_add(1)));
    }

    /// Allocates a tab-transfer token, unique for the process lifetime.
    pub fn allocate_transfer(&self) -> TransferToken {
        let token = TransferToken(self.next_transfer.get());
        self.next_transfer.set(self.next_transfer.get().saturating_add(1));
        token
    }

    /// Whether the app is quitting.
    pub fn is_quitting(&self) -> bool {
        self.quitting.get()
    }

    /// Records the user's quit intent; the next `poll` issues `Tx::quit`.
    pub fn set_quitting(&self) {
        self.quitting.set(true);
    }

    /// Adds a verified-open document to recents, newest first.
    ///
    /// Between `Recent` rows and caps this is [`RECENTS_MAX`] entries. The
    /// store patch is coalesced by the writer thread.
    pub fn record_recent(&self, path: PathBuf, at: u64) {
        let mut recents = self.recents.borrow_mut();
        recents.retain(|recent| recent.path != path);
        recents.insert(0, Recent::new(path, at));
        recents.truncate(RECENTS_MAX);
        self.queue_store(StorePatch::Recents(recents.clone()));
    }

    /// Starts following one file's folder. Safe to call repeatedly.
    pub fn watch_path(&self, path: &Path) {
        *self.watch_counts.borrow_mut().entry(path.to_path_buf()).or_insert(0) += 1;
        // Unit tests and headless tooling keep the counts (a transfer moves
        // one) but never start a watcher thread.
        if self.headless.get() || cfg!(test) {
            return;
        }
        self.ensure_watcher();
        self.sync_watch_files();
    }

    /// Stops following one file's folder; the watcher drops a folder no file
    /// of the window uses any more.
    pub fn unwatch_path(&self, path: &Path) {
        {
            let mut counts = self.watch_counts.borrow_mut();
            if let Some(count) = counts.get_mut(path) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    counts.remove(path);
                }
            }
        }
        if self.headless.get() || cfg!(test) {
            return;
        }
        self.sync_watch_files();
    }

    /// How many open views follow `path`; the transfer gate checks that a moved
    /// tab carries its watch token instead of duplicating or dropping it.
    #[cfg(test)]
    pub(crate) fn watch_refcount(&self, path: &Path) -> usize {
        self.watch_counts.borrow().get(path).copied().unwrap_or(0)
    }

    /// Why the watcher could not start, when it could not.
    pub fn watch_error(&self) -> Option<String> {
        self.watch_error.lock().unwrap_or_else(|error| error.into_inner()).clone()
    }

    /// The watcher setup error, once per process (OI-073).
    ///
    /// The error is recorded on the watcher thread, so the first frame that
    /// drains mailboxes may not see it yet; every later frame asks again, and
    /// only the first one that finds it shows the notice.
    pub fn take_watch_error(&self) -> Option<String> {
        let error = self.watch_error()?;
        (!self.watch_error_shown.replace(true)).then_some(error)
    }

    /// Records a watcher failure for the notice, for tests that do not start
    /// a real watcher thread.
    #[cfg(test)]
    pub(crate) fn record_watch_error(&self, error: &str) {
        *self.watch_error.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.to_owned());
    }

    fn ensure_watcher(&self) {
        if self.watcher.borrow().is_some() || self.watch_error().is_some() {
            return;
        }
        if let Some(service) = WatcherService::start(self.watch_inbox.clone(), self.watch_error.clone()) {
            *self.watcher.borrow_mut() = Some(service);
        }
    }

    fn sync_watch_files(&self) {
        if self.watcher.borrow().is_none() {
            return;
        }
        let files: Vec<PathBuf> = self.watch_counts.borrow().keys().cloned().collect();
        if let Some(watcher) = self.watcher.borrow().as_ref()
            && let Err(error) = watcher.set_files(files)
        {
            *self.watch_error.lock().unwrap_or_else(|error| error.into_inner()) = Some(error.to_string());
            // The next frame reports it (OI-073), even if nothing else changes.
            self.wake();
        }
    }

    /// Watcher events newer than `seq`, oldest first.
    pub fn file_events_since(&self, seq: u64) -> Vec<(u64, FileEvent)> {
        self.watch_inbox.since(seq)
    }

    /// Marks a document's draft as looked at. False when it already was.
    pub fn mark_draft_checked(&self, doc: DocumentId) -> bool {
        self.draft_checked.borrow_mut().insert(doc)
    }

    /// Moves a remembered position when its file is renamed.
    pub fn rename_position(&self, from: &Path, to: &Path) {
        let mut positions = self.positions.borrow_mut();
        positions.rename(from, to);
        let snapshot = positions.clone();
        drop(positions);
        self.queue_store(StorePatch::Positions(snapshot));
    }

    /// Moves a recent entry when its file is renamed, keeping its place.
    pub fn rename_recent(&self, from: &Path, to: &Path) {
        let mut recents = self.recents.borrow_mut();
        let mut changed = false;
        for recent in recents.iter_mut() {
            if recent.path == from {
                *recent = Recent::new(to.to_path_buf(), recent.opened_at);
                changed = true;
            }
        }
        if changed {
            let snapshot = recents.clone();
            drop(recents);
            self.queue_store(StorePatch::Recents(snapshot));
        }
    }

    /// Queues a store patch; a no-op when persistence is unavailable.
    pub fn queue_store(&self, patch: StorePatch) {
        if let Some(store) = &self.store {
            store.patch(patch);
        }
    }

    /// Forces pending state to disk on close/quit.
    pub fn flush_store(&self, timeout: Duration) -> bool {
        self.store.as_ref().is_some_and(|store| store.flush(timeout))
    }

    /// Stores one live window's restorable state. True when it changed.
    ///
    /// The fragments are composed into one [`SessionSnapshot`]; keeping them
    /// here means a window other than the one closing already has its latest
    /// tabs, active index and anchors when the session is written.
    pub fn publish_session_window(&self, id: WindowId, window: WindowSession) -> bool {
        let mut windows = self.session_windows.borrow_mut();
        if windows.get(&id) == Some(&window) {
            return false;
        }
        windows.insert(id, window);
        true
    }

    /// Forgets a window that closed while others stayed open.
    pub fn remove_session_window(&self, id: WindowId) -> bool {
        self.session_windows.borrow_mut().remove(&id).is_some()
    }

    /// The whole restorable session, in window-id order.
    pub fn session_snapshot(&self, clean: bool) -> SessionSnapshot {
        SessionSnapshot { clean, windows: self.session_windows.borrow().values().cloned().collect() }
    }

    /// Writes the session through the coalescing store writer.
    ///
    /// Headless tooling never writes one: a `--snap`/`--tree` run must not
    /// replace the user's session with the scene it rendered.
    pub fn queue_session(&self, clean: bool) {
        if self.headless.get() {
            return;
        }
        self.queue_store(StorePatch::Session(self.session_snapshot(clean)));
    }

    /// Changes why a window exists (a restored torn-off start page).
    pub fn set_window_kind(&self, id: WindowId, kind: WindowKind) -> bool {
        match self.windows.borrow_mut().get_mut(id) {
            Some(entry) if entry.kind != kind => {
                entry.kind = kind;
                true
            }
            _ => false,
        }
    }

    /// Registers a window's inbox for launch routing and tab transfer.
    ///
    /// The first window registered becomes the launch router for the process;
    /// [`Self::unregister_window`] promotes the next live window.
    pub fn register_window(&self, id: WindowId, key: String, inbox: Rc<UiMailbox>, title: &str, kind: WindowKind) {
        let entry = WindowEntry::new(id, key, inbox, title, kind);
        self.windows.borrow_mut().insert(entry);
        if self.router.get().is_none() {
            self.router.set(Some(id));
        }
    }

    /// Removes a closed window from the registry and forgets its waker and
    /// any in-flight claims, promoting a new router when this one is gone.
    pub fn unregister_window(&self, id: WindowId) {
        self.windows.borrow_mut().remove(id);
        self.wake_set.detach(id);
        self.claims.borrow_mut().retain(|_, window| *window != id);
        if self.router.get() == Some(id) {
            let next = self.windows.borrow().iter().next().map(|entry| entry.id);
            self.router.set(next);
            if let Some(next) = next {
                self.wake_set.wake(next);
            }
        }
    }

    /// Stores an open window's native and Rune handles, its kind and, where
    /// the platform reports it, its current placement.
    pub fn attach_window_handles(&self, id: WindowId, handles: WindowHandles, kind: WindowKind) {
        let placement = handles.native.placement();
        if let Some(entry) = self.windows.borrow_mut().get_mut(id) {
            entry.handles = Some(handles);
            entry.kind = kind;
            if placement.is_some() {
                entry.placement = placement;
            }
        }
    }

    /// Publishes what a window shows after `poll`. Returns whether the
    /// registry changed.
    ///
    /// The snapshot is a window's own view: title, the document in front,
    /// every document it shows and its tabs' cached dirty flags. Other windows
    /// read the union; no window has to hash every buffer each frame.
    pub fn publish_window(&self, id: WindowId, snapshot: WindowSnapshot) -> bool {
        let mut changed = false;
        {
            let mut windows = self.windows.borrow_mut();
            let Some(entry) = windows.get_mut(id) else { return false };
            if entry.title != snapshot.title {
                entry.title = snapshot.title;
                changed = true;
            }
            if entry.active_document != snapshot.active_document {
                entry.active_document = snapshot.active_document;
                changed = true;
            }
            if entry.documents != snapshot.documents {
                entry.documents = snapshot.documents.clone();
                changed = true;
            }
            if entry.dirty != snapshot.dirty {
                entry.dirty = snapshot.dirty;
                changed = true;
            }
            if entry.only_start != snapshot.only_start {
                entry.only_start = snapshot.only_start;
                changed = true;
            }
            if entry.prompt_open != snapshot.prompt_open {
                entry.prompt_open = snapshot.prompt_open;
                changed = true;
            }
        }
        // A document that has appeared in its target no longer needs a claim.
        self.claims.borrow_mut().retain(|doc, window| !(*window == id && snapshot.documents.contains(doc)));
        changed
    }

    /// Marks a window as closing (or reopens routing after a cancelled prompt).
    pub fn set_window_closing(&self, id: WindowId, closing: bool) -> bool {
        match self.windows.borrow_mut().get_mut(id) {
            Some(entry) if entry.closing != closing => {
                entry.closing = closing;
                true
            }
            _ => false,
        }
    }

    /// Moves a window to the front of the focus order and refreshes its cached
    /// placement for the next cascade.
    pub fn note_window_focused(&self, id: WindowId) {
        let stamp = self.next_focus.get().saturating_add(1);
        self.next_focus.set(stamp);
        if let Some(entry) = self.windows.borrow_mut().get_mut(id) {
            entry.focus_stamp = stamp;
        }
        self.refresh_placement(id);
    }

    /// Re-reads the native placement where the platform reports one (X11,
    /// Windows). Wayland returns `None`; the cache stays empty.
    pub fn refresh_placement(&self, id: WindowId) {
        let native = self.windows.borrow().get(id).and_then(|entry| entry.handles.as_ref()).map(|h| h.native.clone());
        let Some(native) = native else { return };
        let placement = native.placement();
        if let Some(placement) = placement
            && let Some(entry) = self.windows.borrow_mut().get_mut(id)
        {
            entry.placement = Some(placement);
        }
    }

    /// Stores a window's placement, as a native host or a session restore
    /// reports it.
    pub fn set_window_placement(&self, id: WindowId, placement: WindowPlacement) -> bool {
        match self.windows.borrow_mut().get_mut(id) {
            Some(entry) => {
                entry.placement = Some(placement);
                true
            }
            None => false,
        }
    }

    /// One window's cached placement, when the platform reports positions.
    pub fn window_placement(&self, id: WindowId) -> Option<WindowPlacement> {
        self.windows.borrow().get(id).and_then(|entry| entry.placement)
    }

    /// Re-reads every open window's placement from its native handle (X11).
    ///
    /// Called once on a cross-window drop, never per pointer move: each read is
    /// a native round-trip, and a drag must stay cheap.
    pub fn refresh_window_placements(&self) {
        let ids: Vec<WindowId> = self.windows.borrow().iter().map(|entry| entry.id).collect();
        for id in ids {
            self.refresh_placement(id);
        }
    }

    /// The open window under a physical desktop point, except `except`, from
    /// the cached placements. See [`Self::refresh_window_placements`].
    pub fn window_at(&self, point: (i32, i32), except: Option<WindowId>) -> Option<WindowId> {
        self.windows.borrow().window_at(point, except)
    }

    /// Where a new window from `origin` should open: 32 points down and right
    /// of the origin's frame. `None` on Wayland or before the first placement.
    ///
    /// The origin's placement is re-read first: a window manager may have moved
    /// it since the last focus event.
    pub fn cascade_position(&self, origin: WindowId) -> Option<(i32, i32)> {
        self.refresh_placement(origin);
        let placement = self.windows.borrow().get(origin).and_then(|entry| entry.placement)?;
        let step = (32.0 * placement.scale).round() as i32;
        Some((placement.position.0.saturating_add(step), placement.position.1.saturating_add(step)))
    }

    /// Whether any window reports `doc` dirty.
    pub fn document_is_dirty(&self, doc: DocumentId) -> bool {
        self.windows.borrow().iter().any(|entry| entry.dirty.contains(&doc))
    }

    /// The title of every document any window reports dirty, sorted and unique.
    pub fn dirty_document_titles(&self) -> Vec<String> {
        let dirty = self.windows.borrow().dirty_documents();
        let docs = self.docs.borrow();
        dirty
            .iter()
            .filter_map(|id| docs.get(*id))
            .map(|doc| doc.title.borrow().clone())
            .filter(|title| !title.trim().is_empty())
            .collect()
    }

    /// Every document a quit would take with it, across all windows, one entry
    /// per document.
    ///
    /// A document is included when its buffer differs from its saved base or
    /// when it is pasted text in no file yet (T-24 asks about pasted text even
    /// when unchanged). Titles and ids come from the registry, so two windows
    /// showing the same document produce one row; the save then happens once.
    pub fn close_documents(&self) -> Vec<crate::windows::CloseDoc> {
        let windows = self.windows.borrow();
        let docs = self.docs.borrow();
        let mut found: BTreeMap<DocumentId, crate::windows::CloseDoc> = BTreeMap::new();
        for entry in windows.iter() {
            for id in &entry.documents {
                if found.contains_key(id) {
                    continue;
                }
                let Some(document) = docs.get(*id) else { continue };
                let pasted = matches!(*document.key.borrow(), DocumentKey::Untitled(_));
                let dirty = document.is_dirty();
                if !pasted && !dirty {
                    continue;
                }
                found.insert(*id, crate::windows::CloseDoc { id: *id, title: close_title(&document), pasted, dirty });
            }
        }
        found.into_values().collect()
    }

    /// The window that is currently asking a decision question, if any.
    ///
    /// The quit guard uses this to refuse a second question: the user answers
    /// the one on screen first (T-25).
    pub fn window_with_prompt(&self, except: Option<WindowId>) -> Option<WindowId> {
        self.windows.borrow().window_with_prompt(except)
    }

    /// Installs the runtime window opener. The app entry point sets the real
    /// one; unit tests leave it unset or install a recorder.
    pub fn set_window_opener(&self, opener: WindowOpener) {
        self.opener.set(Some(opener));
    }

    /// Opens a window through the installed opener.
    ///
    /// UI thread only; the caller must not hold a registry or document borrow,
    /// because the opener builds and registers the next `WindowState`.
    pub fn request_new_window(self: Rc<Self>, request: WindowOpenRequest) -> Result<OpenedWindow, String> {
        let Some(opener) = self.opener.get() else {
            return Err(lmdv_i18n::t!("msg.noWindowHost").to_owned());
        };
        opener(&self, request)
    }

    /// Claims `doc` for `window` until its tab appears, so a burst of launches
    /// cannot open it twice or lose one.
    pub fn claim_document(&self, doc: DocumentId, window: WindowId) {
        self.claims.borrow_mut().insert(doc, window);
    }

    /// The live claim for `doc`, dropping a claim whose window has closed.
    pub fn document_claim(&self, doc: DocumentId) -> Option<WindowId> {
        let window = *self.claims.borrow().get(&doc)?;
        if self.windows.borrow().get(window).is_some_and(|entry| !entry.closing) {
            Some(window)
        } else {
            self.claims.borrow_mut().remove(&doc);
            None
        }
    }

    /// Routes one forwarded launch on the UI thread and performs the decision.
    ///
    /// Exactly one window (the router) calls this, so the shared launch queue
    /// is drained once. A launch that resolves to the router itself is
    /// reported as [`RouteAction::OpenHere`]; other windows get the launch in
    /// their own mailbox and are focused.
    pub fn route_launch(self: Rc<Self>, launch: Launch, origin: Option<WindowId>) -> RouteAction {
        let path = launch.path.as_ref().map(|path| lmdv_io::absolute_lexical(path));
        let document = path.map(|path| {
            let mut docs = self.docs_mut();
            docs.insert_loading(DocumentKey::File(path.clone()), crate::window::title_for_path(&path)).id
        });
        let claim = document.and_then(|doc| self.document_claim(doc));
        let query = RouteQuery { has_path: document.is_some(), document, claim, new_window: launch.new_window };
        let decision = self.windows().route(query);
        match decision {
            RouteDecision::New => {
                // A forwarded `--new-window` cascades from the window the user
                // last used; the router itself is not necessarily that window.
                let cascade_origin = self.windows().most_recent().or(origin);
                let position = cascade_origin.and_then(|id| self.cascade_position(id));
                let request = WindowOpenRequest { launch, kind: WindowKind::Normal, position };
                match Rc::clone(&self).request_new_window(request) {
                    Ok(opened) => {
                        if let Some(doc) = document {
                            self.claim_document(doc, opened.id);
                        }
                        RouteAction::Opened(opened.id)
                    }
                    Err(error) => {
                        // A document registered for a window that never opened
                        // has no view; collect it now rather than on the next
                        // window close.
                        self.docs_mut().collect_unused();
                        RouteAction::Failed(error)
                    }
                }
            }
            RouteDecision::Window { id, .. } => {
                if let Some(doc) = document {
                    self.claim_document(doc, id);
                }
                if origin == Some(id) {
                    self.focus_window(id);
                    return RouteAction::OpenHere;
                }
                if self.post_window(id, UiMsg::OpenLaunch(launch)) {
                    self.focus_window(id);
                    RouteAction::Posted(id)
                } else {
                    RouteAction::Failed(lmdv_i18n::t!("transfer.targetGone").to_owned())
                }
            }
        }
    }

    /// Brings a window forward natively where a host exists and records the
    /// focus order either way. Posts the window a [`UiMsg::FocusWindow`] so its
    /// loop settles the frame.
    pub fn focus_window(&self, id: WindowId) -> bool {
        let app = self.windows.borrow().get(id).and_then(|entry| entry.handles.as_ref()).map(|h| h.app.clone());
        let exists = self.windows.borrow().get(id).is_some();
        if !exists {
            return false;
        }
        if let Some(app) = app {
            app.focus();
        }
        self.note_window_focused(id);
        self.post_window(id, UiMsg::FocusWindow(id));
        true
    }

    /// Asks a window to close through its own close guard (T-24/T-25).
    pub fn close_window(&self, id: WindowId) -> bool {
        self.post_window(id, UiMsg::CloseWindow(id))
    }

    /// Posts a same-thread message to another window and wakes that window.
    pub fn post_window(&self, id: WindowId, msg: UiMsg) -> bool {
        self.try_post_window(id, msg).is_ok()
    }

    /// Posts a same-thread message to another window, handing it back when the
    /// window is gone. A transfer uses this so a vanished target loses no tab.
    pub fn try_post_window(&self, id: WindowId, msg: UiMsg) -> Result<(), UiMsg> {
        match self.windows.borrow().post(id, msg) {
            Ok(()) => {
                if !self.wake_set.wake(id) {
                    // The target has no host yet; wake everyone so the router's
                    // next turn can start it or settle.
                    self.wake_set.wake_all();
                }
                Ok(())
            }
            Err(msg) => Err(msg),
        }
    }
}

/// Launches forwarded from later `lmdv` processes.
///
/// The instance listener runs on its own thread, so it pushes here and wakes
/// the UI; a window's `poll` drains the queue on the UI thread.
#[derive(Default)]
pub struct LaunchInbox {
    queue: Mutex<VecDeque<LaunchRequest>>,
    waker: Mutex<Option<Waker>>,
}

impl LaunchInbox {
    /// Queues a forwarded launch and wakes the event loop.
    pub fn push(&self, request: LaunchRequest) {
        self.queue.lock().unwrap_or_else(|error| error.into_inner()).push_back(request);
        self.wake();
    }

    /// Takes every queued launch in arrival order.
    pub fn drain(&self) -> Vec<LaunchRequest> {
        self.queue.lock().unwrap_or_else(|error| error.into_inner()).drain(..).collect()
    }

    /// Installs the UI waker; the listener calls it after a push.
    pub fn set_waker(&self, waker: Waker) {
        *self.waker.lock().unwrap_or_else(|error| error.into_inner()) = Some(waker);
    }

    fn wake(&self) {
        let waker = self.waker.lock().unwrap_or_else(|error| error.into_inner()).clone();
        if let Some(waker) = waker {
            waker();
        }
    }
}

/// The watcher thread's handle. The `DocWatcher` lives on that thread, so
/// installing folder watches never blocks a handler (D7).
struct WatcherService {
    files: Sender<Vec<PathBuf>>,
}

impl WatcherService {
    fn start(inbox: Arc<FileEventInbox>, error: Arc<Mutex<Option<String>>>) -> Option<Self> {
        let (files, requests) = channel::<Vec<PathBuf>>();
        let thread_error = error.clone();
        let failed_wake = Arc::clone(&inbox);
        let spawned = thread::Builder::new().name("lmdv-watch".to_owned()).spawn(move || {
            let mut watcher = match DocWatcher::new(move |event| inbox.push(event)) {
                Ok(watcher) => watcher,
                Err(failure) => {
                    *thread_error.lock().unwrap_or_else(|error| error.into_inner()) = Some(failure.to_string());
                    // The failure is not a file event: wake the UI so the
                    // next poll can show its one notice (OI-073).
                    failed_wake.wake();
                    return;
                }
            };
            while let Ok(paths) = requests.recv() {
                if let Err(failure) = watcher.set_files(&paths) {
                    *thread_error.lock().unwrap_or_else(|error| error.into_inner()) = Some(failure.to_string());
                    failed_wake.wake();
                }
            }
        });
        match spawned {
            Ok(_) => Some(Self { files }),
            Err(failure) => {
                *error.lock().unwrap_or_else(|error| error.into_inner()) = Some(failure.to_string());
                None
            }
        }
    }

    fn set_files(&self, files: Vec<PathBuf>) -> Result<(), String> {
        self.files.send(files).map_err(|_| lmdv_i18n::t!("msg.watcherStopped").to_owned())
    }
}

/// File events from the watcher thread, each with a monotonic sequence so every
/// window can consume the stream independently.
#[derive(Default)]
pub struct FileEventInbox {
    events: Mutex<VecDeque<(u64, FileEvent)>>,
    next: AtomicU64,
    waker: Mutex<Option<Waker>>,
}

impl FileEventInbox {
    /// Queues an event and wakes the UI. The queue is bounded; the oldest
    /// events fall off first and a window that missed them reloads on its next
    /// event or focus.
    pub fn push(&self, event: FileEvent) {
        let seq = self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let mut events = self.events.lock().unwrap_or_else(|error| error.into_inner());
        events.push_back((seq, event));
        while events.len() > 256 {
            events.pop_front();
        }
        let waker = self.waker.lock().unwrap_or_else(|error| error.into_inner()).clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Events newer than `seq`.
    pub fn since(&self, seq: u64) -> Vec<(u64, FileEvent)> {
        let events = self.events.lock().unwrap_or_else(|error| error.into_inner());
        events.iter().filter(|(at, _)| *at > seq).cloned().collect()
    }

    /// Installs the UI waker.
    pub fn set_waker(&self, waker: Waker) {
        *self.waker.lock().unwrap_or_else(|error| error.into_inner()) = Some(waker);
    }

    /// Wakes the UI for a condition that is not a file event, such as the
    /// watcher failing to start (OI-073).
    pub fn wake(&self) {
        let waker = self.waker.lock().unwrap_or_else(|error| error.into_inner()).clone();
        if let Some(waker) = waker {
            waker();
        }
    }
}

/// A same-thread message to one window.
///
/// `Tab` is not `Send`, so the cross-window transfer (and every other message)
/// stays on the UI thread through [`UiMailbox`].
#[derive(Clone, Debug)]
pub enum UiMsg {
    /// Open a path in this window.
    Open(PathBuf),
    /// Open a forwarded launch, keeping its `--line` and `--new-window` intent.
    OpenLaunch(Launch),
    /// Bring an already-open document's tab forward.
    ActivateDocument(DocumentId),
    /// Preferences changed in another window.
    PrefsChanged,
    /// The tab-transfer protocol (`Hover`/`Adopt`/`Adopted`/`Returned`).
    Transfer(TransferMsg),
    /// Close this window's clean views of a document that another window is
    /// about to take over with Save as (E-12).
    ReleaseDocument { doc: DocumentId },
    /// Point this window's views of `from` at `to`, the one document for a
    /// file another window's load completed (OI-005).
    DocumentMerged { from: DocumentId, to: DocumentId },
    /// Focus the window (P4).
    FocusWindow(WindowId),
    /// Ask the window to close (P4).
    CloseWindow(WindowId),
}

/// One window's same-thread inbox. `Rc<RefCell<…>>`: never crosses threads;
/// jobs use [`crate::jobs::WorkerMailbox`] instead.
#[derive(Debug, Default)]
pub struct UiMailbox {
    queue: RefCell<VecDeque<UiMsg>>,
}

impl UiMailbox {
    /// An empty inbox.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a message; the next `poll` consumes it.
    pub fn post(&self, msg: UiMsg) {
        self.queue.borrow_mut().push_back(msg);
    }

    /// Takes every queued message in order.
    pub fn drain(&self) -> Vec<UiMsg> {
        let mut queue = self.queue.borrow_mut();
        queue.drain(..).collect()
    }

    /// Number of queued messages.
    pub fn len(&self) -> usize {
        self.queue.borrow().len()
    }

    /// Whether no message is queued.
    pub fn is_empty(&self) -> bool {
        self.queue.borrow().is_empty()
    }
}

/// The most documents whose parse is retained; older closed documents are
/// pruned first, and the registry bounds how many open documents there can be.
pub const PARSE_CACHE_DOCS: usize = 64;

/// One document's latest parse.
#[derive(Clone, Debug)]
pub struct ParsedDoc {
    /// The buffer revision the parse was made from.
    pub revision: u64,
    /// The model, or why it could not be produced.
    pub result: Result<Arc<MdDoc>, String>,
}

/// Parsed models keyed by document. One entry per document, newest revision.
#[derive(Default)]
pub struct ParseCache {
    entries: HashMap<DocumentId, ParsedDoc>,
}

/// Milliseconds since the Unix epoch, saturating at 0 before it.
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
}

/// The localized sentence for a folder-resolution fallback (OI-012).
fn dirs_issue_notice(issue: &DirsIssue) -> String {
    match issue {
        DirsIssue::PortableUnavailable { root, reason } => {
            lmdv_i18n::t!("msg.portableUnavailable", "root" => root.display(), "err" => reason)
        }
    }
}

/// The localized sentence for a state file that was ignored or quarantined
/// (OI-007).
fn store_issue_notice(issue: &StoreIssue) -> String {
    let file = state_file_name(issue.file());
    match issue {
        StoreIssue::FutureSchema { .. } => lmdv_i18n::t!("msg.stateNewer", "file" => file),
        StoreIssue::ForeignApp { .. } => lmdv_i18n::t!("msg.stateForeign", "file" => file),
        StoreIssue::Oversized { .. } => lmdv_i18n::t!("msg.stateOversized", "file" => file),
        StoreIssue::Corrupt { .. } => lmdv_i18n::t!("msg.stateCorrupt", "file" => file),
    }
}

/// The file name a state issue names in the notice.
fn state_file_name(file: StateFile) -> &'static str {
    match file {
        StateFile::Prefs => "prefs.json",
        StateFile::Recents => "recents.json",
        StateFile::Positions => "positions.json",
        StateFile::Ui => "ui.json",
        StateFile::Session => "session.json",
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::test_support;

    #[test]
    fn the_window_registry_posts_and_forgets() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let inbox = Rc::new(UiMailbox::new());
        let id = shared.allocate_window();
        shared.register_window(id, crate::windows::window_key(id), inbox.clone(), "lmdv", WindowKind::Normal);

        assert_eq!(shared.windows().len(), 1);
        assert!(shared.post_window(id, UiMsg::PrefsChanged));
        assert_eq!(inbox.len(), 1);
        assert!(!shared.post_window(WindowId(999), UiMsg::PrefsChanged));

        let published = WindowSnapshot {
            title: "notes — lmdv".to_owned(),
            active_document: Some(DocumentId(7)),
            documents: BTreeSet::from([DocumentId(7)]),
            dirty: BTreeSet::new(),
            only_start: false,
            prompt_open: false,
        };
        assert!(shared.publish_window(id, published.clone()));
        assert!(!shared.publish_window(id, published));
        assert_eq!(shared.windows().idle_window(), None);

        shared.unregister_window(id);
        assert!(shared.windows().is_empty());
    }

    #[test]
    fn publishing_the_document_clears_its_in_flight_claim() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let id = shared.allocate_window();
        shared.register_window(
            id,
            crate::windows::window_key(id),
            Rc::new(UiMailbox::new()),
            "one",
            WindowKind::Normal,
        );
        shared.claim_document(DocumentId(4), id);
        assert_eq!(shared.document_claim(DocumentId(4)), Some(id));

        let published = WindowSnapshot {
            title: "note.md — lmdv".to_owned(),
            active_document: Some(DocumentId(4)),
            documents: BTreeSet::from([DocumentId(4)]),
            dirty: BTreeSet::new(),
            only_start: false,
            prompt_open: false,
        };
        shared.publish_window(id, published);
        assert_eq!(shared.document_claim(DocumentId(4)), None);
    }

    #[test]
    fn a_start_page_window_is_the_idle_launch_target() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let first = shared.allocate_window();
        let second = shared.allocate_window();
        shared.register_window(
            first,
            crate::windows::window_key(first),
            Rc::new(UiMailbox::new()),
            "one",
            WindowKind::Normal,
        );
        shared.register_window(
            second,
            crate::windows::window_key(second),
            Rc::new(UiMailbox::new()),
            "two",
            WindowKind::Normal,
        );
        let published = WindowSnapshot {
            title: "one".to_owned(),
            active_document: Some(DocumentId(1)),
            documents: BTreeSet::from([DocumentId(1)]),
            dirty: BTreeSet::new(),
            only_start: false,
            prompt_open: false,
        };
        shared.publish_window(first, published);

        assert_eq!(shared.windows().idle_window(), Some(second));
    }

    #[test]
    fn recents_deduplicate_and_stay_capped() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        for index in 0..(RECENTS_MAX + 3) {
            shared.record_recent(PathBuf::from(format!("/tmp/note-{index}.md")), index as u64);
        }
        assert_eq!(shared.recents().len(), RECENTS_MAX);
        assert_eq!(shared.recents()[0].path, PathBuf::from(format!("/tmp/note-{}.md", RECENTS_MAX + 2)));

        shared.record_recent(PathBuf::from("/tmp/note-5.md"), 99);
        assert_eq!(shared.recents().len(), RECENTS_MAX, "reopening does not grow the list");
        assert_eq!(shared.recents()[0].path, PathBuf::from("/tmp/note-5.md"));
        assert_eq!(
            shared
                .recents()
                .iter()
                .filter(|recent| recent.path.as_path() == std::path::Path::new("/tmp/note-5.md"))
                .count(),
            1,
            "the old entry moved to the front instead of duplicating"
        );
    }

    #[test]
    fn the_watcher_service_forwards_events_from_its_own_thread() {
        use std::time::{Duration, Instant};

        let temp = TempDir::new().unwrap();
        let path = temp.path().join("watched.md");
        std::fs::write(&path, "one\n").unwrap();
        let inbox = Arc::new(FileEventInbox::default());
        let errors = Arc::new(Mutex::new(None));
        let service = WatcherService::start(inbox.clone(), errors.clone()).expect("the watcher thread");
        service.set_files(vec![path.clone()]).expect("the watch request");
        // Let the inotify watch settle before writing.
        std::thread::sleep(Duration::from_millis(150));
        std::fs::write(&path, "two\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut event = None;
        while Instant::now() < deadline && event.is_none() {
            event = inbox.since(0).into_iter().next();
            if event.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let Some((_, FileEvent::Changed { path: changed })) = event else {
            panic!("the watcher never reported: {:?}", errors.lock().unwrap());
        };
        assert_eq!(changed, path);
    }

    #[test]
    fn mailbox_messages_drain_in_order() {
        let inbox = UiMailbox::new();
        assert!(inbox.is_empty());
        inbox.post(UiMsg::Open(PathBuf::from("/tmp/a.md")));
        inbox.post(UiMsg::ActivateDocument(DocumentId(3)));
        assert_eq!(inbox.len(), 2);

        let drained = inbox.drain();
        assert_eq!(drained.len(), 2);
        assert!(matches!(&drained[0], UiMsg::Open(path) if path == &PathBuf::from("/tmp/a.md")));
        assert!(matches!(&drained[1], UiMsg::ActivateDocument(doc) if *doc == DocumentId(3)));
        assert!(inbox.is_empty());
    }

    #[test]
    fn the_portable_fallback_is_a_startup_notice_shown_once() {
        let temp = TempDir::new().unwrap();
        let issue = DirsIssue::PortableUnavailable {
            root: PathBuf::from("/media/usb/lmdv-data"),
            reason: "read-only file system".to_owned(),
        };
        let shared = AppShared::start_reported(test_support::dirs(&temp), vec![issue]);

        let notices = shared.take_startup_notices();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("/media/usb/lmdv-data"), "{}", notices[0]);
        assert!(notices[0].contains("read-only file system"), "{}", notices[0]);
        assert!(shared.take_startup_notices().is_empty(), "only the first window is told");
    }

    #[test]
    fn a_future_schema_state_file_is_reported_and_never_overwritten() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        std::fs::create_dir_all(&dirs.config).unwrap();
        let future = r#"{"schema": 99, "app": "lmdv", "data": {"theme": "dark"}}"#;
        std::fs::write(dirs.prefs_path(), future).unwrap();

        let shared = AppShared::start(dirs);
        let notices = shared.take_startup_notices();
        assert!(
            notices.iter().any(|notice| notice.contains("prefs.json") && notice.contains("newer version")),
            "the ignored file is named: {notices:?}"
        );

        // A settings change must not replace a file this version cannot read.
        let mut prefs = shared.prefs().clone();
        prefs.print_break_h1 = true;
        assert!(shared.set_prefs(prefs));
        assert!(shared.flush_store(Duration::from_secs(2)));
        assert_eq!(
            std::fs::read_to_string(shared.dirs().prefs_path()).unwrap(),
            future,
            "the newer version's file is untouched"
        );
    }

    fn spell_fixture() -> Availability {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/spell/system");
        Availability {
            files: lmdv_edit::spell::DictionaryFiles {
                language: "en_US".to_owned(),
                aff: root.join("en_US.aff"),
                dic: root.join("en_US.dic"),
            },
            fallback: false,
        }
    }

    #[test]
    fn spell_discovery_is_one_scan_and_a_stale_answer_is_dropped() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        assert_eq!(shared.spell_discovery(), SpellDiscovery::Unknown);
        assert!(shared.spell_availability().is_none());

        let generation = shared.begin_spell_discovery().expect("the first scan");
        assert!(shared.begin_spell_discovery().is_none(), "a second window joins the first scan");
        assert!(!shared.finish_spell_discovery(generation + 1, None), "a stale generation changes nothing");
        assert!(shared.finish_spell_discovery(generation, Some(spell_fixture())));
        assert_eq!(shared.spell_availability().map(|found| found.language().to_owned()).as_deref(), Some("en_US"));
        assert!(shared.begin_spell_discovery().is_none(), "the scan is done for this launch");
    }

    #[test]
    fn a_dictionary_that_cannot_load_hides_the_checker() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        shared.seed_spell(Some(spell_fixture()), None);
        let generation = shared.begin_spell_load().expect("the load");
        assert!(shared.begin_spell_load().is_none(), "one load at a time");
        assert!(shared.finish_spell_load(generation, None));
        assert!(shared.spell_availability().is_none(), "an unreadable dictionary hides the toggle");
        assert!(shared.spell_dictionary().is_none());
    }

    #[test]
    fn the_watcher_setup_error_is_shown_once() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        assert!(shared.take_watch_error().is_none());
        shared.record_watch_error("inotify watches exhausted");
        assert_eq!(shared.take_watch_error().as_deref(), Some("inotify watches exhausted"));
        assert!(shared.take_watch_error().is_none(), "OI-073: not every frame");
    }
}
