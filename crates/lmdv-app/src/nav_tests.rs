//! Back/Forward history tests (O-12–O-15).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lmdv_core::{DocStatus, NavHistory, Place, ScrollAnchor, TabId};
use rune::{Env, Key, KeyEvent, Mods, PointerButton, Runtime, Tx};
use tempfile::TempDir;

use crate::reader::{ScrollSnapshot, ViewportAnchor};
use crate::section::MeasuredHeading;
use crate::test_support;
use crate::window::{Tab, WindowState};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn poll_until(state: &mut WindowState, what: &str, mut predicate: impl FnMut(&WindowState) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut tx = Tx::default();
    while !predicate(state) {
        assert!(Instant::now() < deadline, "{what} never happened");
        state.poll(0.0, &mut tx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn ready(state: &WindowState) -> bool {
    state.active_tab().and_then(Tab::document).is_some_and(|doc| matches!(*doc.status.borrow(), DocStatus::Ready))
}

fn parsed(state: &WindowState) -> bool {
    state.active_tab().is_some_and(|tab| tab.view.reader.is_some())
}

/// Writes a Markdown file and opens it, waiting for its load and parse.
fn open_file(state: &mut WindowState, temp: &TempDir, name: &str, text: &str) -> (PathBuf, TabId) {
    let path = temp.path().join(name);
    std::fs::write(&path, text).unwrap();
    let tab = state.open_path(path.clone());
    poll_until(state, "the file load", ready);
    poll_until(state, "the first parse", parsed);
    (path, tab)
}

/// Puts the reading state where a reader would be: scrolled `offset` points
/// under heading `heading` (index into the model's headings).
fn place_reader_at(state: &mut WindowState, offset: f32, heading: Option<usize>) {
    let snapshot = ScrollSnapshot { offset, max: 4000.0, viewport: 700.0 };
    state.reading.scroll.set(snapshot);
    state.reading.last = Some(snapshot);
    if let Some(heading) = heading {
        state.set_viewport_anchor(ViewportAnchor { heading: Some(heading), offset: 24.0, top: false });
    } else {
        state.set_viewport_anchor(ViewportAnchor::default());
    }
}

fn nav_of(state: &WindowState, tab: TabId) -> &NavHistory {
    &state.tab(tab).expect("the tab").view.nav
}

fn pasted_key(state: &WindowState, tab: TabId) -> String {
    WindowState::place_key(state.tab(tab).and_then(Tab::document).expect("a document")).expect("a key")
}

// ---- recording (O-12) ----------------------------------------------------

#[test]
fn opening_another_document_records_the_place_left() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let (first, first_tab) = open_file(&mut state, &temp, "a.md", "# A\n\nbody\n");
    assert!(nav_of(&state, first_tab).back.is_empty(), "the first document leaves nothing behind");
    place_reader_at(&mut state, 640.0, Some(0));

    let (_, second_tab) = open_file(&mut state, &temp, "b.md", "# B\n");
    let back = &nav_of(&state, second_tab).back;
    assert_eq!(back.len(), 1, "opening B leaves A for Back");
    assert_eq!(back[0].path, first.to_string_lossy());
    assert_eq!(back[0].pos.id, "a", "the reading anchor's id is the heading slug");
    assert_eq!(back[0].pos.offset, 24.0);
    assert!(state.nav_can_back());
}

#[test]
fn opening_the_same_document_records_nothing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let (path, tab) = open_file(&mut state, &temp, "a.md", "# A\n");
    place_reader_at(&mut state, 120.0, None);
    state.open_path(path);
    assert!(nav_of(&state, tab).back.is_empty(), "the tab did not move");
}

#[test]
fn a_local_link_records_the_place_left_and_opens_the_target() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let (_, first_tab) = open_file(&mut state, &temp, "index.md", "# Index\n\n[next](next.md)\n");
    std::fs::write(temp.path().join("next.md"), "# Next\n").unwrap();
    place_reader_at(&mut state, 300.0, None);

    let mut tx = Tx::default();
    state.open_link("next.md", &mut tx);
    poll_until(&mut state, "the linked document", ready);
    poll_until(&mut state, "the linked parse", parsed);

    let target = state.active_tab().expect("a tab").id;
    assert_ne!(target, first_tab);
    let back = &nav_of(&state, target).back;
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].path, temp.path().join("index.md").to_string_lossy());
    assert_eq!(back[0].pos.offset, 300.0);
}

