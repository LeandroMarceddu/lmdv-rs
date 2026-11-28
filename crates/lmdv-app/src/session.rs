//! Session capture and restore (P4-02d).
//!
//! A window publishes a [`WindowSession`] fragment to
//! [`AppShared`](crate::shared::AppShared) after every frame that changed what
//! it shows. The fragments are composed into one [`SessionSnapshot`] and
//! written through the store thread on a 500 ms debounce and on close/quit.
//!
//! What is stored is only what can be reopened: a file path or a pasted id and
//! title, the scroll anchor, a pending `--line`, the reading mode and the
//! window's frame. Editor text, undo and the parsed model are never stored;
//! drafts own unsaved text.
//!
//! Restore is an intentional extension (the original never restored tabs):
//! the `run_windows` factory asks [`should_restore`] and builds one window per
//! stored window. A clean quit writes `clean: true`; any value left behind by
//! a crash is `false`, and the restored window says so with a notice.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use lmdv_core::{DocStatus, Document, DocumentKey, Draft, PasteId, SourceBuffer, TabId, WindowId};
use lmdv_io::store::{
    ClosedTabSession, PaneSession, ReaderFindSession, SessionSnapshot, SessionWindowKind, TabMode, TabSession,
    TabSource as SessionTabSource, WindowPlacement as StoredPlacement, WindowSession,
};
use lmdv_io::{AppDirs, Launch};
use rune::{Display, Tx, WindowPlacement as RunePlacement};

use crate::actions::ClosedTab;
use crate::shared::AppShared;
use crate::window::{Mode, Tab, WindowState};
use crate::windows::WindowKind;

/// How long after a tab/window change the session is written.
pub const SESSION_DEBOUNCE: f64 = 0.5;
// The dropped-pasted notice is the catalog's `msg.pastedDropped`/`msg.pastedDroppedN`.

/// The facts that decide whether a launch restores the stored session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoreConditions {
    /// `prefs.restore_session`.
    pub restore_session: bool,
    /// Rune's headless tooling (`--snap`, `--tree`, `--script`, `--window`, …).
    pub headless: bool,
    /// A CLI file argument wins over the session for the first window.
    pub has_path: bool,
    /// `--new-window` asks for a fresh window, never a restored one.
    pub new_window: bool,
    /// `--restore-session` (OI-124): a file argument no longer suppresses the
    /// stored windows; the file still wins for the first one.
    pub force: bool,
}

impl RestoreConditions {
    /// Whether the stored session may be restored at this launch.
    ///
    /// Session restore is deliberately absent from headless tooling: a
    /// `--tree`/`--snap` run must not open the user's windows, and its own
    /// scene must be the only state a gate sees.
    pub fn allows(self) -> bool {
        self.restore_session && !self.headless && !self.new_window && (self.force || !self.has_path)
    }
}

/// Whether this process may restore the stored session for `launch`.
///
/// The one exception is the headless gate seam [`TEST_RESTORE_ENV`], which
/// lets a test inspect restored window keys with `--list-windows`/`--window`;
/// writes stay disabled headless either way.
pub(crate) fn should_restore(shared: &AppShared, launch: &Launch) -> bool {
    RestoreConditions {
        restore_session: shared.prefs().restore_session || launch.restore_session,
        headless: (shared.is_headless() || rune::is_headless()) && !test_restore_seam(),
        has_path: launch.path.is_some(),
        new_window: launch.new_window,
        force: launch.restore_session,
    }
    .allows()
}

/// The headless gate's opt-in for session restore.
///
/// A normal `--snap`/`--tree`/`--list-windows` run never reads the user's
/// session; the gates that check restore set this to `1` and point `LMDV_HOME`
/// at a scratch directory. The app itself never sets it, and a release build
/// ignores it. Writes stay disabled headless, so a gate never replaces the
/// user's session.
pub const TEST_RESTORE_ENV: &str = "LMDV_TEST_RESTORE";

pub(crate) fn test_restore_seam() -> bool {
    cfg!(debug_assertions) && std::env::var_os(TEST_RESTORE_ENV).is_some_and(|value| value == "1")
}

/// The stable key of the nth restored window: `main`, then `window-2`,
/// `window-3`, … The first window keeps the original's `main` name and the
/// others match the id the process would give them.
pub(crate) fn restored_window_key(index: usize) -> String {
    if index == 0 { "main".to_owned() } else { format!("window-{}", index + 1) }
}

/// Where a stored window opens, and whether it opens maximized.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlacementPlan {
    /// `WindowOptions::position`, physical frame coordinates.
    pub position: Option<(i32, i32)>,
    /// `WindowOptions::physical_size`, physical content size.
    pub physical_size: Option<(u32, u32)>,
    /// `WindowSpec::maximized`.
    pub maximized: bool,
    /// The display scale the restored frame was planned against (OI-122).
    /// The registry's first placement carries it so a cascade or a tear-out
    /// near the restored window uses physical pixels of the right size.
    pub scale: f64,
}

/// Plans a restored frame. Positions and sizes exist only where the platform
/// reports them (X11); Wayland keeps the maximized flag and lets the
/// compositor place the window.
///
/// OI-122: a frame stored on another display layout must not come back off
/// screen. The factory's displays are available through [`rune::displays`] at
/// this point, so the plan is clamped to one of them before it is returned.
pub fn placement_plan(window: &WindowSession, positions_supported: bool) -> PlacementPlan {
    let maximized = window.maximized || window.placement.is_some_and(|placement| placement.maximized);
    let mut plan = match (window.placement, positions_supported) {
        (Some(placement), true) => PlacementPlan {
            position: Some((placement.x, placement.y)),
            physical_size: (placement.width > 0 && placement.height > 0)
                .then_some((placement.width as u32, placement.height as u32)),
            maximized,
            scale: 1.0,
        },
        _ => PlacementPlan { position: None, physical_size: None, maximized, scale: 1.0 },
    };
    clamp_plan(&mut plan, &rune::displays());
    plan
}

/// Clamps a plan's frame into a connected display and seeds its scale (OI-122).
///
/// The display containing the stored top-left wins; when none does, the
/// primary display does. A frame larger than that display is pinned to its
/// top-left rather than hanging off the visible area. Headless runs and a
/// session without a frame keep the plan untouched.
pub fn clamp_plan(plan: &mut PlacementPlan, displays: &[Display]) {
    let Some(display) = displays
        .iter()
        .find(|display| {
            plan.position.is_some_and(|(x, y)| {
                x >= display.position.0
                    && y >= display.position.1
                    && x < display.position.0.saturating_add(display.size.0 as i32)
                    && y < display.position.1.saturating_add(display.size.1 as i32)
            })
        })
        .or_else(|| displays.iter().find(|display| display.primary))
        .or_else(|| displays.first())
    else {
        return;
    };
    if display.scale.is_finite() && display.scale > 0.0 {
        plan.scale = display.scale;
    }
    let Some((x, y)) = plan.position else { return };
    let Some((width, height)) = plan.physical_size else { return };
    let display_right = display.position.0.saturating_add(display.size.0 as i32);
    let display_bottom = display.position.1.saturating_add(display.size.1 as i32);
    let max_x = display_right.saturating_sub(width as i32).max(display.position.0);
    let max_y = display_bottom.saturating_sub(height as i32).max(display.position.1);
    plan.position = Some((x.clamp(display.position.0, max_x), y.clamp(display.position.1, max_y)));
}

/// Whether this process's window system exposes global window positions.
///
/// Wayland does not; winit picks Wayland when it is available unless the
/// environment forces X11. A session written on X11 may then be opened on
/// Wayland, so the stored frame is read but its position is not applied.
pub(crate) fn positions_available() -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return true;
    }
    !matches!(std::env::var("WINIT_UNIX_BACKEND").as_deref(), Ok("x11"))
}

