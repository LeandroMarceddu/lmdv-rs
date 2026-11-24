//! The Rune application: state construction, scenes, poll and key routing.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use lmdv_io::instance::{Claim, InstancePaths, InstanceServer, LaunchAck, LaunchRequest, forward};
use lmdv_io::store::{SessionSnapshot, load_session_reported};
use lmdv_io::{AppDirs, DirsIssue, Launch};
use rune::{AppSpec, Key, KeyEvent, Tx, Waker};

use crate::commands::{command_for_key_in, dispatch_command};
use crate::session::RestoredDrafts;
use crate::shared::{APP_ID, AppShared};
use crate::view;
use crate::window::WindowState;
use crate::windows::{WindowHandles, WindowKind};

/// The default content size of every window, in points (S-12).
pub const WINDOW_SIZE: (f32, f32) = (1200.0, 860.0);
/// The smallest content size a window can be resized to (S-12).
pub const WINDOW_MIN_SIZE: (f32, f32) = (480.0, 480.0);
/// Headless two-window gate: the primary window's frame, in physical pixels.
const TEST_PRIMARY_POSITION: (i32, i32) = (80, 80);
/// Headless two-window gate: the second window's frame, in physical pixels.
const TEST_SECOND_POSITION: (i32, i32) = (2480, 160);

/// A headless scene: a named state mutator for `--scene`.
pub type Scene = fn(&mut WindowState);

/// Headless scenes, for `--snap`, `--tree` and `--scene`.
pub const SCENES: &[(&str, Scene)] = &[
    ("start", view::scene_start),
    ("tabs", view::scene_tabs),
    ("tab-overflow", view::scene_tab_overflow),
    ("help", view::scene_help),
    ("help-full", view::scene_help_full),
    ("reader", view::scene_reader),
    ("reader-sepia", view::scene_reader_sepia),
    ("reader-black", view::scene_reader_black),
    ("reader-find", view::scene_reader_find),
    ("reader-line", view::scene_reader_line),
    ("reader-progress", view::scene_reader_progress),
    ("reader-section", view::scene_reader_section),
    ("reader-outline", view::scene_reader_outline),
    ("browser", view::scene_browser),
    ("browser-menu", view::scene_browser_menu),
    ("share", view::scene_share),
    ("editor", view::scene_editor),
    ("editor-menus", view::scene_editor_menus),
    ("notice-stack", view::scene_notice_stack),
    ("editor-block", view::scene_editor_block),
    ("editor-size", view::scene_editor_size),
    ("tab-menu", view::scene_tab_menu),
    ("tab-menu-file", view::scene_tab_menu_file),
    ("tab-window-menu", view::scene_tab_window_menu),
    ("tab-card", view::scene_tab_card),
    ("table-preview", crate::table_ui::scene_table_preview),
    ("lightbox", crate::lightbox::scene_lightbox),
    ("palette", crate::palette_ui::scene_palette),
    ("settings", crate::settings_ui::scene_settings),
    ("tab-notice", scene_tab_notice),
    ("version", scene_version),
];

/// T-26: two documents, the second in front, while the first keeps its own
/// sticky notice; the scene ends back on the first tab, so `--tree` shows the
/// notice that was waiting there rather than one raised for the front tab.
fn scene_tab_notice(state: &mut WindowState) {
    let first = state.open_synthetic("Notices", "# Notices\n\nThe first tab keeps its own notice.\n");
    state.sticky_notice("Not found: deleted.md", crate::window::NoticeKind::Error);
    state.open_synthetic("Second", "# Second\n\nAnother tab.\n");
    state.activate_tab(first);
}

/// H-13: the start page's footer and the full Help dialog are both on screen,
/// so `--tree` reads the one `CARGO_PKG_VERSION` they share.
fn scene_version(state: &mut WindowState) {
    state.open_help(crate::window::HelpView::Full);
}

/// Runs lmdv for a parsed launch.
pub fn run(launch: Launch) {
    let resolved = AppDirs::resolve_reported();
    run_reported(resolved.dirs, resolved.issues, launch);
}

/// Runs lmdv with explicit directories (tests, portable builds and the
/// orchestrator's binary).
///
/// The app runs through Rune's window-factory entry point so every window —
/// the first one and those opened at runtime by `rune::open_window` — is a
/// full document window that keeps the process alive. This also gives the
/// headless tools their `--window KEY`/`--list-windows` selection, which a
/// single `App` cannot provide.
pub fn run_with_dirs(dirs: AppDirs, launch: Launch) {
    run_reported(dirs, Vec::new(), launch);
}

