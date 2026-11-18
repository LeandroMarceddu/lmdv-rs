//! P4-02c: X11 tab tear-out — the pure placement/hit-test helpers, and the
//! release path through the two-window transfer protocol.
//!
//! The strip itself is wired in `view.rs`; these tests drive the same handlers
//! `TabStrip::on_tear_move`/`on_tear_out` call. Rune reports no `screen_point`
//! headless or on Wayland, so the strip's tear-out path cannot run in this
//! gate; the handler tests pass the point Rune would report on X11.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use lmdv_core::{Document, DocumentKey, SourceBuffer, TabId, WindowId};
use rune::WindowPlacement;
use rune_kit::shell::{TabTearMove, TabTearOut};
use tempfile::TempDir;

use crate::shared::AppShared;
use crate::tear_out::{placement_contains, point_in_window, tear_out_position, tear_out_spec};
use crate::test_support;
use crate::window::{Tab, WindowState, tab_key};
use crate::windows::{OpenedWindow, WindowOpenRequest, window_key};

fn shared(temp: &TempDir) -> Rc<AppShared> {
    let shared = test_support::shared(temp);
    shared.set_headless();
    shared
}

/// Two registered windows on one shared state.
fn two_windows(temp: &TempDir) -> (Rc<AppShared>, WindowState, WindowState) {
    let shared = shared(temp);
    let first = WindowState::new(Rc::clone(&shared));
    let second = WindowState::new(Rc::clone(&shared));
    (shared, first, second)
}

fn ready_file(shared: &AppShared, path: &Path, text: &str) -> Rc<Document> {
    let key = DocumentKey::File(path.to_path_buf());
    let title = path.file_name().and_then(|name| name.to_str()).unwrap_or("file");
    let id = shared.docs_mut().allocate_id();
    let document = Rc::new(Document::ready(id, key, title, SourceBuffer::new(text)));
    shared.docs_mut().insert_ready(Rc::clone(&document));
    document
}

fn poll(state: &mut WindowState) {
    let mut tx = rune::Tx::default();
    state.poll(0.0, &mut tx);
}

/// A window frame and display, in physical pixels: outer top-left at `(x, y)`,
/// 4/28 points of decoration, and a 2560×1440 display it sits on.
fn placement(x: i32, y: i32, width: u32, height: u32, scale: f64) -> WindowPlacement {
    WindowPlacement {
        position: (x, y),
        content_position: (x + 4, y + 28),
        content_size: (width, height),
        scale,
        display: Some(((0, 0), (2560, 1440))),
        ..WindowPlacement::default()
    }
}

fn place(shared: &AppShared, id: WindowId, x: i32, y: i32, width: u32, height: u32, scale: f64) {
    assert!(shared.set_window_placement(id, placement(x, y, width, height, scale)));
}

fn tear_move(tab: TabId, index: usize, grab: [f32; 2], screen: (i32, i32)) -> TabTearMove {
    TabTearMove { id: tab_key(tab), index, torn: true, grab, window_point: [180.0, 20.0], screen_point: Some(screen) }
}

fn tear_out(tab: TabId, index: usize, screen: Option<(i32, i32)>) -> TabTearOut {
    TabTearOut {
        id: tab_key(tab),
        index,
        offset: [120.0, -80.0],
        window_point: Some([180.0, 20.0]),
        screen_point: screen,
    }
}

#[test]
fn tear_out_is_off_where_screen_points_do_not_exist() {
    assert!(tear_out_spec(false).is_none(), "Wayland and headless keep the menu substitute");
    let spec = tear_out_spec(true).expect("X11 tears out");
    assert_eq!(spec.detach_distance, 36.0, "T-20's 36 px");
    assert_eq!(spec.reattach_distance, 24.0);
    assert!(!spec.lone, "T-21's lone-tab window drag stays a menu substitute");
}