#[test]
fn a_file_link_with_a_section_lands_on_that_heading() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file(&mut state, &temp, "index.md", "# Index\n\n[part](other.md#part)\n");
    std::fs::write(temp.path().join("other.md"), "# Other\n\n## Part\n\ntext\n").unwrap();

    let mut tx = Tx::default();
    state.open_link("other.md#part", &mut tx);
    poll_until(&mut state, "the linked document", ready);
    poll_until(&mut state, "the heading landing", |state| {
        state.active_tab().is_some_and(|tab| tab.view.reader.is_some() && tab.view.fragment.is_none())
    });
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the model");
    assert_eq!(model.resolve_heading("part").map(|index| model.headings[index].text.as_str()), Some("Part"));
}

#[test]
fn a_same_document_anchor_link_jumps_and_records_the_place_left() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# One\n\ntext\n\n## Two\n\nmore\n");
    place_reader_at(&mut state, 500.0, Some(0));

    let mut tx = Tx::default();
    state.open_link("#two", &mut tx);
    let back = &nav_of(&state, tab).back;
    assert_eq!(back.len(), 1, "the place left goes on Back");
    assert_eq!(back[0].pos.id, "one", "the anchor is the heading above the viewport");
    assert_eq!(back[0].pos.offset, 24.0);
}

#[test]
fn a_same_document_anchor_that_names_nothing_records_nothing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# One\n");
    place_reader_at(&mut state, 500.0, Some(0));
    let mut tx = Tx::default();
    state.open_link("#missing", &mut tx);
    assert!(state.ui.notice.is_some(), "the missing heading is reported");
    assert!(nav_of(&state, tab).back.is_empty());
}

#[test]
fn an_outline_jump_records_the_place_left() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Long", "# One\n\nfirst\n\n## Two\n\nsecond\n");
    place_reader_at(&mut state, 900.0, Some(1));
    state.set_heading_tops(vec![MeasuredHeading { index: 0, top: -2000.0, bottom: -1970.0 }]);

    let mut tx = Tx::default();
    state.jump_to_heading("h0", &mut tx);
    let back = &nav_of(&state, tab).back;
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].pos.id, "two", "the section with its distance above the top");
    assert_eq!(back[0].pos.offset, 24.0);
}

#[test]
fn a_jump_that_lands_within_eight_points_records_nothing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Long", "# One\n\nfirst\n\n## Two\n\nsecond\n");
    place_reader_at(&mut state, 900.0, Some(0));
    // The heading sits 14 points below the viewport top: the jump to its
    // 12-point landing would move the page 2 points.
    state.set_heading_tops(vec![MeasuredHeading { index: 0, top: 14.0, bottom: 44.0 }]);
    let mut tx = Tx::default();
    state.jump_to_heading("h0", &mut tx);
    assert!(nav_of(&state, tab).back.is_empty(), "a two-point jump is not a place");

    state.set_heading_tops(vec![MeasuredHeading { index: 0, top: 60.0, bottom: 90.0 }]);
    let mut tx = Tx::default();
    state.jump_to_heading("h0", &mut tx);
    assert_eq!(nav_of(&state, tab).back.len(), 1, "a 48-point jump is a place");
}

#[test]
fn a_jump_to_the_top_of_a_finished_page_is_not_recorded() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Long", "# One\n\nfirst\n");
    place_reader_at(&mut state, 4000.0, Some(0));
    state.set_heading_tops(vec![MeasuredHeading { index: 0, top: 3990.0, bottom: 4020.0 }]);
    let mut tx = Tx::default();
    state.jump_to_heading("h0", &mut tx);
    assert!(nav_of(&state, tab).back.is_empty(), "the page cannot scroll toward it");
}

#[test]
fn a_reload_records_nothing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let (_, tab) = open_file(&mut state, &temp, "a.md", "# A\n\nbody\n");
    place_reader_at(&mut state, 240.0, None);
    let mut tx = Tx::default();
    state.request_reload(&mut tx);
    poll_until(&mut state, "the reload", ready);
    assert!(nav_of(&state, tab).back.is_empty(), "a reload is not a navigation");
}