/// [`run`] with the folder-resolution issues the launch found.
///
/// A portable root that could not be created falls back to XDG with a notice
/// in the first window (OI-012); the doubts about `dirs` travel beside it
/// rather than being dropped by `AppDirs::resolve`.
fn run_reported(dirs: AppDirs, dirs_issues: Vec<DirsIssue>, launch: Launch) {
    rune::set_app_id(APP_ID);
    // Rune ends a quitting app at once by default (`fast_exit`): the process exits
    // without dropping windows or the shared state, so drafts, the session store
    // flush, tab leases and the single-instance socket's removal would be skipped.
    // Their quit hooks cover most of it, but not the socket file or the per-window
    // `Drop`; keep the orderly teardown this app has always had.
    rune::orderly_exit();
    let shared = AppShared::start_reported(dirs, dirs_issues);
    // The saved language (or the OS's, for Auto) is process-wide and must be in
    // place before the first window builds its strings.
    crate::settings::apply_locale(&shared.prefs());
    // A headless Rune run (`--snap`, `--tree`, `--script`, ...) is a tool
    // invocation, not a desktop launch: it must never be forwarded to a
    // running window and must not steal the single-instance socket. It also
    // parses and decodes before its single settled frame, so goldens do not
    // race a worker.
    let headless = tooling_invocation();
    if headless {
        shared.set_headless();
    }
    // Only the real entry point installs the native opener; a unit test never
    // reaches `rune::open_window` unless it installs its own recorder.
    shared.set_window_opener(crate::windows::open_runtime_window);
    if !headless && let Err(error) = claim_instance(&shared, &launch) {
        eprintln!("lmdv: single-instance service unavailable: {error}");
    }
    let factory_shared = Rc::clone(&shared);
    let factory_launch = launch.clone();
    rune::run_windows(move |displays| {
        startup_windows(
            Rc::clone(&factory_shared),
            factory_launch.clone(),
            headless_test_window(),
            center_on_primary(displays),
        )
    });
}

/// The top-left that centres a logical window on a display (S-12).
///
/// `display` is the display's physical `(x, y, width, height)`; a logical
/// content size becomes physical pixels at `scale`. A window larger than its
/// display is pinned to the display's top-left instead of hanging off it.
pub(crate) fn centered_position(display: (i32, i32, u32, u32), logical: (f32, f32), scale: f64) -> (i32, i32) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let width = (f64::from(logical.0) * scale).round() as i64;
    let height = (f64::from(logical.1) * scale).round() as i64;
    let x = i64::from(display.0) + (i64::from(display.2) - width) / 2;
    let y = i64::from(display.1) + (i64::from(display.3) - height) / 2;
    (x.max(i64::from(display.0)) as i32, y.max(i64::from(display.1)) as i32)
}

/// The centred top-left for the launch window, from the factory's displays.
fn center_on_primary(displays: &[rune::Display]) -> Option<(i32, i32)> {
    let display = displays.iter().find(|display| display.primary).or_else(|| displays.first())?;
    Some(centered_position(
        (display.position.0, display.position.1, display.size.0, display.size.1),
        WINDOW_SIZE,
        display.scale,
    ))
}

/// Gives the first unplaced startup window the centred position (S-12).
///
/// A stored session placement and the headless gate's fixed frame are kept;
/// only a window the session did not place opens centred.
fn pick_launch_position(windows: &mut [StartupWindow], center: Option<(i32, i32)>) {
    if let Some(center) = center
        && let Some(window) = windows.iter_mut().find(|window| window.frame.position.is_none())
    {
        window.frame.position = Some(center);
    }
}

/// The windows every process starts with.
///
/// A normal launch restores the stored session when
/// [`crate::session::should_restore`] allows it: one window per stored window,
/// the first keyed `main` and the rest `window-N`. Headless tooling, a CLI
/// file and `--new-window` all keep the single launch window instead. The
/// headless two-window gate's `second` window comes from
/// `LMDV_TEST_WINDOW=<path>`; `rune::open_window` is inert headlessly, so a
/// gate that needs two windows asks the factory.
///
/// The clean flag starts `false` here: from this point a crash leaves a marker
/// the next launch reports. A clean close or quit overwrites it with `true`.
fn startup_windows(
    shared: Rc<AppShared>,
    launch: Launch,
    test_window: Option<PathBuf>,
    center: Option<(i32, i32)>,
) -> Vec<rune::AnyWindowSpec> {
    // The session is read after the store has started, so a future-schema or
    // foreign file must be protected before the factory's first session patch
    // — even when this launch will not restore it, because a CLI launch or
    // `--new-window` replaces the stored session on close.
    let loaded = (!(shared.is_headless() || rune::is_headless())).then(|| load_session_reported(shared.dirs()));
    if let Some(issue) = loaded.as_ref().and_then(|loaded| loaded.issue.clone()) {
        shared.note_store_issue(issue.clone());
        shared.protect_store_files(std::slice::from_ref(&issue));
    }
    let session = loaded
        .map(|loaded| loaded.value)
        .filter(|_| crate::session::should_restore(&shared, &launch))
        .filter(|session| !session.windows.is_empty());
    let drafts =
        session.as_ref().map(|session| crate::session::read_restore_drafts(shared.dirs(), session)).unwrap_or_default();
    // A draft this version cannot read stays on disk for its writer (OI-169).
    shared.protect_drafts(&drafts.issues);
    let restore = session.as_ref().map(|session| StartupRestore {
        session,
        drafts: &drafts,
        positions: crate::session::positions_available(),
    });
    let mut windows = startup_states(Rc::clone(&shared), &launch, test_window, restore);
    pick_launch_position(&mut windows, center);
    shared.queue_session(false);
    windows.into_iter().map(|window| window_spec(window.key, window.state, window.frame, window.kind).boxed()).collect()
}