#[test]
fn a_headless_window_leaves_its_strip_without_tear_out() {
    let temp = TempDir::new().unwrap();
    let shared = shared(&temp);
    let state = WindowState::new(shared);
    assert!(state.tear_out_spec().is_none());
}

#[test]
fn a_release_without_a_source_placement_uses_the_pointer_and_grab() {
    assert_eq!(tear_out_position((500, 400), [20.0, 10.0], None, (1100, 760)), (480, 390));
}

#[test]
fn the_source_scale_and_decorations_shift_the_new_frame() {
    let source = WindowPlacement {
        position: (100, 100),
        content_position: (104, 130),
        content_size: (900, 600),
        scale: 2.0,
        ..WindowPlacement::default()
    };
    // 500 - 20*2 - 4 = 456; 400 - 10*2 - 30 = 350.
    assert_eq!(tear_out_position((500, 400), [20.0, 10.0], Some(source), (1100, 760)), (456, 350));
}

#[test]
fn a_release_near_an_edge_clamps_to_the_source_display() {
    let source = WindowPlacement {
        position: (0, 0),
        content_position: (4, 28),
        content_size: (1900, 1000),
        scale: 1.0,
        display: Some(((0, 0), (1920, 1080))),
        ..WindowPlacement::default()
    };
    // 1920 - 1100 = 820 across; 1080 - 760 = 320 down.
    assert_eq!(tear_out_position((1900, 1000), [0.0, 0.0], Some(source), (1100, 760)), (820, 320));
}

#[test]
fn a_display_to_the_right_keeps_the_frame_on_it() {
    let source = WindowPlacement {
        position: (1920, 0),
        content_position: (1924, 28),
        content_size: (1800, 1000),
        scale: 1.0,
        display: Some(((1920, 0), (1920, 1080))),
        ..WindowPlacement::default()
    };
    assert_eq!(tear_out_position((100, 100), [0.0, 0.0], Some(source), (1100, 760)), (1920, 72));
}

#[test]
fn a_window_larger_than_the_display_opens_at_its_origin() {
    let source = WindowPlacement {
        position: (0, 0),
        content_position: (4, 28),
        content_size: (700, 500),
        scale: 1.0,
        display: Some(((0, 0), (800, 600))),
        ..WindowPlacement::default()
    };
    assert_eq!(tear_out_position((700, 500), [0.0, 0.0], Some(source), (1100, 760)), (0, 0));
}

#[test]
fn the_hit_test_covers_the_decorated_frame_and_stops_at_its_edges() {
    let source = placement(100, 100, 800, 600, 1.0);
    assert!(placement_contains(source, (100, 100)), "the outer top-left is inside");
    assert!(placement_contains(source, (907, 755)), "the last frame pixel is inside");
    assert!(!placement_contains(source, (908, 700)), "past the right decoration");
    assert!(!placement_contains(source, (500, 756)), "past the bottom decoration");
    assert!(!placement_contains(source, (99, 200)), "left of the frame");
}

#[test]
fn a_window_point_is_content_relative_and_scaled() {
    let source = WindowPlacement {
        position: (100, 100),
        content_position: (104, 128),
        content_size: (800, 600),
        scale: 2.0,
        ..WindowPlacement::default()
    };
    assert_eq!(point_in_window(source, (504, 328)), [200.0, 100.0]);
    assert_eq!(point_in_window(source, (204, 108)), [50.0, -10.0], "a point in the titlebar is above the content");
}

#[test]
fn the_window_under_a_point_is_the_topmost_open_one_but_not_the_source() {
    let temp = TempDir::new().unwrap();
    let (shared, source, target) = two_windows(&temp);
    place(&shared, source.id(), 100, 100, 800, 600, 1.0);
    place(&shared, target.id(), 900, 100, 800, 600, 1.0);

    assert_eq!(shared.window_at((1000, 200), Some(source.id())), Some(target.id()));
    assert_eq!(shared.window_at((1000, 200), None), Some(target.id()));
    assert_eq!(shared.window_at((200, 200), None), Some(source.id()));
    assert_eq!(shared.window_at((200, 200), Some(source.id())), None, "a tab never lands back on its source");
    assert_eq!(shared.window_at((800, 200), Some(source.id())), None, "between the windows");

    shared.set_window_closing(target.id(), true);
    assert_eq!(shared.window_at((1000, 200), Some(source.id())), None, "a closing window accepts nothing (T-25)");
}