/// Gives the registry the restored placement before the native window exists,
/// so a crash before the first map keeps the stored frame and a new window can
/// cascade from it.
pub(crate) fn seed_placement(shared: &AppShared, id: WindowId, plan: PlacementPlan) {
    let Some(position) = plan.position else { return };
    shared.set_window_placement(
        id,
        RunePlacement {
            position,
            content_position: position,
            content_size: plan.physical_size.unwrap_or_default(),
            scale: if plan.scale.is_finite() && plan.scale > 0.0 { plan.scale } else { 1.0 },
            maximized: plan.maximized,
            ..RunePlacement::default()
        },
    );
}

/// The window-level facts a fragment is built from, cheaply readable.
fn session_header(state: &WindowState) -> (Option<StoredPlacement>, bool, SessionWindowKind) {
    let (placement, kind) = {
        let windows = state.shared().windows();
        match windows.get(state.id()) {
            Some(entry) => (entry.placement, entry.kind),
            None => (None, WindowKind::Normal),
        }
    };
    (
        placement.map(|placement| StoredPlacement {
            x: placement.position.0,
            y: placement.position.1,
            width: i32::try_from(placement.content_size.0).unwrap_or(i32::MAX),
            height: i32::try_from(placement.content_size.1).unwrap_or(i32::MAX),
            maximized: placement.maximized,
        }),
        placement.is_some_and(|placement| placement.maximized),
        stored_window_kind(kind),
    )
}

/// Captures one window's restorable state.
pub(crate) fn capture_window(state: &WindowState) -> WindowSession {
    let (placement, maximized, kind) = session_header(state);
    WindowSession {
        placement,
        maximized,
        kind,
        tabs: state.tabs().iter().map(capture_tab).collect(),
        active: state.active_index(),
        closed: stored_closed_tabs(state),
        panes: Some(stored_panes(state)),
    }
}

/// The window's side panes, as the store records them (OI-121).
fn stored_panes(state: &WindowState) -> PaneSession {
    PaneSession {
        left_open: state.left.is_open(),
        right_open: state.right.is_open(),
        left_width: state.left.width(),
        right_width: state.right.width(),
    }
}

/// The window's Ctrl+Shift+T stack, newest last, in the store's schema
/// (OI-121). Pasted tabs carry no text here: their drafts own it and a closed
/// pasted tab without a draft cannot be reopened after a restart either.
fn stored_closed_tabs(state: &WindowState) -> Vec<ClosedTabSession> {
    state
        .closed_tabs()
        .iter()
        .map(|closed| match closed {
            ClosedTab::File { path, index, anchor } => ClosedTabSession {
                source: SessionTabSource::File { path: path.clone() },
                index: *index,
                anchor: Some(anchor.clone()),
            },
            ClosedTab::Pasted { id, title, index, anchor, .. } => ClosedTabSession {
                source: SessionTabSource::Pasted { id: *id, title: title.clone() },
                index: *index,
                anchor: Some(anchor.clone()),
            },
        })
        .collect()
}

/// Whether a stored tab still describes `tab` (OI-151).
///
/// Comparing the restorable facts is a read; [`capture_tab`] clones each
/// Back/Forward list, so an idle frame must not build a fragment at all.
/// The find bar without its source-range hits: those are recomputed from the
/// buffer and must never make an idle frame look changed (OI-121).
fn stored_find(tab: &Tab) -> Option<lmdv_core::FindState> {
    tab.view.find.clone().map(|mut find| {
        find.matches.clear();
        find
    })
}

fn same_tab(stored: &TabSession, tab: &Tab) -> bool {
    let source = match (&tab.source, &stored.source) {
        (crate::window::TabSource::Start, SessionTabSource::Start) => true,
        (crate::window::TabSource::Document(document), SessionTabSource::File { path }) => {
            matches!(&*document.key.borrow(), DocumentKey::File(own) if own == path)
        }
        (crate::window::TabSource::Document(document), SessionTabSource::Pasted { id, title }) => {
            matches!(&*document.key.borrow(), DocumentKey::Untitled(paste) if paste.0 == *id)
                && tab.full_title() == *title
        }
        _ => false,
    };
    source
        && stored.anchor.as_ref() == Some(&tab.view.anchor)
        && stored.line == tab.view.line
        && stored.anchor_id == tab.view.fragment
        && stored.nav == tab.view.nav
        && stored.mode == session_mode(tab.view.mode)
        && stored.find == stored_find(tab)
        && stored.reader_find
            == tab.view.reader_find.open.then(|| ReaderFindSession {
                query: tab.view.reader_find.query.text.clone(),
                current: tab.view.reader_find.current,
            })
        && stored.toc_filter == tab.view.toc_filter
        && stored.details_open == tab.view.details_open
        && stored.outline_hidden == tab.view.outline_hidden
}

fn session_mode(mode: Mode) -> TabMode {
    match mode {
        Mode::Viewer => TabMode::Viewer,
        Mode::Editor => TabMode::Editor,
    }
}

/// The fragment this window last published (OI-151).
///
/// A frame that changed nothing restorable must not clone every tab's 50-entry
/// Back/Forward lists again: the cached fragment answers the comparison, and a
/// real change (tab, place, anchor, mode, active index, window frame) builds
/// the next one. Closing still captures the latest state (see
/// [`WindowState::publish_session`] and [`finish_window`]).
#[derive(Debug, Default)]
pub(crate) struct PublishedSession {
    window: Option<WindowSession>,
    /// Fragments built so far, so a test can prove an idle frame builds none.
    #[cfg(test)]
    builds: std::cell::Cell<u64>,
}

impl PublishedSession {
    /// How many fragments this window built; `#[cfg(test)]` only.
    #[cfg(test)]
    pub(crate) fn builds(&self) -> u64 {
        self.builds.get()
    }

    /// Whether `last` still describes `state`.
    fn matches(&self, state: &WindowState) -> bool {
        let Some(last) = self.window.as_ref() else { return false };
        let (placement, maximized, kind) = session_header(state);
        last.placement == placement
            && last.maximized == maximized
            && last.kind == kind
            && last.panes == Some(stored_panes(state))
            && last.active == state.active_index()
            && last.closed == stored_closed_tabs(state)
            && last.tabs.len() == state.tabs().len()
            && last.tabs.iter().zip(state.tabs()).all(|(stored, tab)| same_tab(stored, tab))
    }
}

fn capture_tab(tab: &Tab) -> TabSession {
    let source = match &tab.source {
        crate::window::TabSource::Start => SessionTabSource::Start,
        crate::window::TabSource::Document(document) => match &*document.key.borrow() {
            DocumentKey::File(path) => SessionTabSource::File { path: path.clone() },
            DocumentKey::Untitled(paste) => SessionTabSource::Pasted { id: paste.0, title: tab.full_title() },
            DocumentKey::Start => SessionTabSource::Start,
        },
    };
    TabSession {
        source,
        anchor: Some(tab.view.anchor.clone()),
        line: tab.view.line,
        anchor_id: tab.view.fragment.clone(),
        nav: tab.view.nav.clone(),
        mode: session_mode(tab.view.mode),
        find: stored_find(tab),
        reader_find: tab.view.reader_find.open.then(|| ReaderFindSession {
            query: tab.view.reader_find.query.text.clone(),
            current: tab.view.reader_find.current,
        }),
        toc_filter: tab.view.toc_filter.clone(),
        details_open: tab.view.details_open.clone(),
        outline_hidden: tab.view.outline_hidden,
    }
}

fn stored_window_kind(kind: WindowKind) -> SessionWindowKind {
    match kind {
        WindowKind::Normal => SessionWindowKind::Normal,
        WindowKind::TornOffStart => SessionWindowKind::TornOffStart,
    }
}

/// The app-side kind of a stored window.
pub(crate) fn window_kind(kind: SessionWindowKind) -> WindowKind {
    match kind {
        SessionWindowKind::Normal => WindowKind::Normal,
        SessionWindowKind::TornOffStart => WindowKind::TornOffStart,
    }
}