#[test]
fn a_new_place_clears_forward() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# One\n");
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(Place::new("pasted:1", ScrollAnchor::top()));
        tab.view.nav.commit_back(Some(Place::new("pasted:2", ScrollAnchor::top())));
        assert!(tab.view.nav.can_forward());
        tab.view.nav.record(Place::new("pasted:3", ScrollAnchor::top()));
        assert!(!tab.view.nav.can_forward(), "a new place clears Forward");
    }
}

#[test]
fn the_back_list_is_capped_at_fifty_places() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# One\n");
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        for index in 0..60 {
            tab.view.nav.record(Place::new(format!("pasted:{index}"), ScrollAnchor::top()));
        }
        assert_eq!(tab.view.nav.back.len(), lmdv_core::NAV_HISTORY_MAX);
        assert_eq!(tab.view.nav.back.first().map(|place| place.path.as_str()), Some("pasted:10"));
    }
}

#[test]
fn consecutive_places_under_four_points_are_suppressed() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# One\n");
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        let anchor = |offset| ScrollAnchor { id: "h1".to_owned(), offset };
        tab.view.nav.record(Place::new("/a.md", anchor(100.0)));
        tab.view.nav.record(Place::new("/a.md", anchor(101.5)));
        assert_eq!(tab.view.nav.back.len(), 1, "less than 4 points apart is the same place");
        tab.view.nav.record(Place::new("/a.md", anchor(104.5)));
        assert_eq!(tab.view.nav.back.len(), 2);
    }
}

// ---- travel (O-13/O-14) --------------------------------------------------

#[test]
fn back_activates_the_already_open_tab_and_swaps_the_place() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "# First\n");
    let second = state.open_synthetic("Second", "# Second\n");
    let first_key = pasted_key(&state, first);
    let first_place = Place::new(first_key, ScrollAnchor { id: "first".to_owned(), offset: 36.0 });
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == second) {
        tab.view.nav.record(first_place.clone());
    }
    place_reader_at(&mut state, 220.0, None);

    let mut tx = Tx::default();
    state.nav_back(&mut tx);
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(first), "the first document's tab comes forward");
    assert!(nav_of(&state, second).back.is_empty(), "the place was consumed");
    let forward = &nav_of(&state, second).forward;
    assert_eq!(forward.len(), 1);
    assert_eq!(forward[0].path, pasted_key(&state, second));
    assert_eq!(
        nav_of(&state, first),
        nav_of(&state, second),
        "the journey follows the reader into the tab it landed on"
    );
    assert!(nav_of(&state, first).back.is_empty(), "Back itself is never recorded as a new place");
    assert_eq!(state.tab(first).map(|tab| tab.view.anchor.offset), Some(36.0), "the stored anchor is restored");
    assert!(state.reading.pending.is_none(), "the ready model takes the restore at once");
}

#[test]
fn back_and_forward_restore_the_anchor_in_the_other_tab() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "# First\n");
    let second = state.open_synthetic("Second", "# Second\n");
    let first_key = pasted_key(&state, first);
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == second) {
        tab.view.nav.record(Place::new(first_key, ScrollAnchor { id: "first".to_owned(), offset: 36.0 }));
    }
    place_reader_at(&mut state, 220.0, None);
    let mut tx = Tx::default();
    state.nav_back(&mut tx);

    // Forward on the landing tab returns to where the reader was.
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(first));
    let mut tx = Tx::default();
    state.nav_forward(&mut tx);
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(second));
    assert_eq!(nav_of(&state, second).forward.len(), 0);
    assert_eq!(nav_of(&state, second).back.len(), 1, "Forward put the place back on Back");
    assert_eq!(state.tab(second).map(|tab| tab.view.anchor.offset), Some(220.0));
}

#[test]
fn back_opens_a_document_that_is_not_open_yet() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let _first = state.open_synthetic("First", "# First\n");
    let second = state.open_synthetic("Second", "# Second\n");
    let target = temp.path().join("elsewhere.md");
    std::fs::write(&target, "# Elsewhere\n\nbody\n").unwrap();
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == second) {
        tab.view
            .nav
            .record(Place::new(target.to_string_lossy(), ScrollAnchor { id: "elsewhere".to_owned(), offset: 12.0 }));
    }
    place_reader_at(&mut state, 180.0, None);
    let mut tx = Tx::default();
    state.nav_back(&mut tx);
    assert!(state.navigation.pending.is_some(), "the move waits for the load");
    poll_until(&mut state, "the opened document", ready);
    poll_until(&mut state, "the opened parse", parsed);

    assert_eq!(state.active_path().as_deref(), Some(target.as_path()));
    assert!(nav_of(&state, second).back.is_empty(), "the move consumed its place");
    assert_eq!(nav_of(&state, second).forward.len(), 1);
    assert_eq!(nav_of(&state, second).forward[0].pos.offset, 180.0, "the place left is kept for Forward");
    assert!(state.navigation.pending.is_none());
}

