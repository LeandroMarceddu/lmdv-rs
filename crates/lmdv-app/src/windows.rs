//! The process-wide window registry, launch routing and the runtime window
//! opener.
//!
//! A window is registered by [`crate::window::WindowState`] before its host
//! starts, published after every `poll`, and removed when its state is dropped.
//! Launches arrive on the single-instance thread, wait in [`crate::shared::LaunchInbox`],
//! and are routed exactly once by the process's router window — never by each
//! window in turn.
//!
//! Routing is the ported original table (`docs/LINUX_INTEGRATION.md` §5.6):
//! `--new-window` → the window already showing the document → an in-flight
//! claim → the first idle normal window → the most recently focused window →
//! a new window. A launch without a file fronts the most recently focused
//! window, or opens one.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use lmdv_core::{Document, DocumentId, WindowId};
use lmdv_io::Launch;
use rune::{AppHandle, Waker, WindowHandle, WindowPlacement};

use crate::shared::{AppShared, UiMailbox};
use crate::window::WindowState;

/// Why a window exists. A torn-off start page is not reusable as the idle
/// target for a launch (the original's rule, `docs/ARCHITECTURE_DETAIL.md` §7.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowKind {
    /// An ordinary window opened by the user or a launch.
    #[default]
    Normal,
    /// A window created by tearing out a start page (P4-02).
    TornOffStart,
}

impl WindowKind {
    /// Whether the original lets a launch reuse this window's lone start page.
    pub fn is_reusable(self) -> bool {
        matches!(self, Self::Normal)
    }
}

/// The native and Rune handles of one open window.
///
/// Both are cheap clones of shared references; holding them does not keep the
/// window's state alive and never forms a cycle with [`AppShared`].
#[derive(Clone)]
pub struct WindowHandles {
    /// Feeds and focuses the window from another window's handler.
    pub app: AppHandle<WindowState>,
    /// Where the window is on the desktop, whether it is shown, and its key.
    pub native: WindowHandle,
}

impl std::fmt::Debug for WindowHandles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowHandles").field("key", &self.native.key()).finish_non_exhaustive()
    }
}

impl WindowHandles {
    /// Shows and focuses this window with the compositor's activation token
    /// (OI-117). On Wayland a plain `focus()` may be refused: the compositor
    /// only lets a window take focus when a token from the launching client is
    /// passed to `WindowHandle::activate`. An empty token is ignored by Rune.
    pub fn activate(&self, token: &str) {
        self.native.activate(token);
    }

    /// Moves this native window where the platform allows it (X11); Wayland
    /// keeps compositor placement (the live tear-out follow in [`crate::tear_out`]).
    pub fn set_position(&self, position: (i32, i32)) {
        self.native.set_position(position);
    }
}

/// One window known to the process.
pub struct WindowEntry {
    /// The window identity.
    pub id: WindowId,
    /// The window's unique key (`window-{id}`), used by menus and the session.
    pub key: String,
    /// The last published window title.
    pub title: String,
    /// The window's same-thread inbox.
    pub inbox: Rc<UiMailbox>,
    /// The document in front, for launch routing.
    pub active_document: Option<DocumentId>,
    /// Every document the window shows, in no particular order.
    pub documents: BTreeSet<DocumentId>,
    /// The window shows only start pages (the original's "idle" window).
    pub only_start: bool,
    /// Why the window exists.
    pub kind: WindowKind,
    /// A close is in progress (or its dirty prompt is open): never route here.
    pub closing: bool,
    /// A decision dialog is open here, so another window must not ask a second
    /// question (T-25's one-question-at-a-time rule across the process).
    pub prompt_open: bool,
    /// Monotonic focus stamp; larger is more recent. Zero means never focused.
    pub focus_stamp: u64,
    /// The last placement read from the native window, where the platform has one.
    pub placement: Option<WindowPlacement>,
    /// Documents this window sees as dirty (its own tabs' cached flags).
    pub dirty: BTreeSet<DocumentId>,
    /// Present once the native host has been created.
    pub handles: Option<WindowHandles>,
}

impl WindowEntry {
    /// An entry for a window that has not started yet.
    pub fn new(id: WindowId, key: String, inbox: Rc<UiMailbox>, title: &str, kind: WindowKind) -> Self {
        Self {
            id,
            key,
            title: title.to_owned(),
            inbox,
            active_document: None,
            documents: BTreeSet::new(),
            only_start: true,
            kind,
            closing: false,
            prompt_open: false,
            focus_stamp: 0,
            placement: None,
            dirty: BTreeSet::new(),
            handles: None,
        }
    }

    /// Whether this window shows `doc` in a tab.
    pub fn has_document(&self, doc: DocumentId) -> bool {
        self.documents.contains(&doc)
    }

    /// Whether a physical desktop point lies inside this window's cached frame
    /// (X11 only; a window without a placement never contains one).
    pub fn contains_screen_point(&self, point: (i32, i32)) -> bool {
        self.placement.is_some_and(|placement| crate::tear_out::placement_contains(placement, point))
    }
}