#[test]
fn a_torn_tab_hover_opens_and_clears_the_target_gap() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let tab = source.open_synthetic("Hover", "# Hover\n");
    source.sync_registry();
    target.sync_registry();
    place(&shared, source.id(), 100, 100, 800, 600, 1.0);
    place(&shared, target.id(), 900, 100, 800, 600, 1.0);

    assert!(source.handle_tear_move(tear_move(tab, 1, [12.0, 8.0], (1000, 200))));
    poll(&mut target);
    assert_eq!(
        target.transfer.hover().map(|hover| hover.point),
        Some([96.0, 72.0]),
        "the target's content point, not the desktop's"
    );

    // The pointer leaves every window: the gap closes.
    assert!(source.handle_tear_move(tear_move(tab, 1, [12.0, 8.0], (850, 800))));
    poll(&mut target);
    assert!(target.transfer.hover().is_none());
}

#[test]
fn cancelling_a_tear_closes_the_target_gap_on_the_next_poll() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let tab = source.open_synthetic("Cancel", "# Cancel\n");
    source.sync_registry();
    target.sync_registry();
    place(&shared, source.id(), 100, 100, 800, 600, 1.0);
    place(&shared, target.id(), 900, 100, 800, 600, 1.0);

    assert!(source.handle_tear_move(tear_move(tab, 1, [12.0, 8.0], (1000, 200))));
    poll(&mut target);
    assert!(target.transfer.hover().is_some());

    // Escape (or a lost pointer) ends the strip's gesture without a release;
    // the source's poll sees no torn tab and clears the target's gap.
    source.drag.cancel();
    poll(&mut source);
    poll(&mut target);
    assert!(target.transfer.hover().is_none());
}

#[test]
fn a_release_over_another_window_moves_the_tab_there() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let document = ready_file(&shared, &temp.path().join("moved.md"), "# Moved\n");
    let tab = source.attach_document(Rc::clone(&document));
    source.new_start_tab();
    source.activate_tab(tab);
    source.sync_registry();
    target.sync_registry();
    place(&shared, source.id(), 100, 100, 800, 600, 1.0);
    place(&shared, target.id(), 900, 100, 800, 600, 1.0);
    let screen = (1000, 200);

    assert!(source.handle_tear_move(tear_move(tab, 0, [12.0, 8.0], screen)));
    poll(&mut target);
    assert!(target.transfer.hover().is_some());

    assert!(source.handle_tear_out(tear_out(tab, 0, Some(screen))));
    assert!(source.tab(tab).is_none(), "the source detached the tab");
    assert_eq!(source.tabs().len(), 1, "a start page takes its place");
    assert!(source.active_tab().is_some_and(Tab::is_start));

    poll(&mut target);
    poll(&mut source);
    assert_eq!(target.active_document_id(), Some(document.id));
    assert_eq!(source.transfer.pending_len(), 0);
    assert_eq!(shared.windows().len(), 2, "no window was opened");
    assert!(target.transfer.hover().is_none(), "adoption cleared the incoming gap");
}

#[test]
fn a_release_without_a_screen_point_keeps_the_tab() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, target) = two_windows(&temp);
    let tab = source.open_synthetic("Stays", "# Stays\n");
    source.sync_registry();
    target.sync_registry();
    place(&shared, source.id(), 100, 100, 800, 600, 1.0);
    place(&shared, target.id(), 900, 100, 800, 600, 1.0);

    assert!(!source.handle_tear_out(tear_out(tab, 1, None)), "Wayland/headless: nothing to place");
    assert!(source.tab(tab).is_some());
    assert_eq!(source.transfer.pending_len(), 0);
    assert!(!source.handle_tear_move(TabTearMove {
        id: tab_key(tab),
        index: 1,
        torn: true,
        grab: [4.0, 4.0],
        window_point: [20.0, 20.0],
        screen_point: None,
    }));
}