#[test]
fn a_missing_file_is_dropped_with_a_not_found_notice() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let (_, tab) = open_file(&mut state, &temp, "a.md", "# A\n");
    let missing = temp.path().join("deleted.md");
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(Place::new(missing.to_string_lossy(), ScrollAnchor::top()));
    }
    let tabs = state.tabs().len();
    let mut tx = Tx::default();
    state.nav_back(&mut tx);
    poll_until(&mut state, "the not-found notice", |state| state.ui.notice.is_some());

    let notice = state.ui.notice.as_ref().expect("a notice");
    assert!(notice.text.contains("Not found"), "{}", notice.text);
    assert!(notice.text.contains("deleted.md"), "{}", notice.text);
    assert!(nav_of(&state, tab).back.is_empty(), "the place is dropped");
    assert_eq!(state.tabs().len(), tabs, "the error card the move opened is closed again");
    assert!(state.navigation.pending.is_none());
}

#[test]
fn a_pasted_place_without_its_document_is_dropped_silently() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let (_, tab) = open_file(&mut state, &temp, "a.md", "# A\n");
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(Place::new("pasted:987654", ScrollAnchor::top()));
    }
    let tabs = state.tabs().len();
    let mut tx = Tx::default();
    state.nav_back(&mut tx);
    assert!(nav_of(&state, tab).back.is_empty(), "the place is gone");
    assert!(state.ui.notice.is_none(), "nothing on disk to report");
    assert_eq!(state.tabs().len(), tabs);
}

#[test]
fn back_over_a_start_page_opens_the_document_without_a_forward_place() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let (path, _) = open_file(&mut state, &temp, "a.md", "# A\n");
    state.new_start_tab();
    let start = state.active_tab().expect("the start page").id;
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == start) {
        tab.view.nav.record(Place::new(path.to_string_lossy(), ScrollAnchor::top()));
    }
    let mut tx = Tx::default();
    state.nav_back(&mut tx);
    assert_eq!(state.active_path().as_deref(), Some(path.as_path()));
    assert!(nav_of(&state, start).forward.is_empty(), "nothing was on screen to keep");
    assert!(nav_of(&state, start).back.is_empty());
}

// ---- inputs (O-14) -------------------------------------------------------

fn alt(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { alt: true, ..Mods::default() } }
}

#[test]
fn alt_left_and_right_dispatch_the_navigation_commands() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# Note\n");
    let key = pasted_key(&state, tab);
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(Place::new(key, ScrollAnchor { id: String::new(), offset: 320.0 }));
    }
    let mut tx = Tx::default();
    assert!(crate::app::on_key(&mut state, &alt(Key::Left), &mut tx), "Alt+Left is Back");
    assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(320.0));
    assert!(nav_of(&state, tab).can_forward());

    let mut tx = Tx::default();
    assert!(crate::app::on_key(&mut state, &alt(Key::Right), &mut tx), "Alt+Right is Forward");
    assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(0.0), "back at the top");
    assert!(nav_of(&state, tab).can_back());
}

#[test]
fn alt_left_does_nothing_without_a_place() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Note", "# Note\n");
    let mut tx = Tx::default();
    assert!(!crate::app::on_key(&mut state, &alt(Key::Left), &mut tx), "the command is disabled without history");
    assert!(!crate::key_matches(&alt(Key::Left), "Ctrl+Left"));
    let shift = KeyEvent { key: Key::Left, mods: Mods { alt: true, shift: true, ..Mods::default() } };
    assert!(!crate::key_matches(&shift, "Alt+Left"), "Alt+Shift+Left is a text command, not navigation");
}