impl std::fmt::Debug for WindowEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowEntry")
            .field("id", &self.id)
            .field("key", &self.key)
            .field("title", &self.title)
            .field("active_document", &self.active_document)
            .field("documents", &self.documents)
            .field("only_start", &self.only_start)
            .field("kind", &self.kind)
            .field("closing", &self.closing)
            .field("prompt_open", &self.prompt_open)
            .field("focus_stamp", &self.focus_stamp)
            .field("dirty", &self.dirty)
            .field("handles", &self.handles.is_some())
            .finish()
    }
}

/// What a window publishes to the registry after a `poll`.
#[derive(Clone, Debug)]
pub struct WindowSnapshot {
    pub title: String,
    pub active_document: Option<DocumentId>,
    pub documents: BTreeSet<DocumentId>,
    pub dirty: BTreeSet<DocumentId>,
    pub only_start: bool,
    /// This window is showing a decision dialog right now.
    pub prompt_open: bool,
}

/// One document a close or quit question has to name.
///
/// The union across windows is deduplicated by [`DocumentId`]: a document shown
/// in two windows is one question and one save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseDoc {
    /// The canonical document.
    pub id: DocumentId,
    /// The document's title, as the dialog names it.
    pub title: String,
    /// Pasted text in no file yet (T-24 asks about it even when unchanged).
    pub pasted: bool,
    /// The buffer differs from its saved base.
    pub dirty: bool,
}

/// The routing question: one launch's facts, already resolved against the
/// document registry and the in-flight claims.
#[derive(Clone, Copy, Debug, Default)]
pub struct RouteQuery {
    /// The launch names a file.
    pub has_path: bool,
    /// The canonical document for that path, when it already exists.
    pub document: Option<DocumentId>,
    /// The window holding the in-flight claim for that document.
    pub claim: Option<WindowId>,
    /// `--new-window` ignores every existing window.
    pub new_window: bool,
}

/// Why a window was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteReason {
    /// A window already shows the document.
    ExistingDocument,
    /// The document is in flight to this window.
    Claim,
    /// The first idle normal window.
    IdleWindow,
    /// The most recently focused open window.
    MostRecent,
}

/// Where one launch goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteDecision {
    /// No existing window is a fit; open a new one.
    New,
    /// Send the launch to this window.
    Window { id: WindowId, reason: RouteReason },
}

/// What routing did, for the router window and for tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteAction {
    /// The router is also the target: open it in the router's own state.
    OpenHere,
    /// The launch was posted to another window (and that window was focused).
    Posted(WindowId),
    /// A new window was opened for the launch.
    Opened(WindowId),
    /// Nothing could take the launch.
    Failed(String),
}

/// A request to open a window at runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowOpenRequest {
    /// The launch to apply once the window exists.
    pub launch: Launch,
    /// Why the window exists.
    pub kind: WindowKind,
    /// Cascade position in physical pixels, where the platform has one.
    pub position: Option<(i32, i32)>,
}

/// The first frame's facts a new window inherits from its source (S-13/T-31).
///
/// The position also goes into the [`WindowOpenRequest`]; the shared opener
/// reads this plan for the maximized flag, which no request field carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WindowPlan {
    /// The cascade position in physical pixels, where the platform has one.
    pub(crate) position: Option<(i32, i32)>,
    /// A maximized source yields a maximized child (S-13).
    pub(crate) maximized: bool,
}

/// A window the opener started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenedWindow {
    /// The process-wide id the new window registered with.
    pub id: WindowId,
}

/// Opens a native window. A function pointer, so the default host installs no
/// captured state and no reference cycle can form; tests install a recorder.
pub type WindowOpener = fn(&Rc<AppShared>, WindowOpenRequest) -> Result<OpenedWindow, String>;

/// Every live window, ordered by id.
#[derive(Debug, Default)]
pub struct WindowRegistry {
    entries: BTreeMap<WindowId, WindowEntry>,
}