/// A window's frame for its first map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WindowFrame {
    /// Physical frame top-left (`WindowOptions::position`).
    pub position: Option<(i32, i32)>,
    /// Physical content size (`WindowOptions::physical_size`).
    pub physical_size: Option<(u32, u32)>,
    pub maximized: bool,
}

/// One window the startup factory will create, before it is boxed.
pub(crate) struct StartupWindow {
    pub key: String,
    pub state: WindowState,
    pub frame: WindowFrame,
    pub kind: WindowKind,
}

/// The stored session and its drafts, when this launch may restore one.
pub(crate) struct StartupRestore<'a> {
    pub session: &'a SessionSnapshot,
    pub drafts: &'a RestoredDrafts,
    /// Whether this process's window system exposes positions (X11).
    pub positions: bool,
}

/// Builds the startup windows without a native host, so the factory and its
/// tests share one plan.
pub(crate) fn startup_states(
    shared: Rc<AppShared>,
    launch: &Launch,
    test_window: Option<PathBuf>,
    restore: Option<StartupRestore<'_>>,
) -> Vec<StartupWindow> {
    if let Some(restore) = restore {
        let mut windows = Vec::with_capacity(restore.session.windows.len());
        let mut pasted_docs = BTreeMap::new();
        for (index, window_session) in restore.session.windows.iter().enumerate() {
            let key = crate::session::restored_window_key(index);
            let kind = crate::session::window_kind(window_session.kind);
            let id = shared.allocate_window();
            let mut state = WindowState::new_keyed_kind(Rc::clone(&shared), id, key.clone(), kind);
            state.clear_tabs_for_restore();
            let plan = crate::session::placement_plan(window_session, restore.positions);
            crate::session::seed_placement(&shared, id, plan);
            let report = crate::session::restore_tabs(&mut state, window_session, restore.drafts, &mut pasted_docs);
            if let Some(notice) = crate::session::restore_notice(restore.session, index, report) {
                // The startup warnings (folder fallback, ignored state files)
                // belong to the same first frame; keep both sentences.
                match state.ui.notice.as_mut() {
                    Some(existing) => {
                        existing.text.push(' ');
                        existing.text.push_str(&notice);
                    }
                    None => state.notice(notice),
                }
            }
            state.sync_registry();
            let _ = state.publish_session();
            windows.push(StartupWindow {
                key,
                state,
                frame: WindowFrame {
                    position: plan.position,
                    physical_size: plan.physical_size,
                    maximized: plan.maximized,
                },
                kind,
            });
        }
        if !windows.is_empty() {
            return windows;
        }
    }

    let primary_state = WindowState::start(Rc::clone(&shared), launch);
    let primary_key = crate::windows::window_key(primary_state.id());
    let primary_position = test_window.as_ref().map(|_| TEST_PRIMARY_POSITION);
    let mut windows = vec![StartupWindow {
        key: primary_key,
        state: primary_state,
        frame: WindowFrame { position: primary_position, ..WindowFrame::default() },
        kind: WindowKind::Normal,
    }];
    if let Some(path) = test_window {
        let second_launch = Launch { path: Some(path), ..Launch::default() };
        let second_state = WindowState::start_keyed(Rc::clone(&shared), &second_launch, "second".to_owned());
        windows.push(StartupWindow {
            key: "second".to_owned(),
            state: second_state,
            frame: WindowFrame { position: Some(TEST_SECOND_POSITION), ..WindowFrame::default() },
            kind: WindowKind::Normal,
        });
    }
    windows
}

/// The headless gate's second window, when the environment declares one.
fn headless_test_window() -> Option<PathBuf> {
    if !rune::is_headless() {
        return None;
    }
    std::env::var_os("LMDV_TEST_WINDOW").map(PathBuf::from)
}