/// What restoring a window needs from disk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoredDrafts {
    /// Pasted text, by paste id; the tab exists only when its id is here.
    pub pasted: BTreeMap<u64, Draft>,
    /// File tabs whose edit session found a draft. They reopen in the viewer
    /// so the normal draft prompt decides, instead of silently entering edit
    /// mode over an unread draft.
    pub files_with_drafts: BTreeSet<PathBuf>,
    /// Drafts this version cannot read; the store must keep them (OI-169).
    pub issues: Vec<lmdv_io::store::DraftIssue>,
}

impl RestoredDrafts {
    /// An empty set, for a session with no drafts or a test that builds its
    /// own.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Reads the drafts a stored session needs: every pasted tab's text, and the
/// presence of a draft for file tabs that were being edited.
///
/// Startup only (the factory runs before the first frame); the interactive
/// draft prompt still loads file drafts through the job worker.
pub(crate) fn read_restore_drafts(dirs: &AppDirs, session: &SessionSnapshot) -> RestoredDrafts {
    let mut drafts = RestoredDrafts::new();
    for window in &session.windows {
        for tab in &window.tabs {
            match &tab.source {
                SessionTabSource::Pasted { id, .. } => {
                    if !drafts.pasted.contains_key(id) {
                        let loaded = lmdv_io::store::load_draft_reported(dirs, &pasted_key(*id));
                        if let Some(issue) = loaded.issue {
                            drafts.issues.push(issue);
                        }
                        if let Some(draft) = loaded.value {
                            drafts.pasted.insert(*id, draft);
                        }
                    }
                }
                SessionTabSource::File { path } if tab.mode == TabMode::Editor => {
                    let loaded = lmdv_io::store::load_draft_reported(dirs, &file_key(path));
                    if let Some(issue) = loaded.issue {
                        drafts.issues.push(issue);
                    }
                    if let Some(draft) = loaded.value {
                        // OI-123: an equal-to-disk draft is dropped silently at
                        // open; waiting here for a prompt that will not come
                        // would leave a restored editor tab in viewer mode.
                        let silent =
                            lmdv_io::read_document(path).is_ok_and(|on_disk| on_disk.buffer.text() == draft.text);
                        if !silent {
                            drafts.files_with_drafts.insert(path.clone());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    drafts
}

fn pasted_key(id: u64) -> String {
    format!("pasted:{id}")
}

fn file_key(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

/// What one window's restore did, for the notices and the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    /// Pasted tabs reopened from their draft.
    pub restored_pasted: usize,
    /// Pasted tabs whose draft is gone, dropped with a notice.
    pub dropped_pasted: usize,
    /// Tabs whose stored edit mode is waiting for the document to load.
    pub editor_tabs: usize,
}

/// Rebuilds a window's tabs in their stored order.
///
/// The state must have no tabs (the factory clears the initial start page) and
/// is left with its stored active tab, its anchor queued through the normal
/// reading path. Files open through [`WindowState::open_path`], so a restored
/// window uses exactly the launch path: parse, position table, recents and
/// draft prompts.
pub(crate) fn restore_tabs(
    state: &mut WindowState,
    window: &WindowSession,
    drafts: &RestoredDrafts,
    pasted_docs: &mut BTreeMap<u64, Rc<Document>>,
) -> RestoreReport {
    let mut report = RestoreReport::default();
    let mut index_map: Vec<Option<usize>> = Vec::with_capacity(window.tabs.len());
    state.restoring_tabs = true;
    // A restored tab is not a place reached from the one before it (O-12).
    state.navigation.suppress();

    for tab in &window.tabs {
        let restored: Option<(TabId, bool)> = match &tab.source {
            SessionTabSource::Start => Some((state.push_start_tab(), false)),
            SessionTabSource::File { path } => {
                // OI-123: two windows restoring the same file must not start
                // two load jobs. The first window owns the job; later windows
                // attach the already-registered document instead.
                let absolute = lmdv_io::absolute_lexical(path);
                let existing = state.shared().docs().find_path(&absolute);
                Some(match existing {
                    Some(document) => (state.attach_document(document), false),
                    None => (state.open_path(path.clone()), false),
                })
            }
            SessionTabSource::Pasted { id, title } => match drafts.pasted.get(id) {
                Some(draft) => {
                    let document = match pasted_docs.get(id) {
                        Some(document) => document.clone(),
                        None => {
                            let document = restored_pasted_document(state.shared(), *id, title, draft);
                            pasted_docs.insert(*id, document.clone());
                            document
                        }
                    };
                    let tab_id = state.attach_document(document.clone());
                    if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        tab.view.edit_base = Some(draft.base.clone());
                        tab.view.dirty = document.is_dirty();
                        tab.view.draft_written = tab.view.dirty;
                    }
                    report.restored_pasted += 1;
                    Some((tab_id, true))
                }
                None => {
                    report.dropped_pasted += 1;
                    None
                }
            },
        };

        let Some((tab_id, pasted)) = restored else {
            index_map.push(None);
            continue;
        };
        let Some(index) = state.tabs.iter().position(|candidate| candidate.id == tab_id) else {
            index_map.push(None);
            continue;
        };
        index_map.push(Some(index));
        if let Some(restored) = state.tabs.get_mut(index) {
            if let Some(anchor) = &tab.anchor {
                restored.view.anchor = anchor.clone();
            }
            if let Some(id) = &tab.anchor_id
                && !id.is_empty()
            {
                restored.view.anchor.id = id.clone();
                restored.view.fragment = Some(id.clone());
            }
            restored.view.line = tab.line;
            restored.view.nav = tab.nav.clone();
            restored.view.nav.sanitize();
            // OI-121: the restorable per-tab surfaces come back with the tab;
            // matches are recomputed by the find refresh on the next frame.
            if let Some(find) = &tab.find {
                restored.view.find = Some(find.clone());
            }
            if let Some(reader_find) = &tab.reader_find {
                restored.view.reader_find.open = true;
                restored.view.reader_find.query.replace_text(reader_find.query.clone());
                restored.view.reader_find.current = reader_find.current;
                crate::reader_find::refresh_tab(restored);
            }
            restored.view.toc_filter = tab.toc_filter.clone();
            restored.view.details_open = tab.details_open.clone();
            restored.view.outline_hidden = tab.outline_hidden;
            if tab.mode == TabMode::Editor && !restored.is_start() {
                let has_draft = match &tab.source {
                    SessionTabSource::File { path } => drafts.files_with_drafts.contains(path),
                    SessionTabSource::Pasted { .. } => true,
                    SessionTabSource::Start => false,
                };
                if pasted || !has_draft {
                    state.pending_editor_tabs.insert(tab_id);
                    report.editor_tabs += 1;
                }
            }
        }
        if matches!(tab.source, SessionTabSource::File { .. })
            && let Some(document) = state.tabs.get(index).and_then(Tab::document)
            && matches!(*document.status.borrow(), DocStatus::Loading)
        {
            state.restored_docs.insert(document.id);
        }
    }

    // OI-121: the closed-tab stack is part of the window, not of a tab, so it
    // survives a restart for both files and pasted text (whose draft owns the
    // text). Drop entries whose pasted draft is gone.
    for closed in &window.closed {
        let record = match &closed.source {
            SessionTabSource::File { path } => ClosedTab::File {
                path: path.clone(),
                index: closed.index,
                anchor: closed.anchor.clone().unwrap_or_default(),
            },
            SessionTabSource::Pasted { id, title } => match drafts.pasted.get(id) {
                Some(draft) => ClosedTab::Pasted {
                    id: *id,
                    title: title.clone(),
                    text: draft.text.clone(),
                    index: closed.index,
                    anchor: closed.anchor.clone().unwrap_or_default(),
                },
                None => continue,
            },
            SessionTabSource::Start => continue,
        };
        state.remember_closed_tab(record);
    }

    state.restoring_tabs = false;
    state.navigation.unsuppress();
    if state.tabs.is_empty() {
        state.push_start_tab();
    }
    let active = index_map
        .get(window.active)
        .and_then(|index| *index)
        .or_else(|| index_map.iter().rev().flatten().copied().next())
        .unwrap_or(0);
    state.active = active.min(state.tabs.len().saturating_sub(1));
    state.sync_editor_panes();
    if let Some(panes) = window.panes {
        state.left.set_width(panes.left_width);
        state.left.set_open(panes.left_open);
        state.right.set_width(panes.right_width);
        state.right.set_open(panes.right_open);
    }
    let anchor = state.active_tab().map(|tab| tab.view.anchor.clone()).unwrap_or_default();
    state.begin_restore(&anchor);
    state.load_outline_filter();
    state.sync_outline();
    report
}

fn restored_pasted_document(shared: &AppShared, id: u64, title: &str, draft: &Draft) -> Rc<Document> {
    let paste = PasteId(id);
    shared.reserve_paste(paste);
    let id = shared.docs_mut().allocate_id();
    let document = Rc::new(Document::ready(id, DocumentKey::Untitled(paste), title, SourceBuffer::new(&draft.text)));
    // The draft's base is the text the edits started from; comparing it with
    // the restored buffer keeps the tab dirty when the draft really changed
    // something, exactly as the editor would have left it.
    document.saved.borrow_mut().hash = lmdv_core::hash_text(&draft.base);
    shared.docs_mut().insert_ready(document.clone());
    document
}

/// Opens the CLI file in the first restored window when `--restore-session`
/// was passed alongside it (OI-124). Stored windows keep their tabs; the file
/// joins the first one instead of replacing the whole session.
///
/// The one-line factory call is in the reported `app.rs` patch; until the
/// orchestrator applies it, this helper is intentionally unused (OI-124).
#[allow(dead_code)]
pub(crate) fn open_launch_in_restored(state: &mut WindowState, launch: &Launch) -> bool {
    let Some(path) = launch.path.as_ref() else { return false };
    state.open_path(path.clone());
    state.sync_registry();
    let _ = state.publish_session();
    true
}

/// The notice a restored window shows, if any.
pub(crate) fn restore_notice(session: &SessionSnapshot, window_index: usize, report: RestoreReport) -> Option<String> {
    let mut parts = Vec::new();
    if window_index == 0 && !session.clean {
        parts.push(lmdv_i18n::t!("msg.draftRecovered").to_owned());
    }
    if report.restored_pasted == 1 {
        parts.push(lmdv_i18n::t!("msg.pastedRecovered").to_owned());
    } else if report.restored_pasted > 1 {
        parts.push(lmdv_i18n::t!("msg.pastedRecoveredN", "n" => report.restored_pasted));
    }
    if report.dropped_pasted == 1 {
        parts.push(lmdv_i18n::t!("msg.pastedDropped").to_owned());
    } else if report.dropped_pasted > 1 {
        parts.push(lmdv_i18n::t!("msg.pastedDroppedN", "n" => report.dropped_pasted));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Completes a window close (`quit == false`) or an app quit.
///
/// The last window closing is a clean exit, so it keeps its own entry and
/// writes `clean: true`. A window closing while others remain leaves the
/// session: its entry is removed and the remaining windows stay unclean. A
/// quit keeps every window and marks the file clean; the last window's
/// `on_quit` may be skipped, so `Drop` repeats this in-memory decision.
pub(crate) fn finish_window(state: &mut WindowState, quit: bool) {
    let last = state.shared().windows().len() <= 1;
    if quit || last {
        state.shared().set_quitting();
    }
    state.flush_all_drafts();
    state.capture_active_position();
    state.shared().refresh_placement(state.id());
    if state.shared().is_quitting() {
        let _ = state.publish_session();
        state.shared().queue_session(true);
    } else {
        state.shared().remove_session_window(state.id());
        state.shared().queue_session(false);
    }
    state.session_flushed = true;
    state.shared().flush_store(Duration::from_millis(750));
}

impl WindowState {
    /// Publishes this window's restorable fragment, building it only when
    /// something restorable changed (OI-151). True when the published fragment
    /// changed.
    ///
    /// The comparison reads the fragment already built here; it does not clone
    /// any tab's history. A last-moment change still reaches the crash snapshot
    /// because [`finish_window`] and `Drop` call this after capturing the
    /// position, and the cached fragment is compared against that state too.
    pub(crate) fn publish_session(&self) -> bool {
        let mut published = self.session_publish.borrow_mut();
        if published.matches(self) {
            return false;
        }
        let captured = capture_window(self);
        #[cfg(test)]
        published.builds.set(published.builds.get() + 1);
        let changed = self.shared.publish_session_window(self.id(), captured.clone());
        published.window = Some(captured);
        changed
    }

    /// Drops the factory's initial start page before restored tabs are added.
    pub(crate) fn clear_tabs_for_restore(&mut self) {
        self.tabs.clear();
        self.active = 0;
    }

    /// Applies stored edit modes once their documents are ready and no draft
    /// prompt has to decide first. Returns whether the view changed.
    pub(crate) fn apply_pending_editor_tabs(&mut self, tx: &mut Tx) -> bool {
        if self.pending_editor_tabs.is_empty() {
            return false;
        }
        let original = self.active;
        let pending: Vec<TabId> = self.pending_editor_tabs.iter().copied().collect();
        let mut changed = false;
        for tab_id in pending {
            let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
                self.pending_editor_tabs.remove(&tab_id);
                changed = true;
                continue;
            };
            let Some(document) = self.tabs[index].document().cloned() else {
                self.pending_editor_tabs.remove(&tab_id);
                changed = true;
                continue;
            };
            let (ready, loading) = {
                let status = document.status.borrow();
                (matches!(*status, DocStatus::Ready), matches!(*status, DocStatus::Loading))
            };
            if !ready {
                if !loading {
                    self.pending_editor_tabs.remove(&tab_id);
                    changed = true;
                }
                continue;
            }
            if document.lease_holder().is_some_and(|lease| !(lease.window == self.id() && lease.tab == tab_id)) {
                // Another restored window got the lease first; this view stays
                // a reader instead of opening a startup dialog.
                self.pending_editor_tabs.remove(&tab_id);
                continue;
            }
            self.pending_editor_tabs.remove(&tab_id);
            if index != self.active {
                self.switch_to_index(index);
                changed = true;
            }
            if self.active_tab().is_some_and(|tab| tab.view.mode == Mode::Viewer) {
                changed |= self.enter_editor(None, tx);
            }
        }
        if self.active != original && original < self.tabs.len() {
            self.switch_to_index(original);
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use lmdv_io::Launch;
    use lmdv_io::store::{StoreHandle, StorePatch, TabSource as SessionTabSource, load_draft, load_session};
    use rune::WindowPlacement as RunePlacement;
    use tempfile::TempDir;

    use super::*;
    use crate::app::{StartupRestore, startup_states};
    use crate::test_support;

    fn state(shared: &Rc<AppShared>, key: &str) -> WindowState {
        let id = shared.allocate_window();
        WindowState::new_keyed(shared.clone(), id, key.to_owned())
    }

    fn pasted_tab_session(id: u64, title: &str, mode: TabMode) -> TabSession {
        TabSession { source: SessionTabSource::Pasted { id, title: title.to_owned() }, mode, ..TabSession::default() }
    }

    #[test]
    fn restore_is_skipped_for_headless_launches_and_cli_files() {
        let allowed = RestoreConditions {
            restore_session: true,
            headless: false,
            has_path: false,
            new_window: false,
            force: false,
        };
        assert!(allowed.allows());
        assert!(!RestoreConditions { restore_session: false, ..allowed }.allows());
        assert!(!RestoreConditions { headless: true, ..allowed }.allows());
        assert!(!RestoreConditions { has_path: true, ..allowed }.allows());
        assert!(!RestoreConditions { new_window: true, ..allowed }.allows());

        // OI-124: the explicit flag overrides prefs and a CLI path for the
        // stored windows, but never headless or `--new-window`.
        let forced = RestoreConditions { force: true, has_path: true, ..allowed };
        assert!(forced.allows());
        assert!(!RestoreConditions { headless: true, ..forced }.allows());
        assert!(!RestoreConditions { new_window: true, ..forced }.allows());
        // `should_restore` sets `restore_session` from the flag too.
        assert!(RestoreConditions { force: true, ..allowed }.allows());
    }

    #[test]
    fn restored_windows_keep_the_main_name_then_count() {
        assert_eq!(restored_window_key(0), "main");
        assert_eq!(restored_window_key(1), "window-2");
        assert_eq!(restored_window_key(4), "window-5");
    }

    #[test]
    fn wayland_omits_the_stored_frame_but_keeps_maximized() {
        let window = WindowSession {
            placement: Some(StoredPlacement { x: 120, y: 80, width: 1000, height: 700, maximized: false }),
            maximized: true,
            ..WindowSession::default()
        };
        let x11 = placement_plan(&window, true);
        assert_eq!(x11.position, Some((120, 80)));
        assert_eq!(x11.physical_size, Some((1000, 700)));
        assert!(x11.maximized);

        let wayland = placement_plan(&window, false);
        assert_eq!(wayland.position, None);
        assert_eq!(wayland.physical_size, None);
        assert!(wayland.maximized, "the maximum state survives without a frame");
    }

    #[test]
    fn a_session_without_a_placement_still_restores_maximized() {
        let window = WindowSession { maximized: true, ..WindowSession::default() };
        let plan = placement_plan(&window, true);
        assert_eq!(plan.position, None);
        assert!(plan.maximized);
    }

    #[test]
    fn restored_frames_are_clamped_to_a_visible_display_and_seed_its_scale() {
        let display = Display { id: None, name: None, position: (0, 0), size: (1920, 1080), scale: 2.0, primary: true };
        let mut plan = PlacementPlan {
            position: Some((3000, 1200)),
            physical_size: Some((1000, 700)),
            maximized: false,
            scale: 1.0,
        };
        clamp_plan(&mut plan, std::slice::from_ref(&display));
        assert_eq!(plan.position, Some((920, 380)), "the frame is pulled back onto the display");
        assert_eq!(plan.scale, 2.0, "the first placement carries the display's scale");

        let mut oversized = PlacementPlan {
            position: Some((-100, -100)),
            physical_size: Some((2400, 1600)),
            maximized: false,
            scale: 1.0,
        };
        clamp_plan(&mut oversized, &[display]);
        assert_eq!(oversized.position, Some((0, 0)), "a frame too big for the display pins to its origin");

        let mut headless =
            PlacementPlan { position: Some((9, 9)), physical_size: None, scale: 1.0, ..Default::default() };
        clamp_plan(&mut headless, &[]);
        assert_eq!(headless.position, Some((9, 9)));
    }

    #[test]
    fn the_closed_tab_stack_survives_a_restart() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("closed.md");
        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                tabs: vec![TabSession::default()],
                closed: vec![
                    ClosedTabSession {
                        source: SessionTabSource::File { path: path.clone() },
                        index: 2,
                        anchor: Some(lmdv_core::ScrollAnchor { id: "where".to_owned(), offset: 8.0 }),
                    },
                    ClosedTabSession {
                        source: SessionTabSource::Pasted { id: 6, title: "Lost paste".to_owned() },
                        index: 0,
                        anchor: None,
                    },
                ],
                ..WindowSession::default()
            }],
        };
        let drafts = RestoredDrafts {
            pasted: BTreeMap::from([(6, Draft::new("kept\n", "kept\n", 1))]),
            ..RestoredDrafts::default()
        };
        let shared = test_support::shared(&temp);
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &drafts, positions: true }),
        );
        let closed = restored[0].state.closed_tabs();
        assert_eq!(closed.len(), 2, "a closed file and a closed pasted tab with a draft both return");
        let ClosedTab::File { path: stored, index, anchor } = &closed[0] else {
            panic!("the restored entry is a file tab");
        };
        assert_eq!(stored, &path);
        assert_eq!(*index, 2);
        assert_eq!(anchor.id, "where");
        let ClosedTab::Pasted { id, text, .. } = &closed[1] else {
            panic!("the restored entry is a pasted tab");
        };
        assert_eq!(*id, 6);
        assert_eq!(text, "kept\n");

        // A pasted closed tab whose draft is gone cannot be reopened.
        let no_draft = RestoredDrafts::new();
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &no_draft, positions: true }),
        );
        assert_eq!(restored[0].state.closed_tabs().len(), 1);
    }

    #[test]
    fn an_equal_to_disk_draft_does_not_delay_the_stored_editor_mode() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        let path = temp.path().join("same.md");
        std::fs::write(&path, "# On disk\n").unwrap();
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        handle.patch(StorePatch::Draft { key: file_key(&path), draft: Draft::new("# On disk\n", "# On disk\n", 1) });
        assert!(handle.flush(Duration::from_secs(2)));
        drop(handle);

        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                tabs: vec![TabSession {
                    source: SessionTabSource::File { path: path.clone() },
                    mode: TabMode::Editor,
                    ..TabSession::default()
                }],
                ..WindowSession::default()
            }],
        };
        let drafts = read_restore_drafts(&dirs, &session);
        assert!(!drafts.files_with_drafts.contains(&path), "OI-123: a silent drop has no prompt to wait for");

        std::fs::write(&path, "# Changed on disk\n").unwrap();
        let drafts = read_restore_drafts(&dirs, &session);
        assert!(drafts.files_with_drafts.contains(&path), "a real draft still waits for the prompt");
    }

    #[test]
    fn two_restored_windows_share_one_load_for_the_same_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("shared.md");
        let shared = test_support::shared(&temp);
        let doc = shared.docs_mut().insert_loading(DocumentKey::File(path.clone()), "shared.md".to_owned());
        let session = SessionSnapshot {
            clean: true,
            windows: vec![
                WindowSession {
                    tabs: vec![TabSession {
                        source: SessionTabSource::File { path: path.clone() },
                        ..TabSession::default()
                    }],
                    ..WindowSession::default()
                },
                WindowSession {
                    tabs: vec![TabSession {
                        source: SessionTabSource::File { path: path.clone() },
                        ..TabSession::default()
                    }],
                    ..WindowSession::default()
                },
            ],
        };
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &RestoredDrafts::new(), positions: true }),
        );
        let first = restored[0].state.tabs()[0].document().expect("first tab");
        let second = restored[1].state.tabs()[0].document().expect("second tab");
        assert!(Rc::ptr_eq(first, second));
        assert_eq!(first.id, doc.id, "both windows joined the one registered document");
    }

    #[test]
    fn capture_serializes_the_reopenable_state_only() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = state(&shared, "window-1");
        let tab = state.open_synthetic("Secret", "editor text that must never reach session.json");
        if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
            tab.view.anchor = lmdv_core::ScrollAnchor { id: "heading".to_owned(), offset: 24.0 };
            tab.view.line = Some(3);
            tab.view.mode = Mode::Editor;
        }
        shared.set_window_placement(
            state.id(),
            RunePlacement { position: (40, 60), content_size: (900, 700), maximized: true, ..RunePlacement::default() },
        );
        let captured = capture_window(&state);
        assert_eq!(captured.active, 0);
        assert_eq!(captured.tabs[0].anchor.as_ref().map(|anchor| anchor.id.as_str()), Some("heading"));
        assert_eq!(captured.tabs[0].line, Some(3));
        assert_eq!(captured.tabs[0].mode, TabMode::Editor);
        assert!(captured.maximized);
        assert_eq!(captured.placement.map(|placement| placement.width), Some(900));

        let json = serde_json::to_string(&captured).unwrap();
        assert!(!json.contains("editor text"), "editor text is never serialized: {json}");
        let parsed: WindowSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, captured);
    }

    #[test]
    fn find_outline_details_and_panes_round_trip_through_the_session() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let path = temp.path().join("notes.md");
        std::fs::write(&path, "# Notes\n\nbody\n").unwrap();
        let mut state = state(&shared, "window-1");
        let tab_id = state.open_path(path);
        let tab = state.tabs.iter_mut().find(|tab| tab.id == tab_id).expect("the tab");
        let mut find = lmdv_core::FindState { query: "body".to_owned(), ..lmdv_core::FindState::default() };
        find.matches.push(lmdv_core::FindMatch { source: 0..1, block: 0, line: 0 });
        tab.view.find = Some(find);
        tab.view.reader_find.open = true;
        tab.view.reader_find.query.replace_text("body");
        tab.view.reader_find.current = 2;
        tab.view.toc_filter = "head".to_owned();
        tab.view.details_open.insert("b1/details:0".to_owned());
        tab.view.outline_hidden = true;
        state.left.set_width(310.0);
        state.left.set_open(true);
        state.right.set_width(280.0);
        state.right.set_open(true);
        assert!(state.publish_session());
        let session = shared.session_snapshot(true);

        let restored_shared = test_support::shared(&temp);
        let restored = startup_states(
            Rc::clone(&restored_shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &RestoredDrafts::new(), positions: true }),
        );
        let window = &restored[0].state;
        let tab = window.tabs().iter().find(|tab| !tab.is_start()).expect("the tab");
        let find = tab.view.find.as_ref().expect("the editor find bar");
        assert_eq!(find.query, "body");
        assert!(find.matches.is_empty(), "hits are recomputed, not frozen into the session");
        assert!(tab.view.reader_find.open);
        assert_eq!(tab.view.reader_find.query.text, "body");
        assert_eq!(tab.view.reader_find.current, 2);
        assert_eq!(tab.view.toc_filter, "head");
        assert!(tab.view.details_open.contains("b1/details:0"));
        assert!(tab.view.outline_hidden);
        assert_eq!(window.left.width(), 310.0);
        assert!(window.left.is_open());
        assert_eq!(window.right.width(), 280.0);
        // Right openness follows the restored outline's headings (O-06); the
        // left pane's dock state is taken verbatim.
    }

    /// Headless proof for the restore/clamp slice. Set
    /// `LMDV_SESSION_PROOF_DIR` to keep the artifacts under
    /// `/tmp/opencode/session-polish/`; otherwise this is an ordinary test.
    #[test]
    fn a_restore_proof_records_the_clamp_and_the_restored_state() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("restored.md");
        std::fs::write(&path, "# Restored\n\nbody\n").unwrap();
        let session = SessionSnapshot {
            clean: false,
            windows: vec![WindowSession {
                placement: Some(StoredPlacement { x: 5000, y: 4000, width: 1200, height: 860, maximized: false }),
                kind: SessionWindowKind::TornOffStart,
                tabs: vec![TabSession {
                    source: SessionTabSource::File { path: path.clone() },
                    mode: TabMode::Editor,
                    toc_filter: "head".to_owned(),
                    ..TabSession::default()
                }],
                active: 0,
                closed: vec![ClosedTabSession {
                    source: SessionTabSource::File { path: path.clone() },
                    index: 1,
                    anchor: Some(lmdv_core::ScrollAnchor { id: "where".to_owned(), offset: 4.0 }),
                }],
                ..WindowSession::default()
            }],
        };
        let window_session = &session.windows[0];
        let displays =
            vec![Display { id: None, name: None, position: (0, 0), size: (1920, 1080), scale: 2.0, primary: true }];
        let mut plan = PlacementPlan {
            position: Some((5000, 4000)),
            physical_size: Some((1200, 860)),
            maximized: false,
            scale: 1.0,
        };
        clamp_plan(&mut plan, &displays);
        assert_eq!(plan.position, Some((720, 220)), "the stored frame is pulled onto the only display");
        assert_eq!(plan.scale, 2.0);

        let shared = test_support::shared(&temp);
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &RestoredDrafts::new(), positions: true }),
        );
        let state = &restored[0].state;
        assert_eq!(state.tabs()[0].view.toc_filter, "head");
        assert_eq!(state.closed_tabs().len(), 1);
        assert_eq!(restored[0].kind, WindowKind::TornOffStart);
        assert!(state.ui.notice.is_some(), "an unclean session reports recovery");

        if let Some(dir) = std::env::var_os("LMDV_SESSION_PROOF_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
            std::fs::write(
                dir.join("clamp.txt"),
                format!(
                    "stored=({},{}) {}+{}\nclamped={:?} scale={}\nrestored kind={:?} tabs={} closed={} filter={:?}\n",
                    window_session.placement.unwrap().x,
                    window_session.placement.unwrap().y,
                    window_session.placement.unwrap().width,
                    window_session.placement.unwrap().height,
                    plan.position,
                    plan.scale,
                    restored[0].kind,
                    state.tabs().len(),
                    state.closed_tabs().len(),
                    state.tabs()[0].view.toc_filter,
                ),
            )
            .unwrap();
        }
    }

    #[test]
    fn capture_keeps_the_torn_off_kind() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let id = shared.allocate_window();
        let state = WindowState::new_keyed_kind(shared.clone(), id, "window-2".to_owned(), WindowKind::TornOffStart);
        let captured = capture_window(&state);
        assert_eq!(captured.kind, SessionWindowKind::TornOffStart);
    }

    #[test]
    fn restoring_a_file_tab_does_not_rewrite_the_recent_files() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("already-open.md");
        std::fs::write(&path, "# Already\n").unwrap();
        let shared = test_support::shared(&temp);
        let id = shared.docs_mut().allocate_id();
        let document = Rc::new(Document::ready(
            id,
            DocumentKey::File(path.clone()),
            "already-open.md",
            SourceBuffer::new("# Already\n"),
        ));
        shared.docs_mut().insert_ready(document.clone());
        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                tabs: vec![TabSession {
                    source: SessionTabSource::File { path: path.clone() },
                    ..TabSession::default()
                }],
                ..WindowSession::default()
            }],
        };
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &RestoredDrafts::new(), positions: true }),
        );
        assert!(shared.recents().is_empty(), "a restored file is not a new recent");
        assert!(restored[0].state.tabs()[0].document().is_some_and(|view| Rc::ptr_eq(view, &document)));
    }

    #[test]
    fn two_windows_capture_and_restore_through_the_factory_plan() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut first = state(&shared, "window-1");
        let mut second = state(&shared, "window-2");
        let first_paste = first.open_synthetic("First notes", "# First\n\nbody\n");
        let first_paste_id = first
            .tab(first_paste)
            .and_then(Tab::document)
            .and_then(|document| match &*document.key.borrow() {
                DocumentKey::Untitled(paste) => Some(paste.0),
                _ => None,
            })
            .unwrap();
        let second_paste = second.open_synthetic("Second notes", "# Second\n");
        let second_paste_id = second
            .tab(second_paste)
            .and_then(Tab::document)
            .and_then(|document| match &*document.key.borrow() {
                DocumentKey::Untitled(paste) => Some(paste.0),
                _ => None,
            })
            .unwrap();
        let third_paste = second.open_synthetic("Third notes", "# Third\n");
        let third_paste_id = second
            .tab(third_paste)
            .and_then(Tab::document)
            .and_then(|document| match &*document.key.borrow() {
                DocumentKey::Untitled(paste) => Some(paste.0),
                _ => None,
            })
            .unwrap();
        if let Some(tab) = second.active_tab_mut() {
            tab.view.anchor = lmdv_core::ScrollAnchor { id: "two".to_owned(), offset: 42.0 };
            tab.view.line = Some(7);
        }
        second.active = 1;
        shared.set_window_kind(second.id(), WindowKind::TornOffStart);

        assert!(first.publish_session());
        assert!(second.publish_session());
        let session = shared.session_snapshot(false);
        assert_eq!(session.windows.len(), 2);

        let drafts = RestoredDrafts {
            pasted: BTreeMap::from([
                (first_paste_id, Draft::new("# First\n\nbody\n", "# First\n", 1)),
                (second_paste_id, Draft::new("# Second\n", "# Second\n", 2)),
                (third_paste_id, Draft::new("# Third\n", "# Third\n", 3)),
            ]),
            files_with_drafts: BTreeSet::new(),
            issues: Vec::new(),
        };

        let restored_shared = test_support::shared(&temp);
        let restored = startup_states(
            Rc::clone(&restored_shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &drafts, positions: true }),
        );
        assert_eq!(restored.iter().map(|window| window.key.as_str()).collect::<Vec<_>>(), vec!["main", "window-2"]);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].state.tabs().len(), 1, "one persisted pasted tab");
        assert_eq!(restored[1].state.tabs().len(), 2);
        assert_eq!(restored[1].state.active_index(), 1);
        assert_eq!(restored[1].state.tabs()[1].view.anchor.id, "two", "the per-tab anchor travels with the session");
        assert_eq!(restored[1].state.tabs()[1].view.line, Some(7));
        assert_eq!(
            restored_shared.windows().by_key("window-2").map(|entry| entry.kind),
            Some(WindowKind::TornOffStart)
        );
        assert_eq!(restored[1].state.tabs()[0].full_title(), "Second notes");
        assert_eq!(
            restored[1].state.tabs()[0].document().map(|document| document.text().text().to_owned()),
            Some("# Second\n".to_owned())
        );
    }

    #[test]
    fn a_pasted_tab_without_a_draft_is_dropped_with_a_notice() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                tabs: vec![
                    pasted_tab_session(1, "Kept", TabMode::Viewer),
                    pasted_tab_session(2, "Lost", TabMode::Viewer),
                ],
                active: 1,
                ..WindowSession::default()
            }],
        };
        let drafts = RestoredDrafts {
            pasted: BTreeMap::from([(1, Draft::new("kept", "kept", 1))]),
            ..RestoredDrafts::default()
        };
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &drafts, positions: true }),
        );
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].state.tabs().len(), 1);
        assert_eq!(restored[0].state.tabs()[0].full_title(), "Kept");
        let notice = restored[0].state.ui.notice.as_ref().expect("the lost pasted tab is reported");
        assert!(notice.text.contains("draft is missing"), "{}", notice.text);
        assert_eq!(restored[0].state.active_index(), 0, "the dropped active index falls back");
    }

    #[test]
    fn a_crashed_session_restores_and_says_it_was_recovered() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let session = SessionSnapshot {
            clean: false,
            windows: vec![WindowSession { tabs: vec![TabSession::default()], ..WindowSession::default() }],
        };
        let drafts = RestoredDrafts::new();
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &drafts, positions: true }),
        );
        assert_eq!(restored[0].key, "main");
        let notice = restored[0].state.ui.notice.as_ref().expect("a crash is announced");
        assert_eq!(notice.text, lmdv_i18n::t!("msg.draftRecovered"));
    }

    #[test]
    fn a_clean_session_restores_without_the_recovery_notice() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession { tabs: vec![TabSession::default()], ..WindowSession::default() }],
        };
        let drafts = RestoredDrafts::new();
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &drafts, positions: true }),
        );
        assert!(restored[0].state.ui.notice.is_none());
    }

    #[test]
    fn a_restored_editor_tab_enters_edit_mode_on_the_first_poll() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                tabs: vec![pasted_tab_session(4, "Draft", TabMode::Editor)],
                ..WindowSession::default()
            }],
        };
        let drafts = RestoredDrafts {
            pasted: BTreeMap::from([(4, Draft::new("changed\n", "base\n", 1))]),
            ..RestoredDrafts::default()
        };
        let mut restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &drafts, positions: true }),
        );
        let window = &mut restored[0].state;
        assert_eq!(window.tabs()[0].view.mode, Mode::Viewer, "mode waits for a live frame");
        let mut tx = Tx::default();
        window.poll(0.0, &mut tx);
        assert_eq!(window.tabs()[0].view.mode, Mode::Editor);
        assert!(window.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some());
    }

    #[test]
    fn a_file_editor_tab_with_a_draft_waits_for_the_draft_prompt() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let path = temp.path().join("edited.md");
        std::fs::write(&path, "# On disk\n").unwrap();
        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                tabs: vec![TabSession {
                    source: SessionTabSource::File { path: path.clone() },
                    mode: TabMode::Editor,
                    ..TabSession::default()
                }],
                ..WindowSession::default()
            }],
        };
        let drafts = RestoredDrafts { files_with_drafts: BTreeSet::from([path]), ..RestoredDrafts::default() };
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &drafts, positions: true }),
        );
        assert!(restored[0].state.pending_editor_tabs.is_empty(), "the draft prompt owns edit entry");
    }

    #[test]
    fn close_and_quit_write_the_clean_flag_and_drop_closed_windows() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut first = state(&shared, "window-1");
        let mut second = state(&shared, "window-2");
        let _ = first.publish_session();
        let _ = second.publish_session();

        finish_window(&mut first, false);
        assert!(shared.flush_store(Duration::from_secs(2)));
        let closed = load_session(shared.dirs());
        assert!(!closed.clean, "another window is still open, so the exit was not clean");
        assert_eq!(closed.windows.len(), 1, "the closed window leaves the session");
        drop(first);

        finish_window(&mut second, false);
        assert!(shared.flush_store(Duration::from_secs(2)));
        let quit = load_session(shared.dirs());
        assert!(quit.clean, "the last window closing is a clean exit");
        assert_eq!(quit.windows.len(), 1);
        drop(second);
    }

    #[test]
    fn a_quit_keeps_every_window_and_marks_the_session_clean() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut first = state(&shared, "window-1");
        let mut second = state(&shared, "window-2");
        first.publish_session();
        second.publish_session();
        finish_window(&mut first, true);
        finish_window(&mut second, true);
        assert!(shared.flush_store(Duration::from_secs(2)));
        let session = load_session(shared.dirs());
        assert!(session.clean);
        assert_eq!(session.windows.len(), 2, "a quit restores the whole session next time");
    }

    #[test]
    fn headless_tooling_never_writes_a_session_file() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        shared.set_headless();
        let state = state(&shared, "window-1");
        let _ = state.publish_session();
        shared.queue_session(false);
        assert!(shared.flush_store(Duration::from_secs(2)));
        assert!(!shared.dirs().session_path().exists(), "a --snap/--tree run must not clobber the user session");
    }

    #[test]
    fn restored_tabs_keep_their_order_paths_anchors_and_lines() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ordered.md");
        std::fs::write(&path, "# One\n\nbody\n").unwrap();
        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                tabs: vec![
                    TabSession::default(),
                    TabSession {
                        source: SessionTabSource::File { path: path.clone() },
                        anchor: Some(lmdv_core::ScrollAnchor { id: "one".to_owned(), offset: 36.0 }),
                        line: Some(3),
                        ..TabSession::default()
                    },
                    TabSession::default(),
                ],
                active: 1,
                ..WindowSession::default()
            }],
        };
        let shared = test_support::shared(&temp);
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &RestoredDrafts::new(), positions: true }),
        );
        let state = &restored[0].state;
        assert_eq!(state.tabs().len(), 3);
        assert!(state.tabs()[0].is_start());
        assert!(state.tabs()[2].is_start());
        assert_eq!(state.tabs()[1].full_title(), "ordered.md");
        assert_eq!(state.tabs()[1].document().and_then(|doc| doc.key.borrow().path().map(PathBuf::from)), Some(path));
        assert_eq!(state.active_index(), 1);
        assert_eq!(state.tabs()[1].view.anchor.id, "one");
        assert_eq!(state.tabs()[1].view.line, Some(3));
    }

    #[test]
    fn two_windows_showing_one_pasted_document_share_it_on_restore() {
        let temp = TempDir::new().unwrap();
        let session = SessionSnapshot {
            clean: true,
            windows: vec![
                WindowSession {
                    tabs: vec![pasted_tab_session(8, "Shared paste", TabMode::Viewer)],
                    ..WindowSession::default()
                },
                WindowSession {
                    tabs: vec![pasted_tab_session(8, "Shared paste", TabMode::Viewer)],
                    ..WindowSession::default()
                },
            ],
        };
        let drafts = RestoredDrafts {
            pasted: BTreeMap::from([(8, Draft::new("shared\n", "shared\n", 1))]),
            ..RestoredDrafts::default()
        };
        let shared = test_support::shared(&temp);
        let restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &drafts, positions: true }),
        );
        let first = restored[0].state.tabs()[0].document().cloned().expect("first document");
        let second = restored[1].state.tabs()[0].document().cloned().expect("second document");
        assert!(Rc::ptr_eq(&first, &second), "one pasted document per paste id");
    }

    #[test]
    fn a_session_file_is_written_and_reread() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        let shared = AppShared::start(dirs.clone());
        let mut state = state(&shared, "window-1");
        state.open_synthetic("Note", "private body text\n");
        state.publish_session();
        shared.queue_session(false);
        assert!(shared.flush_store(Duration::from_secs(2)));

        let text = std::fs::read_to_string(dirs.session_path()).unwrap();
        println!("session file {}:\n{text}", dirs.session_path().display());
        assert!(text.contains("\"schema\": 1"));
        assert!(!text.contains("private body"), "editor text is never stored");
        let loaded = load_session(&dirs);
        assert!(!loaded.clean);
        assert_eq!(loaded.windows.len(), 1);
        assert!(matches!(loaded.windows[0].tabs[0].source, SessionTabSource::Pasted { .. }));
    }

    #[test]
    fn closing_a_restored_clean_pasted_tab_removes_its_draft() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        handle.patch(StorePatch::Draft { key: pasted_key(3), draft: Draft::new("kept\n", "kept\n", 1) });
        assert!(handle.flush(Duration::from_secs(2)));
        drop(handle);

        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                tabs: vec![pasted_tab_session(3, "Kept", TabMode::Viewer)],
                ..WindowSession::default()
            }],
        };
        let drafts = read_restore_drafts(&dirs, &session);
        assert!(drafts.pasted.contains_key(&3));
        let shared = test_support::shared(&temp);
        let mut restored = startup_states(
            Rc::clone(&shared),
            &Launch::default(),
            None,
            Some(StartupRestore { session: &session, drafts: &drafts, positions: true }),
        );
        let state = &mut restored[0].state;
        let tab = state.tabs()[0].id;
        assert_eq!(state.close_tab(tab), crate::window::CloseOutcome::Closed);
        assert!(shared.flush_store(Duration::from_secs(2)));
        assert!(load_draft(&dirs, &pasted_key(3)).is_none(), "P-19: a clean pasted tab leaves no draft");
    }

    #[test]
    fn read_restore_drafts_reads_pasted_text_and_only_edited_file_drafts() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        handle.patch(StorePatch::Draft { key: pasted_key(9), draft: Draft::new("pasted text\n", "pasted\n", 1) });
        let edited = PathBuf::from("/tmp/lmdv-session-edited.md");
        let viewing = PathBuf::from("/tmp/lmdv-session-viewing.md");
        handle.patch(StorePatch::Draft { key: file_key(&edited), draft: Draft::new("# edited\n", "# base\n", 2) });
        handle.patch(StorePatch::Draft { key: file_key(&viewing), draft: Draft::new("# viewing\n", "# base\n", 3) });
        assert!(handle.flush(Duration::from_secs(2)));
        drop(handle);

        let session = SessionSnapshot {
            clean: false,
            windows: vec![WindowSession {
                tabs: vec![
                    pasted_tab_session(9, "Pasted", TabMode::Viewer),
                    TabSession {
                        source: SessionTabSource::File { path: edited.clone() },
                        mode: TabMode::Editor,
                        ..TabSession::default()
                    },
                    TabSession {
                        source: SessionTabSource::File { path: viewing.clone() },
                        mode: TabMode::Viewer,
                        ..TabSession::default()
                    },
                ],
                ..WindowSession::default()
            }],
        };
        let drafts = read_restore_drafts(&dirs, &session);
        assert_eq!(drafts.pasted.get(&9).map(|draft| draft.text.as_str()), Some("pasted text\n"));
        assert!(drafts.files_with_drafts.contains(&edited));
        assert!(!drafts.files_with_drafts.contains(&viewing), "viewer tabs are handled by the draft prompt");
        assert_eq!(load_draft(&dirs, &pasted_key(9)).map(|draft| draft.text), Some("pasted text\n".to_owned()));

        // Only the pasted tab had a draft, so a viewer-only file session
        // reads as empty.
        let files_only = SessionSnapshot {
            windows: vec![WindowSession {
                tabs: vec![TabSession {
                    source: SessionTabSource::File { path: viewing },
                    mode: TabMode::Viewer,
                    ..TabSession::default()
                }],
                ..WindowSession::default()
            }],
            ..SessionSnapshot::default()
        };
        assert!(read_restore_drafts(&dirs, &files_only).files_with_drafts.is_empty());
    }

    #[test]
    fn session_keys_are_stable_and_tab_kinds_round_trip() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let id = shared.allocate_window();
        let mut state =
            WindowState::new_keyed_kind(shared.clone(), id, "window-2".to_owned(), WindowKind::TornOffStart);
        state.clear_tabs_for_restore();
        assert!(state.tabs().is_empty());
        state.push_start_tab();
        assert_eq!(state.tabs().len(), 1);
    }

    #[test]
    fn an_idle_frame_does_not_rebuild_the_published_fragment() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = state(&shared, "window-1");
        let tab = state.open_synthetic("Note", "# Note\n");
        assert!(state.publish_session(), "the first publish builds the fragment");
        let builds = state.session_publish.borrow().builds();
        for _ in 0..20 {
            assert!(!state.publish_session(), "nothing restorable changed");
        }
        assert_eq!(state.session_publish.borrow().builds(), builds, "an idle frame builds no fragment");

        // A Back/Forward place is restorable: the next publish builds once.
        if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
            tab.view.nav.record(lmdv_core::Place::new("pasted:9", lmdv_core::ScrollAnchor::top()));
        }
        assert!(state.publish_session());
        assert_eq!(state.session_publish.borrow().builds(), builds + 1);
        assert_eq!(shared.session_snapshot(false).windows[0].tabs[0].nav.back[0].path, "pasted:9");
    }

    #[test]
    fn a_new_tab_or_a_window_frame_change_republishes_the_session() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = state(&shared, "window-1");
        // The constructor published the start page; this is the baseline.
        let _ = state.publish_session();
        // The first document replaces the start page; the second appends.
        state.open_synthetic("First", "first\n");
        assert!(state.publish_session(), "an opened document is restorable");
        let builds = state.session_publish.borrow().builds();
        state.open_synthetic("Second", "second\n");
        assert!(state.publish_session(), "a new tab is restorable");
        assert_eq!(state.session_publish.borrow().builds(), builds + 1);
        assert_eq!(shared.session_snapshot(false).windows[0].tabs.len(), 2);

        shared.set_window_placement(
            state.id(),
            RunePlacement { position: (40, 60), content_size: (900, 700), ..RunePlacement::default() },
        );
        assert!(state.publish_session(), "the native frame is restorable");
        let stored = shared.session_snapshot(false).windows[0].placement.expect("a stored frame");
        assert_eq!((stored.x, stored.y, stored.width, stored.height), (40, 60, 900, 700));
    }

    #[test]
    fn a_last_moment_change_reaches_the_crash_snapshot() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = state(&shared, "window-1");
        let tab = state.open_synthetic("Note", "# Note\n");
        assert!(state.publish_session());
        // The reader moved and entered edit mode after the last published
        // fragment; closing must still store both.
        if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
            tab.view.anchor = lmdv_core::ScrollAnchor { id: "late".to_owned(), offset: 72.0 };
            tab.view.mode = Mode::Editor;
        }
        finish_window(&mut state, true);
        let session = shared.session_snapshot(true);
        assert_eq!(session.windows[0].tabs[0].anchor.as_ref().map(|anchor| anchor.id.as_str()), Some("late"));
        assert_eq!(session.windows[0].tabs[0].mode, TabMode::Editor);
    }
}