impl WindowRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces an entry.
    pub fn insert(&mut self, entry: WindowEntry) {
        self.entries.insert(entry.id, entry);
    }

    /// Removes a window, returning its entry.
    pub fn remove(&mut self, id: WindowId) -> Option<WindowEntry> {
        self.entries.remove(&id)
    }

    /// Looks a window up by id.
    pub fn get(&self, id: WindowId) -> Option<&WindowEntry> {
        self.entries.get(&id)
    }

    /// Mutable access for title, document and focus updates.
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut WindowEntry> {
        self.entries.get_mut(&id)
    }

    /// Looks a window up by its stable key.
    pub fn by_key(&self, key: &str) -> Option<&WindowEntry> {
        self.entries.values().find(|entry| entry.key == key)
    }

    /// Posts to one window's inbox. The message comes back when no such
    /// window exists, so a transfer can restore its tab instead of losing it.
    pub fn post(&self, id: WindowId, msg: crate::shared::UiMsg) -> Result<(), crate::shared::UiMsg> {
        match self.entries.get(&id) {
            Some(entry) => {
                entry.inbox.post(msg);
                Ok(())
            }
            None => Err(msg),
        }
    }

    /// Number of live windows.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no window is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every window in id order.
    pub fn iter(&self) -> impl Iterator<Item = &WindowEntry> {
        self.entries.values()
    }

    /// The focus list: most recently focused first, then never-focused windows
    /// from the lowest id. Ties are deterministic.
    pub fn focus_order(&self) -> Vec<WindowId> {
        let mut entries: Vec<&WindowEntry> = self.entries.values().collect();
        entries.sort_by(|a, b| b.focus_stamp.cmp(&a.focus_stamp).then_with(|| a.id.0.cmp(&b.id.0)));
        entries.into_iter().map(|entry| entry.id).collect()
    }

    /// Whether the window exists and is still a routing target.
    fn is_open(&self, id: WindowId) -> bool {
        self.entries.get(&id).is_some_and(|entry| !entry.closing)
    }

    /// The most recently focused open window, never-focused ones last.
    pub fn most_recent(&self) -> Option<WindowId> {
        self.focus_order().into_iter().find(|id| self.is_open(*id))
    }

    /// The first idle normal window in focus order.
    pub fn idle_window(&self) -> Option<WindowId> {
        self.focus_order().into_iter().find(|id| {
            self.entries.get(id).is_some_and(|entry| !entry.closing && entry.only_start && entry.kind.is_reusable())
        })
    }

    /// The first open window in focus order that shows `doc`.
    pub fn window_showing(&self, doc: DocumentId) -> Option<WindowId> {
        self.focus_order()
            .into_iter()
            .find(|id| self.entries.get(id).is_some_and(|entry| !entry.closing && entry.has_document(doc)))
    }

    /// The first open window in focus order whose cached frame contains a
    /// physical desktop point, never `except` (a tab's source window).
    ///
    /// The focus order is the stacking order for a drag's purpose: the window
    /// the user last worked in wins when two overlap. A closing window never
    /// accepts a tab (T-25).
    pub fn window_at(&self, point: (i32, i32), except: Option<WindowId>) -> Option<WindowId> {
        self.focus_order().into_iter().find(|id| {
            Some(*id) != except
                && self.entries.get(id).is_some_and(|entry| !entry.closing && entry.contains_screen_point(point))
        })
    }

    /// The first window in focus order with a decision dialog open, other than
    /// `except`. A closing window whose close question is open still counts.
    pub fn window_with_prompt(&self, except: Option<WindowId>) -> Option<WindowId> {
        self.focus_order()
            .into_iter()
            .find(|id| Some(*id) != except && self.entries.get(id).is_some_and(|entry| entry.prompt_open))
    }

    /// Decides where a launch goes. Pure: callers resolve the document id and
    /// the claim before asking, and perform the decision afterwards.
    pub fn route(&self, query: RouteQuery) -> RouteDecision {
        if query.new_window {
            return RouteDecision::New;
        }
        if !query.has_path {
            return match self.most_recent() {
                Some(id) => RouteDecision::Window { id, reason: RouteReason::MostRecent },
                None => RouteDecision::New,
            };
        }
        if let Some(document) = query.document
            && let Some(id) = self.window_showing(document)
        {
            return RouteDecision::Window { id, reason: RouteReason::ExistingDocument };
        }
        if let Some(id) = query.claim.filter(|id| self.is_open(*id)) {
            return RouteDecision::Window { id, reason: RouteReason::Claim };
        }
        if let Some(id) = self.idle_window() {
            return RouteDecision::Window { id, reason: RouteReason::IdleWindow };
        }
        if let Some(id) = self.most_recent() {
            return RouteDecision::Window { id, reason: RouteReason::MostRecent };
        }
        RouteDecision::New
    }

    /// Every document any window reports as dirty.
    pub fn dirty_documents(&self) -> BTreeSet<DocumentId> {
        let mut dirty = BTreeSet::new();
        for entry in self.entries.values() {
            dirty.extend(entry.dirty.iter().copied());
        }
        dirty
    }
}

/// The UI wakers of every live window.
///
/// Each window registers the waker `AppSpec::setup` hands it. A message to one
/// window wakes exactly that window; launches and watcher events wake all of
/// them. Cloning the set snapshot before calling avoids holding a lock across
/// a waker that may re-enter the loop.
#[derive(Default)]
pub struct WakeSet {
    wakers: RefCell<BTreeMap<WindowId, Waker>>,
    all: Arc<Mutex<Vec<Waker>>>,
}

impl std::fmt::Debug for WakeSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WakeSet").field("windows", &self.wakers.borrow().len()).finish()
    }
}

impl WakeSet {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces one window's waker.
    pub fn attach(&self, id: WindowId, waker: Waker) {
        self.wakers.borrow_mut().insert(id, waker);
        self.rebuild();
    }

    /// Forgets a closed window's waker.
    pub fn detach(&self, id: WindowId) {
        self.wakers.borrow_mut().remove(&id);
        self.rebuild();
    }

    /// Wakes one window. False when it has no waker yet (its host has not
    /// started), so the caller can fall back to waking every window.
    pub fn wake(&self, id: WindowId) -> bool {
        let waker = self.wakers.borrow().get(&id).cloned();
        match waker {
            Some(waker) => {
                waker();
                true
            }
            None => false,
        }
    }

    /// Wakes every registered window.
    pub fn wake_all(&self) {
        let wakers = self.snapshot();
        for waker in wakers {
            waker();
        }
    }