/// The callbacks every lmdv window shares.
fn app_spec() -> AppSpec<WindowState> {
    AppSpec {
        min_size: Some(WINDOW_MIN_SIZE),
        scenes: SCENES,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("lmdv", WINDOW_SIZE, fallback_state, view::view)
    }
}

/// One window's spec, with the same close guard, store flush, focus tracking
/// and key routing as the first window.
pub(crate) fn window_spec(
    key: String,
    state: WindowState,
    frame: WindowFrame,
    kind: WindowKind,
) -> rune::WindowSpec<WindowState> {
    let id = state.id();
    let shared = state.shared_rc();
    let app = rune::App::new(app_spec())
        .on_close_request(close_request)
        .on_close(window_closed)
        .on_quit(app_quit)
        .on_focus(focus_changed);
    let mut spec = rune::WindowSpec::from_app(key, app).state(state).on_modifiers(crate::window::modifiers_changed);
    if let Some(position) = frame.position {
        spec.options.position = Some(position);
    }
    if let Some(size) = frame.physical_size {
        spec.options.physical_size = Some(size);
    }
    if frame.maximized {
        spec = spec.maximized(true);
    }
    // Factory windows cannot return their native handle to the caller; this
    // hook attaches it (and the window's `AppHandle`) as soon as the host
    // exists. `rune::open_window` windows attach the same pair immediately.
    let app_handle = spec.app.handle();
    spec = spec.on_created(move |native| {
        shared.attach_window_handles(id, WindowHandles { app: app_handle.clone(), native }, kind);
    });
    spec
}

/// Whether this process was started by Rune's headless tooling.
///
/// The flags are Rune's, not lmdv's; `rune::extra_args()` excludes them, so the
/// raw argument list is the only place to recognize the mode before the app
/// starts.
fn tooling_invocation() -> bool {
    std::env::args().skip(1).any(|arg| {
        arg.starts_with("--snap")
            || arg.starts_with("--tree")
            || arg.starts_with("--script")
            || arg.starts_with("--frames")
            || arg.starts_with("--scene")
            || arg.starts_with("--size")
            || arg.starts_with("--scale")
            || arg.starts_with("--at")
            || arg.starts_with("--out")
            || arg.starts_with("--video")
            || arg.starts_with("--desktop")
            || arg.starts_with("--html")
            || arg.starts_with("--trace")
            || arg.starts_with("--list-")
            || arg.starts_with("--window")
            || arg == "--first-pixels"
    })
}

/// Claims the process-wide instance role.
///
/// A second process forwards its parsed launch to the running one and exits;
/// the primary keeps the listener alive in `AppShared` and its window's `poll`
/// drains the forwarded launches. A failure here degrades to a normal app
/// start without single-instance routing rather than blocking the user.
fn claim_instance(shared: &std::rc::Rc<AppShared>, launch: &Launch) -> Result<(), String> {
    let paths = InstancePaths::discover(shared.dirs()).map_err(|error| error.to_string())?;
    let inbox = shared.launch_inbox();
    match InstanceServer::claim(paths, move |request| {
        inbox.push(request);
        LaunchAck::ok()
    })
    .map_err(|error| error.to_string())?
    {
        Claim::Primary(server) => {
            shared.set_instance(server);
            Ok(())
        }
        Claim::Secondary(paths) => {
            let mut request = LaunchRequest::new(launch.clone());
            request.activation_token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
            match forward(&paths, &request).map_err(|error| error.to_string())? {
                ack if ack.ok => std::process::exit(0),
                ack => Err(ack.error.unwrap_or_else(|| "the running instance refused the launch".to_owned())),
            }
        }
    }
}

/// The `AppSpec::init` requirement. [`run_with_dirs`] always installs a real
/// first state through `App::init`; this path only runs if another host builds
/// the spec by hand.
fn fallback_state() -> WindowState {
    WindowState::start(AppShared::start(AppDirs::resolve()), &Launch::default())
}

/// Installs this window's UI waker so worker completions, launches and
/// cross-window messages wake `poll`. Each window owns one; the shared inboxes
/// installed a composite that wakes them all.
fn setup(state: &mut WindowState, waker: Waker) {
    state.shared().attach_window_waker(state.id(), waker.clone());
    state.worker().attach(waker.clone());
    // The window's `rune::ImageCache` is created on the first picture; it
    // decodes on its own threads and wakes this frame when one lands.
    state.attach_waker(waker);
}

/// Records the focus order used by launch routing (the native focus comes from
/// Rune itself).
fn focus_changed(state: &mut WindowState, focused: bool, _: &mut Tx) {
    if focused {
        state.shared().note_window_focused(state.id());
        // Linux can drop a recursive watch (inotify limits, suspends): reinstall
        // it when the window comes back, as the original does (F-08).
        state.rearm_browser_watch();
    }
}