#[test]
fn the_mouse_side_buttons_navigate() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# Note\n");
    let key = pasted_key(&state, tab);
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(Place::new(key, ScrollAnchor { id: String::new(), offset: 90.0 }));
    }
    let mut tx = Tx::default();
    state.nav_pointer_button(PointerButton::Back, &mut tx);
    assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(90.0));
    let mut tx = Tx::default();
    state.nav_pointer_button(PointerButton::Forward, &mut tx);
    assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(0.0));
    let mut tx = Tx::default();
    state.nav_pointer_button(PointerButton::Middle, &mut tx);
    assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(0.0), "another button does nothing");
}

#[test]
fn the_tab_menu_starts_with_back_and_forward() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Note", "# Note\n");
    if let Some(tab) = state.active_tab() {
        state.show_tab_menu(tab.id, rune::PointerPress { x: 40.0, y: 20.0 });
    }
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    let labels: Vec<String> = runtime
        .elements()
        .into_iter()
        .filter_map(|element| element.label)
        .filter(|label| label == lmdv_i18n::t!("ctx.back") || label == lmdv_i18n::t!("ctx.forward"))
        .collect();
    assert!(labels.iter().any(|label| label == lmdv_i18n::t!("ctx.back")), "{labels:?}");
    assert!(labels.iter().any(|label| label == lmdv_i18n::t!("ctx.forward")), "{labels:?}");
}

// ---- palette and session (O-15) -----------------------------------------

#[test]
fn the_palette_offers_back_and_forward_and_closes_on_the_move() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# Note\n");
    let key = pasted_key(&state, tab);
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(Place::new(key, ScrollAnchor { id: String::new(), offset: 240.0 }));
    }
    let mut tx = Tx::default();
    state.open_palette(Some(crate::palette::PaletteChord::CtrlK), &mut tx);
    state.set_palette_query(">back", &mut tx);
    let row = (0..state.palette.len())
        .find(|index| state.palette.entry(*index).is_some_and(|entry| entry.key == "c:nav.back"))
        .expect("Back is a palette command");
    let mut tx = Tx::default();
    state.palette_pick(row, crate::palette::PaletteWhere::Here, &mut tx);
    assert!(!state.palette_is_open(), "the palette closes on a navigation (C-06)");
    assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(240.0));
    assert_eq!(state.ui.last_command.as_deref(), Some("nav.back"));
}

#[test]
fn back_scrolls_the_article_to_the_remembered_offset() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let long = state.open_synthetic("Long", crate::view::READER_MARKDOWN);
    let other = state.open_synthetic("Other", "# Other\n");
    let key = pasted_key(&state, long);
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == other) {
        tab.view.nav.record(Place::new(key, ScrollAnchor { id: String::new(), offset: 420.0 }));
    }
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.update(|state, tx| state.nav_back(tx));
    runtime.frame(0.0);
    runtime.update(|state, tx| state.apply_pending_scroll(tx));
    runtime.frame(0.0);
    let article = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(crate::reader::ARTICLE_KEY))
        .expect("the article column");
    assert!((article.scrolled[1] - 420.0).abs() < 2.0, "scrolled to {}", article.scrolled[1]);
}

#[test]
fn an_overtaken_move_is_not_committed() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "# First\n");
    let second = state.open_synthetic("Second", "# Second\n");
    let opened = state.open_synthetic("Opened by the move", "# Opened\n");
    let first_key = pasted_key(&state, first);
    let second_key = pasted_key(&state, second);
    let place = Place::new(first_key, ScrollAnchor { id: "first".to_owned(), offset: 30.0 });
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == second) {
        tab.view.nav.record(place.clone());
    }
    let doc = state.tab(opened).and_then(Tab::document_id);
    state.navigation.pending = Some(crate::nav::PendingTravel {
        dir: -1,
        place,
        here: Some(Place::new(second_key, ScrollAnchor::top())),
        origin: Some(second),
        tab: Some(opened),
        doc,
    });
    // Another document came first.
    assert!(state.activate_tab(first));
    let mut tx = Tx::default();
    state.finish_travel(doc.expect("a document"), true, None, &mut tx);
    assert_eq!(nav_of(&state, second).back.len(), 1, "the place is still there to try again");
    assert!(nav_of(&state, opened).forward.is_empty(), "an overtaken move records nothing");
    assert!(state.navigation.pending.is_none());
}