    /// Wakes every registered window except `except`.
    ///
    /// A publish that changed a document's dirty flag or title is visible to
    /// the other windows (they read the registry), so they must draw another
    /// frame; the window that published has already drawn its own (OI-074).
    pub fn wake_except(&self, except: WindowId) {
        let wakers: Vec<Waker> =
            self.wakers.borrow().iter().filter(|(id, _)| **id != except).map(|(_, waker)| waker.clone()).collect();
        for waker in wakers {
            waker();
        }
    }

    /// A waker that wakes every window, for the launch and watcher inboxes,
    /// whose threads cannot reach [`AppShared`].
    pub fn composite(&self) -> Waker {
        let all = Arc::clone(&self.all);
        Arc::new(move || {
            let wakers = all.lock().unwrap_or_else(|error| error.into_inner()).clone();
            for waker in wakers {
                waker();
            }
        })
    }

    /// Number of windows with a waker.
    pub fn len(&self) -> usize {
        self.wakers.borrow().len()
    }

    /// Whether no window has a waker yet.
    pub fn is_empty(&self) -> bool {
        self.wakers.borrow().is_empty()
    }

    fn snapshot(&self) -> Vec<Waker> {
        self.wakers.borrow().values().cloned().collect()
    }

    fn rebuild(&self) {
        let wakers = self.snapshot();
        *self.all.lock().unwrap_or_else(|error| error.into_inner()) = wakers;
    }
}

/// The stable registry key for a window id.
pub fn window_key(id: WindowId) -> String {
    format!("window-{}", id.0)
}

/// The name a close/quit dialog uses for a document: its title, or
/// “Untitled” while a pasted document has none.
pub fn close_title(document: &Document) -> String {
    let title = document.title.borrow().clone();
    if title.trim().is_empty() { lmdv_i18n::t!("tab.untitled").to_owned() } else { title }
}