/// Drains mailboxes and timers; true when the view must rebuild.
fn poll(state: &mut WindowState, now: f64, tx: &mut Tx) -> bool {
    state.poll(now, tx)
}

/// Global shortcuts. The palette holds the keyboard while it shows (C-09), its
/// opening keys open it anywhere (C-01), the Help dialog takes Escape and
/// `Ctrl+W` first (T-29); then the command table's default chords.
pub(crate) fn on_key(state: &mut WindowState, event: &KeyEvent, tx: &mut Tx) -> bool {
    if state.prompt().is_some() {
        if event.key == Key::Escape {
            state.prompt_cancel();
            return true;
        }
        // The dialog's buttons take the keyboard through Rune's modal focus.
        return false;
    }
    // The settings dialog is modal: Escape backs out of it (or out of its
    // language list / reset question first), and a chord that would run a
    // window command behind it is held back. Tab traversal, arrow navigation
    // and Enter/Space on its controls still reach the dialog.
    if state.settings_is_open() {
        if state.settings_key(event, tx) {
            return true;
        }
        return command_for_key_in(state, event).is_some();
    }
    // The palette is modal: this is the fallback for a frame where focus sits
    // outside its search box (the box's own handler takes the keys first).
    if state.palette_is_open() && state.palette_key(event, tx) {
        return true;
    }
    if let Some(chord) = crate::palette::palette_open_chord(state, event) {
        state.open_palette(Some(chord), tx);
        return true;
    }
    if state.help_view().is_some() && (event.key == Key::Escape || (event.is_command('w') && !event.mods.shift)) {
        state.close_help();
        return true;
    }
    // H-04: F1 again closes the Help it opened.
    if state.help_view().is_some() && event.key == Key::Function(1) && !event.mods.ctrl && !event.mods.alt {
        state.close_help();
        return true;
    }
    // R-11: `?` toggles the keys overlay. The dialog's own modal handles Esc and
    // a backdrop click; `?` again closes it here. A focused text field takes the
    // character first, so typing `?` in the find bar never opens this.
    if state.help_view().is_some() && crate::commands::key_matches(event, "?") {
        state.close_help();
        return true;
    }
    if state.share.menu_open && event.key == Key::Escape {
        state.close_share_menu();
        return true;
    }
    // The table preview's own layer handles its keys; this is the fallback for
    // a frame where focus sits nowhere inside it yet (V-14).
    if state.table_preview().is_some() && event.key == Key::Escape {
        state.table_preview_escape(tx);
        return true;
    }
    if state.editor_context_key(event, tx) {
        return true;
    }
    if state.viewer_context_key(event, tx) {
        return true;
    }
    if state.reading_key(event, tx) {
        return true;
    }
    if state.tab_key(event, tx) {
        return true;
    }
    if event.key == Key::Escape && state.ui.notice.is_some() {
        state.dismiss_notice();
        return true;
    }
    let Some(command) = command_for_key_in(state, event) else { return false };
    dispatch_command(state, command.id, tx)
}

/// Vetoes a native close while a document is dirty, showing the real
/// Save / Don't save / Cancel dialog (ADR-020, OI-006).
fn close_request(state: &mut WindowState, _: &mut Tx) -> bool {
    state.guard_close_window()
}

/// Flushes drafts, reading positions and the session when one window closes.
///
/// A window closing is not the app quitting while other windows remain (T-25):
/// the closed window leaves the session and the others stay recoverable.
/// The last window closing is a clean exit and writes `clean: true`.
fn window_closed(state: &mut WindowState, _: &mut Tx) {
    crate::session::finish_window(state, false);
}