#[test]
fn a_lone_tab_never_tears_out() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, target) = two_windows(&temp);
    let tab = source.open_synthetic("Only", "# Only\n");
    source.sync_registry();
    place(&shared, source.id(), 100, 100, 800, 600, 1.0);
    place(&shared, target.id(), 900, 100, 800, 600, 1.0);

    assert!(!source.handle_tear_out(tear_out(tab, 1, Some((1000, 200)))));
    assert!(source.tab(tab).is_some(), "the only tab stays (TearOutSpec::lone is false)");
    assert_eq!(source.tabs().len(), 1);
    assert_eq!(source.transfer.pending_len(), 0);
}

thread_local! {
    static OPENED: RefCell<Vec<WindowOpenRequest>> = const { RefCell::new(Vec::new()) };
    static OPEN_TARGET: RefCell<Option<WindowState>> = const { RefCell::new(None) };
}

fn target_opener(shared: &Rc<AppShared>, request: WindowOpenRequest) -> Result<OpenedWindow, String> {
    OPENED.with(|opened| opened.borrow_mut().push(request));
    let id = shared.allocate_window();
    let state = WindowState::new_keyed(Rc::clone(shared), id, window_key(id));
    OPEN_TARGET.with(|target| *target.borrow_mut() = Some(state));
    Ok(OpenedWindow { id })
}

fn reset_opener(shared: &AppShared) {
    OPENED.with(|opened| opened.borrow_mut().clear());
    OPEN_TARGET.with(|target| *target.borrow_mut() = None);
    shared.set_window_opener(target_opener);
}

#[test]
fn a_drop_outside_every_window_opens_one_at_the_pointer_and_adopts_the_tab() {
    let temp = TempDir::new().unwrap();
    let shared = shared(&temp);
    let mut source = WindowState::new(Rc::clone(&shared));
    let document = ready_file(&shared, &temp.path().join("torn.md"), "# Torn\n");
    let tab = source.attach_document(Rc::clone(&document));
    source.new_start_tab();
    source.activate_tab(tab);
    source.sync_registry();
    place(&shared, source.id(), 100, 100, 1100, 760, 1.0);
    reset_opener(&shared);

    let screen = (500, 400);
    let _ = source.handle_tear_move(tear_move(tab, 0, [20.0, 10.0], screen));
    assert!(source.handle_tear_out(tear_out(tab, 0, Some(screen))));
    assert!(source.tab(tab).is_none(), "the source detached the tab");
    assert!(source.active_tab().is_some_and(Tab::is_start));

    let opened = OPENED.with(|opened| opened.borrow().clone());
    assert_eq!(opened.len(), 1);
    assert_eq!(
        opened[0].position,
        Some((476, 362)),
        "500 - 20 - 4 across, 400 - 10 - 28 down: the grabbed point under the pointer"
    );
    let mut target = OPEN_TARGET.with(|target| target.borrow_mut().take()).expect("the opener built a window");

    poll(&mut target);
    poll(&mut source);
    assert_eq!(target.tabs().len(), 1, "the adopted tab replaced the new window's start page");
    assert_eq!(target.active_document_id(), Some(document.id));
    assert_eq!(source.transfer.pending_len(), 0);
    assert_eq!(shared.windows().len(), 2);
}