/// Opens a native document window at runtime on the UI thread.
///
/// This is the production [`WindowOpener`], installed by
/// [`crate::run_with_dirs`]. It builds the whole [`WindowState`] first, with no
/// live `RefCell` borrow held, then hands the spec to `rune::open_window` so
/// the window keeps the app alive and is destroyed when closed — unlike
/// `App::window`'s secondary windows, which hide instead.
///
/// UI thread only (a handler or `poll`). The caller must not hold a borrow of
/// the registry, the document registry or a launch queue when it calls this.
pub fn open_runtime_window(shared: &Rc<AppShared>, request: WindowOpenRequest) -> Result<OpenedWindow, String> {
    if rune::is_headless() {
        return Err(lmdv_i18n::t!("msg.headlessWindows").to_owned());
    }
    let id = shared.allocate_window();
    // S-13/T-31: every runtime window the user causes is the next cascade's
    // anchor for three seconds, not only Ctrl+Shift+N.
    shared.note_window_created(id, shared.now());
    let mut state = WindowState::new_keyed(Rc::clone(shared), id, window_key(id));
    state.open_launch(&request.launch);
    state.sync_registry();
    let _ = state.publish_session();
    // S-13/T-31: a maximized source's child opens maximized too.
    let plan = shared.take_window_plan();
    let frame = crate::app::WindowFrame {
        position: request.position,
        maximized: plan.maximized,
        ..crate::app::WindowFrame::default()
    };
    let spec = crate::app::window_spec(window_key(id), state, frame, request.kind);
    let app = spec.app.handle();
    let native = rune::open_window(spec);
    shared.attach_window_handles(id, WindowHandles { app, native }, request.kind);
    Ok(OpenedWindow { id })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tempfile::TempDir;

    use super::*;
    use crate::shared::UiMsg;
    use crate::test_support;
    use lmdv_core::DocumentKey;
    use rune::Tx;

    fn entry(id: u64, documents: &[u64]) -> WindowEntry {
        let id = WindowId(id);
        let mut entry = WindowEntry::new(id, window_key(id), Rc::new(UiMailbox::new()), "lmdv", WindowKind::Normal);
        entry.documents = documents.iter().copied().map(DocumentId).collect();
        entry.only_start = entry.documents.is_empty();
        entry
    }

    fn registry(entries: Vec<WindowEntry>) -> WindowRegistry {
        let mut registry = WindowRegistry::new();
        for entry in entries {
            registry.insert(entry);
        }
        registry
    }

    #[test]
    fn a_new_window_request_ignores_every_existing_window() {
        let registry = registry(vec![entry(1, &[]), entry(2, &[])]);
        assert_eq!(registry.route(RouteQuery { new_window: true, ..RouteQuery::default() }), RouteDecision::New);
    }

    #[test]
    fn an_open_document_selects_the_most_recently_focused_holder() {
        let mut registry = registry(vec![entry(1, &[7]), entry(2, &[7]), entry(3, &[])]);
        registry.get_mut(WindowId(1)).unwrap().focus_stamp = 2;
        registry.get_mut(WindowId(2)).unwrap().focus_stamp = 5;

        let decision =
            registry.route(RouteQuery { has_path: true, document: Some(DocumentId(7)), ..RouteQuery::default() });
        assert_eq!(decision, RouteDecision::Window { id: WindowId(2), reason: RouteReason::ExistingDocument });
    }

    #[test]
    fn an_in_flight_claim_beats_an_idle_window() {
        let registry = registry(vec![entry(1, &[]), entry(2, &[])]);
        let decision = registry.route(RouteQuery {
            has_path: true,
            document: Some(DocumentId(9)),
            claim: Some(WindowId(2)),
            new_window: false,
        });
        assert_eq!(decision, RouteDecision::Window { id: WindowId(2), reason: RouteReason::Claim });
    }

    #[test]
    fn a_file_prefers_the_first_idle_normal_window_in_focus_order() {
        let mut registry = registry(vec![entry(1, &[3]), entry(2, &[]), entry(3, &[])]);
        registry.get_mut(WindowId(1)).unwrap().focus_stamp = 9;
        registry.get_mut(WindowId(3)).unwrap().focus_stamp = 1;

        let decision =
            registry.route(RouteQuery { has_path: true, document: Some(DocumentId(4)), ..RouteQuery::default() });
        assert_eq!(decision, RouteDecision::Window { id: WindowId(3), reason: RouteReason::IdleWindow });
    }

    #[test]
    fn a_torn_off_start_page_is_not_the_idle_target() {
        let mut registry = registry(vec![entry(1, &[3]), entry(2, &[])]);
        registry.get_mut(WindowId(2)).unwrap().kind = WindowKind::TornOffStart;
        registry.get_mut(WindowId(1)).unwrap().focus_stamp = 2;

        let decision =
            registry.route(RouteQuery { has_path: true, document: Some(DocumentId(4)), ..RouteQuery::default() });
        assert_eq!(decision, RouteDecision::Window { id: WindowId(1), reason: RouteReason::MostRecent });
    }

    #[test]
    fn without_an_idle_window_a_file_goes_to_the_most_recent_window() {
        let mut registry = registry(vec![entry(1, &[3]), entry(2, &[5])]);
        registry.get_mut(WindowId(1)).unwrap().focus_stamp = 1;
        registry.get_mut(WindowId(2)).unwrap().focus_stamp = 4;

        let decision =
            registry.route(RouteQuery { has_path: true, document: Some(DocumentId(6)), ..RouteQuery::default() });
        assert_eq!(decision, RouteDecision::Window { id: WindowId(2), reason: RouteReason::MostRecent });
    }

    #[test]
    fn a_closing_window_is_skipped() {
        let mut registry = registry(vec![entry(1, &[]), entry(2, &[])]);
        registry.get_mut(WindowId(1)).unwrap().focus_stamp = 9;
        registry.get_mut(WindowId(1)).unwrap().closing = true;

        let decision =
            registry.route(RouteQuery { has_path: true, document: Some(DocumentId(1)), ..RouteQuery::default() });
        assert_eq!(decision, RouteDecision::Window { id: WindowId(2), reason: RouteReason::IdleWindow });
    }

    #[test]
    fn a_file_with_no_windows_opens_a_new_one() {
        let registry = registry(Vec::new());
        assert_eq!(
            registry.route(RouteQuery { has_path: true, document: Some(DocumentId(1)), ..RouteQuery::default() }),
            RouteDecision::New
        );
    }

    #[test]
    fn a_launch_without_a_file_fronts_the_most_recent_window_or_makes_one() {
        let mut registry = registry(vec![entry(1, &[2]), entry(2, &[3])]);
        registry.get_mut(WindowId(1)).unwrap().focus_stamp = 8;
        registry.get_mut(WindowId(2)).unwrap().focus_stamp = 2;

        assert_eq!(
            registry.route(RouteQuery::default()),
            RouteDecision::Window { id: WindowId(1), reason: RouteReason::MostRecent }
        );
        assert_eq!(WindowRegistry::new().route(RouteQuery::default()), RouteDecision::New);
    }

    #[test]
    fn registration_and_focus_order_are_deterministic() {
        let mut registry = registry(vec![entry(1, &[]), entry(2, &[]), entry(3, &[])]);
        assert_eq!(registry.focus_order(), vec![WindowId(1), WindowId(2), WindowId(3)], "never focused, by id");

        registry.get_mut(WindowId(2)).unwrap().focus_stamp = 4;
        registry.get_mut(WindowId(3)).unwrap().focus_stamp = 7;
        assert_eq!(registry.focus_order(), vec![WindowId(3), WindowId(2), WindowId(1)]);
        assert_eq!(registry.most_recent(), Some(WindowId(3)));
    }

    #[test]
    fn the_registry_finds_windows_by_key_and_document() {
        let mut registry = registry(vec![entry(1, &[4]), entry(2, &[])]);
        registry.get_mut(WindowId(1)).unwrap().focus_stamp = 2;
        registry.get_mut(WindowId(2)).unwrap().focus_stamp = 6;

        assert_eq!(registry.by_key("window-2").map(|entry| entry.id), Some(WindowId(2)));
        assert_eq!(registry.window_showing(DocumentId(4)), Some(WindowId(1)));
        assert_eq!(registry.window_showing(DocumentId(99)), None);
    }

    #[test]
    fn the_wake_set_wakes_one_or_every_window() {
        let set = WakeSet::new();
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let first_count = first.clone();
        let second_count = second.clone();

        set.attach(
            WindowId(1),
            Arc::new(move || {
                first_count.fetch_add(1, Ordering::SeqCst);
            }),
        );
        set.attach(
            WindowId(2),
            Arc::new(move || {
                second_count.fetch_add(1, Ordering::SeqCst);
            }),
        );
        assert_eq!(set.len(), 2);

        assert!(set.wake(WindowId(2)));
        assert_eq!((first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)), (0, 1));
        assert!(!set.wake(WindowId(9)), "a window without a host falls back to waking all");

        set.wake_all();
        assert_eq!((first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)), (1, 2));

        let composite = set.composite();
        composite();
        assert_eq!((first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)), (2, 3));

        set.detach(WindowId(1));
        set.wake_all();
        assert_eq!((first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)), (2, 4));
    }

    #[test]
    fn waking_except_one_window_leaves_the_publisher_asleep() {
        let set = WakeSet::new();
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let first_count = first.clone();
        let second_count = second.clone();
        set.attach(
            WindowId(1),
            Arc::new(move || {
                first_count.fetch_add(1, Ordering::SeqCst);
            }),
        );
        set.attach(
            WindowId(2),
            Arc::new(move || {
                second_count.fetch_add(1, Ordering::SeqCst);
            }),
        );

        set.wake_except(WindowId(1));
        assert_eq!(first.load(Ordering::SeqCst), 0);
        assert_eq!(second.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_cold_start_burst_queues_ten_launches_in_one_window_with_its_own_file_first() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let write = |name: &str| {
            let path = temp.path().join(name);
            std::fs::write(&path, "# one\n").unwrap();
            path
        };
        let own = write("own.md");
        // The single-instance thread queues ten forwards before the first page
        // listens; two name a file already open (one is the window's own), so
        // the burst must suppress both duplicates.
        let forwarded: Vec<std::path::PathBuf> =
            ["f01.md", "own.md", "f02.md", "f03.md", "f04.md", "f05.md", "f03.md", "f06.md", "f07.md", "f08.md"]
                .iter()
                .map(|name| write(name))
                .collect();

        let mut state =
            WindowState::start(Rc::clone(&shared), &Launch { path: Some(own.clone()), ..Launch::default() });
        for (index, path) in forwarded.iter().enumerate() {
            let launch = Launch { path: Some(path.clone()), line: Some(index + 1), ..Launch::default() };
            shared.launch_inbox().push(lmdv_io::instance::LaunchRequest::new(launch));
        }
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);

        assert_eq!(shared.windows().len(), 1, "the burst stays in the one window it started");
        let titles: Vec<String> = state.tabs().iter().map(crate::window::Tab::full_title).collect();
        assert_eq!(
            titles,
            ["own.md", "f01.md", "f02.md", "f03.md", "f04.md", "f05.md", "f06.md", "f07.md", "f08.md"],
            "the window's own launch is first and the forwards follow in arrival order"
        );
        let mut unique = titles.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), titles.len(), "no forwarded launch opened a second tab for a path");
    }

    #[test]
    fn a_new_window_cascades_32_points_from_the_focused_window() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let id = shared.allocate_window();
        shared.register_window(id, window_key(id), Rc::new(UiMailbox::new()), "one", WindowKind::Normal);
        assert_eq!(shared.cascade_position(id), None, "no placement yet: the compositor decides");

        let placement = WindowPlacement { position: (100, 200), scale: 2.0, ..WindowPlacement::default() };
        assert!(shared.set_window_placement(id, placement));
        assert_eq!(shared.cascade_position(id), Some((164, 264)), "32 points at scale 2, in physical pixels");
        assert_eq!(shared.cascade_position(WindowId(99)), None);
    }

    fn placed(id: WindowId, x: i32, y: i32, display: ((i32, i32), (u32, u32))) -> (WindowId, WindowPlacement) {
        let placement = WindowPlacement {
            position: (x, y),
            content_size: (1200, 860),
            scale: 1.0,
            display: Some(display),
            ..WindowPlacement::default()
        };
        (id, placement)
    }

    fn register_placed(shared: &AppShared, id: WindowId, placement: WindowPlacement) {
        shared.register_window(id, window_key(id), Rc::new(UiMailbox::new()), "w", WindowKind::Normal);
        assert!(shared.set_window_placement(id, placement));
    }

    #[test]
    fn a_new_window_cascades_from_the_last_one_created_within_three_seconds() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let display = ((0, 0), (2560, 1440));
        let first = shared.allocate_window();
        let second = shared.allocate_window();
        let (_, first_placement) = placed(first, 100, 100, display);
        let (_, second_placement) = placed(second, 300, 300, display);
        register_placed(&shared, first, first_placement);
        register_placed(&shared, second, second_placement);

        assert_eq!(shared.new_window_plan(first, 5.0).position, Some((132, 132)), "no previous window: from this one");

        // T-31/S-13: for three seconds the last window created anchors the
        // next cascade, even when the shortcut is pressed in another window.
        shared.note_window_created(second, 10.0);
        assert_eq!(shared.new_window_plan(first, 12.9).position, Some((332, 332)));
        assert_eq!(
            shared.new_window_plan(first, 13.1).position,
            Some((132, 132)),
            "after 3 s the current window anchors"
        );
        assert_eq!(
            shared.new_window_plan(second, 12.9).position,
            Some((332, 332)),
            "the anchor is the current window too"
        );
    }

    #[test]
    fn a_cascade_that_leaves_the_display_restarts_at_its_top_left_plus_32() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let id = shared.allocate_window();
        let (_, placement) = placed(id, 1800, 900, ((0, 0), (1920, 1080)));
        register_placed(&shared, id, placement);

        // (1832, 932) + the 1200×860 default window leaves the 1920×1080
        // display on both axes (S-13): restart at its top-left + 32.
        assert_eq!(shared.new_window_plan(id, 0.0).position, Some((32, 32)));

        // At scale 2 the step is 64 physical pixels and the frame is 2400×1720.
        let scaled = shared.allocate_window();
        let mut scaled_placement = placement;
        scaled_placement.position = (3700, 2000);
        scaled_placement.scale = 2.0;
        scaled_placement.display = Some(((0, 0), (3840, 2160)));
        register_placed(&shared, scaled, scaled_placement);
        assert_eq!(shared.new_window_plan(scaled, 0.0).position, Some((64, 64)));
    }

    #[test]
    fn a_maximized_source_yields_a_maximized_child_and_a_minimized_one_is_centered() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let maximized = shared.allocate_window();
        let (_, mut placement) = placed(maximized, 0, 0, ((0, 0), (1920, 1080)));
        placement.maximized = true;
        register_placed(&shared, maximized, placement);
        let plan = shared.new_window_plan(maximized, 0.0);
        assert!(plan.maximized, "S-13: maximized source, maximized child");
        assert_eq!(plan.position, Some((32, 32)), "and it still cascades before the window manager maximizes it");

        let minimized = shared.allocate_window();
        let (_, mut placement) = placed(minimized, 1500, 700, ((0, 0), (1920, 1080)));
        placement.minimized = true;
        register_placed(&shared, minimized, placement);
        let plan = shared.new_window_plan(minimized, 0.0);
        assert!(!plan.maximized);
        assert_eq!(plan.position, Some((360, 110)), "S-13: the default 1200×860 centred on the display");
    }

    #[test]
    fn a_closed_anchor_falls_back_to_the_current_window() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let current = shared.allocate_window();
        let (_, placement) = placed(current, 50, 60, ((0, 0), (1920, 1080)));
        register_placed(&shared, current, placement);

        shared.note_window_created(WindowId(999), 1.0);
        assert_eq!(shared.cascade_anchor(current, 2.0), current, "a gone window cannot anchor");
        assert_eq!(shared.new_window_plan(current, 2.0).position, Some((82, 92)));
    }

    #[test]
    fn dirty_documents_are_the_union_of_every_window() {
        let mut registry = registry(vec![entry(1, &[1]), entry(2, &[2])]);
        registry.get_mut(WindowId(1)).unwrap().dirty.insert(DocumentId(1));
        registry.get_mut(WindowId(2)).unwrap().dirty.insert(DocumentId(2));
        registry.get_mut(WindowId(2)).unwrap().dirty.insert(DocumentId(1));

        assert_eq!(registry.dirty_documents(), BTreeSet::from([DocumentId(1), DocumentId(2)]));
    }

    #[test]
    fn a_window_without_a_host_reports_the_missing_opener() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let request = WindowOpenRequest { launch: Launch::default(), kind: WindowKind::Normal, position: None };
        let error = shared.request_new_window(request).expect_err("no opener is installed in unit tests");
        assert!(error.contains("window host"), "{error}");
    }

    #[test]
    fn a_message_to_one_window_lands_in_its_inbox_only() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let first = shared.allocate_window();
        let second = shared.allocate_window();
        let first_inbox = Rc::new(UiMailbox::new());
        let second_inbox = Rc::new(UiMailbox::new());
        shared.register_window(first, window_key(first), first_inbox.clone(), "one", WindowKind::Normal);
        shared.register_window(second, window_key(second), second_inbox.clone(), "two", WindowKind::Normal);

        assert!(shared.post_window(second, UiMsg::PrefsChanged));
        assert_eq!(first_inbox.len(), 0);
        assert_eq!(second_inbox.len(), 1);
        assert_eq!(shared.router(), Some(first), "the first window routes launches");
    }

    #[test]
    fn closing_the_router_promotes_the_next_window() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let first = shared.allocate_window();
        let second = shared.allocate_window();
        shared.register_window(first, window_key(first), Rc::new(UiMailbox::new()), "one", WindowKind::Normal);
        shared.register_window(second, window_key(second), Rc::new(UiMailbox::new()), "two", WindowKind::Normal);

        shared.unregister_window(first);
        assert_eq!(shared.router(), Some(second));
        shared.unregister_window(second);
        assert_eq!(shared.router(), None);
    }

    #[test]
    fn ui_focus_and_close_messages_reach_only_their_window() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let first = WindowState::new(Rc::clone(&shared));
        let mut second = WindowState::new(Rc::clone(&shared));

        assert!(shared.close_window(second.id()));
        assert_eq!(second.ui_inbox().len(), 1);
        assert_eq!(first.ui_inbox().len(), 0);
        let mut tx = Tx::default();
        second.poll(0.0, &mut tx);
        assert!(shared.windows().get(second.id()).is_some_and(|entry| entry.closing), "the close guard ran");
        assert!(!shared.windows().get(first.id()).is_some_and(|entry| entry.closing));

        assert!(shared.focus_window(first.id()));
        assert_eq!(first.ui_inbox().len(), 1);
        assert_eq!(shared.windows().focus_order().first(), Some(&first.id()));
    }

    #[test]
    fn a_launch_for_a_new_file_goes_to_the_idle_window_and_clears_its_claim() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let router = shared.allocate_window();
        let idle = shared.allocate_window();
        let router_inbox = Rc::new(UiMailbox::new());
        let idle_inbox = Rc::new(UiMailbox::new());
        shared.register_window(router, window_key(router), router_inbox.clone(), "one", WindowKind::Normal);
        shared.register_window(idle, window_key(idle), idle_inbox.clone(), "two", WindowKind::Normal);
        let published = WindowSnapshot {
            title: "note".to_owned(),
            active_document: Some(DocumentId(99)),
            documents: BTreeSet::from([DocumentId(99)]),
            dirty: BTreeSet::new(),
            only_start: false,
            prompt_open: false,
        };
        shared.publish_window(router, published);

        let path = temp.path().join("launch.md");
        let launch = Launch { path: Some(path.clone()), line: Some(3), ..Launch::default() };
        let action = Rc::clone(&shared).route_launch(launch.clone(), Some(router));
        assert_eq!(action, RouteAction::Posted(idle));
        assert_eq!(idle_inbox.len(), 2, "the launch and the focus message");
        assert_eq!(shared.windows().focus_order().first(), Some(&idle));

        let doc = shared.docs().find_path(&path).expect("the router registered the loading document");
        assert_eq!(shared.document_claim(doc.id), Some(idle), "a burst cannot route the file twice");

        // The target opens it and publishes; the claim is no longer needed.
        shared.publish_window(
            idle,
            WindowSnapshot {
                title: "launch.md".to_owned(),
                active_document: Some(doc.id),
                documents: BTreeSet::from([doc.id]),
                dirty: BTreeSet::new(),
                only_start: false,
                prompt_open: false,
            },
        );
        assert_eq!(shared.document_claim(doc.id), None);
        assert_eq!(shared.windows().window_showing(doc.id), Some(idle));
    }

    #[test]
    fn a_launch_for_an_already_open_file_targets_its_window() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let first = shared.allocate_window();
        let second = shared.allocate_window();
        let first_inbox = Rc::new(UiMailbox::new());
        let second_inbox = Rc::new(UiMailbox::new());
        shared.register_window(first, window_key(first), first_inbox, "one", WindowKind::Normal);
        shared.register_window(second, window_key(second), second_inbox.clone(), "two", WindowKind::Normal);
        let path = temp.path().join("open.md");
        let doc = shared.docs_mut().insert_loading(DocumentKey::File(path.clone()), "open.md".to_owned());
        shared.publish_window(
            second,
            WindowSnapshot {
                title: "open.md".to_owned(),
                active_document: Some(doc.id),
                documents: BTreeSet::from([doc.id]),
                dirty: BTreeSet::new(),
                only_start: false,
                prompt_open: false,
            },
        );

        let launch = Launch { path: Some(path), ..Launch::default() };
        assert_eq!(Rc::clone(&shared).route_launch(launch, Some(first)), RouteAction::Posted(second));
        assert_eq!(second_inbox.len(), 2, "OpenLaunch then FocusWindow");
    }

    #[test]
    fn new_window_launches_ask_the_installed_opener() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let origin = shared.allocate_window();
        shared.register_window(origin, window_key(origin), Rc::new(UiMailbox::new()), "one", WindowKind::Normal);
        RECORDED.with(|recorded| recorded.borrow_mut().clear());

        let launch = Launch { new_window: true, ..Launch::default() };
        let error = Rc::clone(&shared).route_launch(launch.clone(), Some(origin));
        assert!(matches!(error, RouteAction::Failed(_)), "unit tests never call rune::open_window");

        shared.set_window_opener(recording_opener);
        assert_eq!(Rc::clone(&shared).route_launch(launch.clone(), Some(origin)), RouteAction::Opened(WindowId(0)));
        let recorded = recorded_windows();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].launch, launch);
        assert_eq!(recorded[0].kind, WindowKind::Normal);
    }

    thread_local! {
        static RECORDED: RefCell<Vec<WindowOpenRequest>> = const { RefCell::new(Vec::new()) };
    }

    fn recording_opener(_: &Rc<AppShared>, request: WindowOpenRequest) -> Result<OpenedWindow, String> {
        RECORDED.with(|recorded| recorded.borrow_mut().push(request));
        Ok(OpenedWindow { id: WindowId(0) })
    }

    fn recorded_windows() -> Vec<WindowOpenRequest> {
        RECORDED.with(|recorded| recorded.borrow().clone())
    }
}