/// Flushes on an app quit (Rune's `on_quit`). Every live window keeps its
/// session entry, so the next launch restores them all; the last window's
/// `on_quit` is skipped, and `WindowState::drop` makes the same decision there.
fn app_quit(state: &mut WindowState, _: &mut Tx) {
    crate::session::finish_window(state, true);
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::test_support;
    use lmdv_io::store::load_session;

    fn window(temp: &TempDir) -> WindowState {
        WindowState::new(test_support::shared(temp))
    }

    #[test]
    fn escape_dismisses_a_notice() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.notice("hello");
        let mut tx = Tx::default();
        let event = KeyEvent { key: Key::Escape, mods: rune::Mods::default() };
        assert!(on_key(&mut state, &event, &mut tx));
        assert!(state.ui.notice.is_none());
    }

    #[test]
    fn escape_closes_help_before_a_notice() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_help(crate::window::HelpView::Shortcuts);
        state.notice("hello");
        let mut tx = Tx::default();
        let event = KeyEvent { key: Key::Escape, mods: rune::Mods::default() };
        assert!(on_key(&mut state, &event, &mut tx));
        assert_eq!(state.help_view(), None);
        assert!(state.ui.notice.is_some());
    }

    #[test]
    fn ctrl_w_closes_help_before_the_tab() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.new_start_tab();
        state.open_help(crate::window::HelpView::Full);
        let mut tx = Tx::default();
        let event =
            KeyEvent { key: Key::Command('w'), mods: rune::Mods { cmd: true, ctrl: true, ..rune::Mods::default() } };
        assert!(on_key(&mut state, &event, &mut tx));
        assert_eq!(state.help_view(), None);
        assert_eq!(state.tabs().len(), 2, "the tab was not closed");
    }

    #[test]
    fn question_mark_opens_and_closes_the_keys_overlay() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.new_start_tab();
        let question =
            KeyEvent { key: Key::Text("?".to_owned()), mods: rune::Mods { shift: true, ..rune::Mods::default() } };
        let mut tx = Tx::default();
        assert!(on_key(&mut state, &question, &mut tx), "R-11: `?` opens the overlay on the start page");
        assert_eq!(state.help_view(), Some(crate::window::HelpView::Shortcuts));
        assert!(on_key(&mut state, &question, &mut tx), "`?` again closes it");
        assert_eq!(state.help_view(), None);
    }

    #[test]
    fn f1_opens_the_full_help_and_question_mark_the_shortcut_list() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();

        let f1 = KeyEvent { key: Key::Function(1), mods: rune::Mods::default() };
        assert!(on_key(&mut state, &f1, &mut tx), "F1 runs app.help");
        assert_eq!(state.help_view(), Some(crate::window::HelpView::Full));
        assert!(on_key(&mut state, &f1, &mut tx), "F1 again closes Help (H-04)");
        assert_eq!(state.help_view(), None);
        assert!(on_key(&mut state, &f1, &mut tx));

        let escape = KeyEvent { key: Key::Escape, mods: rune::Mods::default() };
        assert!(on_key(&mut state, &escape, &mut tx));
        assert_eq!(state.help_view(), None, "Escape closes the full guide");

        let question =
            KeyEvent { key: Key::Text("?".to_owned()), mods: rune::Mods { shift: true, ..rune::Mods::default() } };
        assert!(on_key(&mut state, &question, &mut tx));
        assert_eq!(state.help_view(), Some(crate::window::HelpView::Shortcuts), "`?` keeps the compact list");
    }

    #[test]
    fn a_dirty_document_vetoes_the_close_with_the_save_dialog() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Dirty", "text");
        let doc = state.tab(tab).and_then(crate::window::Tab::document).cloned().expect("doc");
        doc.apply(0..4, "changed");
        state.sync_tab_dirty(&doc);
        let mut tx = Tx::default();
        assert!(close_request(&mut state, &mut tx));
        assert!(matches!(state.prompt(), Some(crate::editor::Prompt::CloseWindow)));
    }

    #[test]
    fn a_clean_window_allows_the_close() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();
        assert!(!close_request(&mut state, &mut tx));
        assert!(state.ui.notice.is_none());
    }

    #[test]
    fn scenes_are_all_reachable_by_name() {
        for (name, scene) in SCENES {
            let temp = TempDir::new().unwrap();
            let mut state = window(&temp);
            scene(&mut state);
            assert!(!name.is_empty());
            assert!(!state.tabs().is_empty(), "scene {name} left no tab");
        }
    }

    #[test]
    fn the_main_window_is_1200_by_860_with_a_480_minimum() {
        // S-12: the spec's logical content size and floor.
        assert_eq!(WINDOW_SIZE, (1200.0, 860.0));
        assert_eq!(WINDOW_MIN_SIZE, (480.0, 480.0));

        let spec = app_spec();
        assert_eq!(spec.size, WINDOW_SIZE);
        assert_eq!(spec.min_size, Some(WINDOW_MIN_SIZE), "Rune's host receives the floor");
    }

    #[test]
    fn a_window_without_a_placement_opens_centered() {
        // S-12: 1920×1080 at scale 1 centres the 1200×860 content.
        assert_eq!(centered_position((0, 0, 1920, 1080), WINDOW_SIZE, 1.0), (360, 110));
        // At scale 2 the content is 2400×1720 on a 2560×1440 display: it fits
        // across but not down, so only the overflowing axis pins to the top.
        assert_eq!(centered_position((0, 0, 2560, 1440), WINDOW_SIZE, 2.0), (80, 0));
        // A second display keeps its own origin.
        assert_eq!(centered_position((1920, 0, 1920, 1080), WINDOW_SIZE, 1.0), (2280, 110));
        assert_eq!(centered_position((0, 0, 1920, 1080), WINDOW_SIZE, 0.0), (360, 110), "a bad scale reads as 1");

        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut windows = startup_states(Rc::clone(&shared), &Launch::default(), None, None);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].frame.position, None, "nothing places it before the display is read");
        pick_launch_position(&mut windows, Some((360, 110)));
        assert_eq!(windows[0].frame.position, Some((360, 110)), "the launch window opens centred");

        let mut placed = startup_states(Rc::clone(&shared), &Launch::default(), None, None);
        placed[0].frame.position = Some((40, 50));
        pick_launch_position(&mut placed, Some((360, 110)));
        assert_eq!(placed[0].frame.position, Some((40, 50)), "a stored placement wins over the centre");
    }

    #[test]
    fn the_startup_factory_declares_the_primary_and_the_headless_second_window() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let path = temp.path().join("second.md");
        std::fs::write(&path, "# Second\n").unwrap();

        let specs = startup_windows(Rc::clone(&shared), Launch::default(), Some(path), None);
        let keys: Vec<String> = specs.iter().map(|spec| spec.key().to_owned()).collect();
        assert_eq!(keys, vec!["window-1".to_owned(), "second".to_owned()]);
        assert_eq!(shared.windows().len(), 2);
        assert_eq!(shared.windows().by_key("second").map(|entry| entry.id), Some(lmdv_core::WindowId(2)));

        drop(specs);
        assert!(shared.windows().is_empty(), "dropping the specs released both window states");
    }

    #[test]
    fn startup_marks_the_session_unclean_and_a_clean_last_close_marks_it_clean() {
        let temp = TempDir::new().unwrap();
        let shared = AppShared::start(test_support::dirs(&temp));
        let specs = startup_windows(Rc::clone(&shared), Launch::default(), None, None);
        assert_eq!(specs.len(), 1);
        assert!(shared.flush_store(std::time::Duration::from_secs(2)));

        let session = load_session(shared.dirs());
        assert!(!session.clean, "the running process leaves a crash marker");
        assert_eq!(session.windows.len(), 1);
        assert!(matches!(
            session.windows[0].tabs.first().map(|tab| &tab.source),
            Some(lmdv_io::store::TabSource::Start)
        ));

        drop(specs);
        let session = load_session(shared.dirs());
        assert!(session.clean, "the last window closing is a clean exit");
        assert_eq!(session.windows.len(), 1);
    }

    #[test]
    fn the_factory_restores_one_window_per_stored_window_with_main_first() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        write_stored_session(&dirs, 2);
        let shared = AppShared::start(dirs);
        let specs = startup_windows(Rc::clone(&shared), Launch::default(), None, None);
        let keys: Vec<String> = specs.iter().map(|spec| spec.key().to_owned()).collect();
        assert_eq!(keys, vec!["main".to_owned(), "window-2".to_owned()]);
        assert_eq!(shared.windows().len(), 2);
        drop(specs);
    }

    #[test]
    fn a_launch_file_skips_the_stored_session_and_opens_the_launch_window() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        write_stored_session(&dirs, 2);
        let shared = AppShared::start(dirs);
        let path = temp.path().join("launch.md");
        std::fs::write(&path, "# Launch\n").unwrap();
        let specs = startup_windows(Rc::clone(&shared), Launch { path: Some(path), ..Launch::default() }, None, None);
        let keys: Vec<String> = specs.iter().map(|spec| spec.key().to_owned()).collect();
        assert_eq!(keys, vec!["window-1".to_owned()], "a CLI file wins over the session");
        drop(specs);
    }

    #[test]
    fn new_window_and_headless_runs_skip_the_stored_session() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        write_stored_session(&dirs, 2);
        let shared = AppShared::start(dirs);
        let specs = startup_windows(Rc::clone(&shared), Launch { new_window: true, ..Launch::default() }, None, None);
        assert_eq!(specs.iter().map(|spec| spec.key().to_owned()).collect::<Vec<_>>(), vec!["window-1"]);
        drop(specs);

        let headless_temp = TempDir::new().unwrap();
        let headless = AppShared::start(test_support::dirs(&headless_temp));
        headless.set_headless();
        let specs = startup_windows(Rc::clone(&headless), Launch::default(), None, None);
        assert_eq!(specs.iter().map(|spec| spec.key().to_owned()).collect::<Vec<_>>(), vec!["window-1"]);
        drop(specs);
        assert!(!headless.dirs().session_path().exists(), "headless tooling never writes a session");
    }

    #[test]
    fn a_future_schema_session_is_never_overwritten() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        std::fs::create_dir_all(&dirs.data).unwrap();
        let future = r#"{"schema": 99, "app": "lmdv", "data": {"clean": true, "windows": []}}"#;
        std::fs::write(dirs.session_path(), future).unwrap();

        let shared = AppShared::start(dirs.clone());
        let path = temp.path().join("launch.md");
        std::fs::write(&path, "# Launch\n").unwrap();
        // A CLI file skips the restore, but the launch still replaces the
        // stored session on close: the ignored file must stay untouched.
        let specs = startup_windows(Rc::clone(&shared), Launch { path: Some(path), ..Launch::default() }, None, None);
        assert!(shared.flush_store(std::time::Duration::from_secs(2)));
        drop(specs);
        assert_eq!(std::fs::read_to_string(dirs.session_path()).unwrap(), future);
    }

    #[test]
    fn restore_session_off_skips_the_stored_session() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        let handle = lmdv_io::store::StoreHandle::start(dirs.clone()).unwrap();
        handle.patch(lmdv_io::store::StorePatch::Prefs(lmdv_core::Prefs {
            restore_session: false,
            ..lmdv_core::Prefs::default()
        }));
        handle.patch(lmdv_io::store::StorePatch::Session(SessionSnapshot {
            clean: true,
            windows: (0..2)
                .map(|_| lmdv_io::store::WindowSession {
                    tabs: vec![lmdv_io::store::TabSession::default()],
                    ..lmdv_io::store::WindowSession::default()
                })
                .collect(),
        }));
        assert!(handle.flush(std::time::Duration::from_secs(2)));
        drop(handle);

        let shared = AppShared::start(dirs);
        let specs = startup_windows(Rc::clone(&shared), Launch::default(), None, None);
        assert_eq!(specs.iter().map(|spec| spec.key().to_owned()).collect::<Vec<_>>(), vec!["window-1"]);
        drop(specs);
    }

    fn write_stored_session(dirs: &AppDirs, windows: usize) {
        let handle = lmdv_io::store::StoreHandle::start(dirs.clone()).unwrap();
        let session = SessionSnapshot {
            clean: true,
            windows: (0..windows)
                .map(|_| lmdv_io::store::WindowSession {
                    tabs: vec![lmdv_io::store::TabSession::default()],
                    ..lmdv_io::store::WindowSession::default()
                })
                .collect(),
        };
        handle.patch(lmdv_io::store::StorePatch::Session(session));
        assert!(handle.flush(std::time::Duration::from_secs(2)));
    }

    #[test]
    fn only_the_router_window_drains_the_launch_queue() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut first = WindowState::new(Rc::clone(&shared));
        let mut second = WindowState::new(Rc::clone(&shared));
        let path = temp.path().join("routed.md");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        shared
            .launch_inbox()
            .push(lmdv_io::instance::LaunchRequest::new(Launch { path: Some(path), ..Launch::default() }));

        let mut tx = Tx::default();
        first.poll(0.0, &mut tx);
        second.poll(0.0, &mut tx);
        assert!(first.active_document_id().is_some(), "the router opened the forwarded launch");
        assert_eq!(second.tabs().len(), 1);
        assert!(second.active_tab().is_some_and(crate::window::Tab::is_start));
    }

    #[test]
    fn ctrl_w_cancels_a_tab_drag_before_it_closes_the_tab() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("One", "# One\n");
        state.open_synthetic("Two", "# Two\n");
        let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(false, 2.0));
        runtime.on_key = Some(on_key);
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);

        let dragged = runtime.state.tabs()[0].id;
        let (x, y) = runtime.find(&format!("t-{}", crate::window::tab_key(dragged))).expect("the tab");
        runtime.pointer_move(x, y);
        runtime.frame(0.0);
        runtime.pointer_down(x, y);
        runtime.frame(0.0);
        runtime.pointer_move(x + 140.0, y);
        runtime.frame(0.0);
        assert!(
            matches!(runtime.state.drag.phase(), rune_kit::shell::TabDragPhase::Dragging(_)),
            "the drag is in hand"
        );

        // T-29: Ctrl+W ends the drag first; the tab is still there.
        let tabs = runtime.state.tabs().len();
        let ctrl_w = rune::KeyEvent {
            key: rune::Key::Command('w'),
            mods: rune::Mods { ctrl: true, cmd: true, ..rune::Mods::default() },
        };
        runtime.key(ctrl_w.clone());
        runtime.frame(0.0);
        assert!(
            matches!(runtime.state.drag.phase(), rune_kit::shell::TabDragPhase::Idle),
            "Ctrl+W cancelled the drag: {:?}",
            runtime.state.drag.phase()
        );
        assert_eq!(runtime.state.tabs().len(), tabs, "the drag took the press, not the tab");

        // The next press closes it.
        runtime.key(ctrl_w);
        runtime.frame(0.0);
        assert_eq!(runtime.state.tabs().len(), tabs - 1, "a second Ctrl+W closes the tab");
    }
}