#[test]
fn a_drop_without_moves_uses_the_release_point_with_no_grab() {
    let temp = TempDir::new().unwrap();
    let shared = shared(&temp);
    let mut source = WindowState::new(Rc::clone(&shared));
    let document = ready_file(&shared, &temp.path().join("fallback.md"), "# Fallback\n");
    let tab = source.attach_document(Rc::clone(&document));
    source.new_start_tab();
    source.activate_tab(tab);
    source.sync_registry();
    place(&shared, source.id(), 100, 100, 1100, 760, 1.0);
    reset_opener(&shared);

    assert!(source.handle_tear_out(tear_out(tab, 0, Some((500, 400)))));
    let opened = OPENED.with(|opened| opened.borrow().clone());
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].position, Some((496, 372)), "no stored grab, decorations still apply");
}

#[test]
fn a_drop_over_the_second_window_does_not_open_another() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let tab = source.open_synthetic("Moved", "# Moved\n");
    let document = source.tab(tab).and_then(Tab::document).cloned().expect("document");
    source.new_start_tab();
    source.activate_tab(tab);
    source.sync_registry();
    target.sync_registry();
    place(&shared, source.id(), 100, 100, 800, 600, 1.0);
    place(&shared, target.id(), 900, 100, 800, 600, 1.0);
    reset_opener(&shared);

    assert!(source.handle_tear_out(tear_out(tab, 0, Some((1000, 200)))));
    poll(&mut target);
    poll(&mut source);
    assert!(OPENED.with(|opened| opened.borrow().is_empty()), "the transfer is not a new window");
    assert_eq!(target.active_document_id(), Some(document.id));
    assert_eq!(shared.windows().len(), 2);
}

#[test]
fn a_dropped_dirty_tab_keeps_its_text_and_lease_in_the_new_window() {
    let temp = TempDir::new().unwrap();
    let shared = shared(&temp);
    let mut source = WindowState::new(Rc::clone(&shared));
    let path = temp.path().join("dirty.md");
    let document = ready_file(&shared, &path, "base\n");
    let tab = source.attach_document(Rc::clone(&document));
    source.new_start_tab();
    source.activate_tab(tab);
    document.apply(0..0, "edited ");
    source.sync_tab_dirty(&document);
    source.sync_registry();
    place(&shared, source.id(), 100, 100, 1100, 760, 1.0);
    reset_opener(&shared);

    assert!(source.handle_tear_out(tear_out(tab, 0, Some((500, 400)))));
    let mut target = OPEN_TARGET.with(|target| target.borrow_mut().take()).expect("a window");
    poll(&mut target);
    poll(&mut source);
    assert_eq!(document.text().text(), "edited base\n", "the shared buffer travelled");
    assert!(target.tab(target.active_tab().map(|tab| tab.id).expect("tab")).is_some_and(|tab| tab.view.dirty));
    assert!(document.is_dirty());
    assert_eq!(document.subscribers.borrow().len(), 1);
    assert!(document.subscribers.borrow().contains(&target.id()));
}

#[test]
fn a_refused_drop_returns_the_tab_to_the_source() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("shared.md");
    let document = ready_file(&shared, &path, "base\n");
    let moved = source.attach_document(Rc::clone(&document));
    source.open_synthetic("Other", "# Other\n");
    target.attach_document(Rc::clone(&document));
    document.apply(0..0, "edited ");
    source.sync_tab_dirty(&document);
    target.sync_tab_dirty(&document);
    source.sync_registry();
    target.sync_registry();
    place(&shared, source.id(), 100, 100, 800, 600, 1.0);
    place(&shared, target.id(), 900, 100, 800, 600, 1.0);

    // T-22: the same file is dirty in both windows, so the target refuses the
    // duplicate and the P4-02b protocol hands the tab back with its text.
    assert!(source.handle_tear_out(tear_out(moved, 1, Some((1000, 200)))));
    assert!(source.tab(moved).is_none(), "the source detached it for the offer");
    poll(&mut target);
    poll(&mut source);
    assert!(source.tab(moved).is_some(), "the refused tab came back");
    assert_eq!(document.text().text(), "edited base\n");
    assert_eq!(document.subscribers.borrow().len(), 2);
    assert_eq!(source.transfer.pending_len(), 0);
}