#[test]
fn back_and_forward_never_light_the_landing_highlight() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# Note\n");
    let key = pasted_key(&state, tab);
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(Place::new(key, ScrollAnchor { id: "note".to_owned(), offset: 40.0 }));
    }
    let mut tx = Tx::default();
    state.nav_back(&mut tx);
    assert!(state.reading.line_flash.is_none(), "a place is not a --line landing");
    let mut tx = Tx::default();
    state.nav_forward(&mut tx);
    assert!(state.reading.line_flash.is_none());
}

#[test]
fn the_browser_open_path_records_its_place() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let (_, first_tab) = open_file(&mut state, &temp, "one.md", "# One\n");
    place_reader_at(&mut state, 64.0, None);
    let second = temp.path().join("two.md");
    std::fs::write(&second, "# Two\n").unwrap();
    let mut tx = Tx::default();
    state.open_browser_file(second, None, Mods::default(), &mut tx);
    poll_until(&mut state, "the browser's file", ready);
    let opened = state.active_tab().expect("a tab").id;
    assert_ne!(opened, first_tab);
    assert_eq!(nav_of(&state, opened).back.len(), 1, "the file pane is an open like any other");
    assert_eq!(nav_of(&state, opened).back[0].pos.offset, 64.0);
}

#[test]
fn a_tab_switch_keeps_each_tabs_history() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "# First\n");
    let second = state.open_synthetic("Second", "# Second\n");
    let first_key = pasted_key(&state, first);
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == second) {
        tab.view.nav.record(Place::new(first_key, ScrollAnchor::top()));
    }
    assert!(state.activate_tab(first));
    assert!(state.activate_tab(second));
    assert_eq!(nav_of(&state, second).back.len(), 1, "the place stayed with its tab");
    assert!(nav_of(&state, first).back.is_empty());
}

#[test]
fn a_session_round_trip_keeps_each_tabs_history() {
    let temp = TempDir::new().unwrap();
    let shared = test_support::shared(&temp);
    let mut state = WindowState::new_keyed(shared.clone(), shared.allocate_window(), "main".to_owned());
    let tab = state.open_synthetic("Kept", "# Kept\n");
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(Place::new("/notes/left.md", ScrollAnchor { id: "intro".to_owned(), offset: 18.0 }));
        tab.view.nav.record(Place::new("/notes/another.md", ScrollAnchor::top()));
        tab.view.nav.commit_back(Some(Place::new("pasted:7", ScrollAnchor::top())));
    }
    assert!(state.publish_session());
    let session = shared.session_snapshot(false);
    let captured = &session.windows[0].tabs[0];
    assert_eq!(captured.nav.back.len(), 1);
    assert_eq!(captured.nav.forward.len(), 1);
    assert_eq!(captured.nav.back[0].pos.id, "intro");

    let json = serde_json::to_string(&session).unwrap();
    let parsed: lmdv_io::store::SessionSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.windows[0].tabs[0].nav, captured.nav, "the lists survive the file");

    // A session written before the history fields existed still reads.
    let old = r#"{"clean":true,"windows":[{"tabs":[{"source":{"kind":"file","path":"/notes/a.md"},"anchor":{"id":"x","offset":4.0},"line":null,"mode":"viewer"}],"active":0}]}"#;
    let old: lmdv_io::store::SessionSnapshot = serde_json::from_str(old).expect("an old session reads");
    assert!(old.windows[0].tabs[0].nav.back.is_empty());
    assert!(old.windows[0].tabs[0].nav.forward.is_empty());
}

#[test]
fn history_places_follow_a_renamed_file() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Note", "# Note\n");
    let from = PathBuf::from("/notes/old.md");
    let to = PathBuf::from("/notes/new.md");
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(Place::new(from.to_string_lossy(), ScrollAnchor::top()));
    }
    state.rename_history_paths(&from, &to);
    assert_eq!(nav_of(&state, tab).back[0].path, "/notes/new.md");
}

#[test]
fn a_place_key_names_files_and_pasted_text() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let (path, tab) = open_file(&mut state, &temp, "a.md", "# A\n");
    let key = state.tab(tab).and_then(Tab::document).and_then(|document| WindowState::place_key(document));
    assert_eq!(key.as_deref(), Some(path.to_string_lossy().as_ref()));
    let pasted = state.open_synthetic("Pasted", "text\n");
    let key = pasted_key(&state, pasted);
    assert!(key.starts_with("pasted:"), "{key}");
    assert!(!Path::new(&key).is_absolute());
}
