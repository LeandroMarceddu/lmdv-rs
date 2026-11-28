//! P6-READ-A: reading keys and smooth motion (R-06–R-09, A-15, OI-156).
//!
//! The pure grammar and pin timing live in `reading_keys.rs`; these tests drive
//! the real view in a `rune::Runtime` at 60 Hz frames, so the glide, the pin's
//! scroll reports and the reduced-motion jumps are the ones a window sees.

use lmdv_render::heading_key;
use rune::{Env, Key, KeyEvent, Mods, Runtime, Tx, WheelEvent};
use tempfile::TempDir;

use crate::nav::{JUMP_INSET, heading_destination};
use crate::reader::{ARTICLE_SCROLL_KEY, ScrollSnapshot, toc_headings};
use crate::reading_keys::{
    DOUBLE_G_SECONDS, KEY_ACCUMULATE_SECONDS, KEY_SCROLL, ReadingKeys, WHEEL_ACCUMULATE_SECONDS,
};
use crate::test_support;
use crate::view::{scene_reader, scene_reader_outline, scene_reader_progress, scene_reader_section, view};
use crate::window::WindowState;

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn open(state: WindowState) -> Runtime<WindowState> {
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    // The AppSpec's hooks: a bare runtime does not run the window's poll or the
    // app's key router by itself.
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.on_key = Some(crate::app::on_key);
    runtime
}

/// `open` with the app's root key preview, as `AppSpec` installs it (O-14).
fn open_preview(state: WindowState) -> Runtime<WindowState> {
    let mut runtime = Runtime::new(state, crate::app::app_view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.on_key = Some(crate::app::on_key);
    runtime
}

fn settle(runtime: &mut Runtime<WindowState>, frames: usize) {
    for _ in 0..frames {
        runtime.frame(1.0 / 60.0);
    }
}

fn key(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn text(value: &str) -> KeyEvent {
    key(Key::Text(value.to_owned()))
}

fn shifted(value: &str) -> KeyEvent {
    KeyEvent { key: Key::Text(value.to_owned()), mods: Mods { shift: true, ..Mods::default() } }
}

fn command(letter: char) -> KeyEvent {
    KeyEvent { key: Key::Command(letter), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } }
}

fn scroll(runtime: &Runtime<WindowState>) -> ScrollSnapshot {
    runtime.state.reading.scroll.get()
}

fn heading_list(runtime: &Runtime<WindowState>) -> Vec<usize> {
    let doc = runtime.state.active_reader().expect("a parsed model");
    toc_headings(&doc).map(|(index, _)| index).collect()
}

fn pinned(runtime: &Runtime<WindowState>) -> Option<String> {
    let doc = runtime.state.active_reader()?;
    runtime.state.pinned_heading(&doc)
}

// ---- j/k (R-06) ----------------------------------------------------------

#[test]
fn five_quick_j_presses_add_up_from_the_target() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let start = scroll(&runtime);
    assert!(start.max > 700.0, "a page to scroll: {start:?}");

    for _ in 0..5 {
        runtime.key(text("j"));
        runtime.frame(1.0 / 60.0);
    }
    settle(&mut runtime, 120);
    let landed = scroll(&runtime);
    let expected = (start.offset + 5.0 * KEY_SCROLL).min(landed.max);
    assert!(
        (landed.offset - expected).abs() < 1.0,
        "five presses are {} points, not {}: {landed:?}",
        expected - start.offset,
        landed.offset
    );
    assert!((expected - start.offset - 400.0).abs() < 1.0, "80 points a press");

    // `k` walks back the same way.
    for _ in 0..5 {
        runtime.key(text("k"));
        runtime.frame(1.0 / 60.0);
    }
    settle(&mut runtime, 120);
    assert!(scroll(&runtime).offset <= start.offset + 1.0);
}

#[test]
fn a_held_key_repeat_jumps_instead_of_gliding() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);

    runtime.key(text("j"));
    runtime.frame(1.0 / 60.0);
    let gliding = scroll(&runtime).offset;
    assert!(gliding > 0.0 && gliding < KEY_SCROLL - 1.0, "the fresh press still glides: {gliding}");

    runtime.repeat_key(text("j"));
    runtime.frame(1.0 / 60.0);
    let repeated = scroll(&runtime).offset;
    assert!((repeated - 2.0 * KEY_SCROLL).abs() < 1.0, "the repeat lands at 160 at once: {repeated}");
    assert!(repeated > gliding + 1.0);
}

// ---- Space (R-07) --------------------------------------------------------

#[test]
fn space_pages_ninety_percent_and_shift_space_pages_back() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let start = scroll(&runtime);
    assert!(start.viewport > 100.0 && start.max > start.viewport, "a page to move: {start:?}");

    runtime.key(text(" "));
    runtime.frame(1.0 / 60.0);
    let down = scroll(&runtime);
    let page = start.viewport * 0.9;
    assert!(
        (down.offset - (start.offset + page).min(start.max)).abs() < 2.0,
        "one page is 90% of the viewport, not two: {} vs {}",
        down.offset,
        start.offset + page
    );

    runtime.key(shifted(" "));
    runtime.frame(1.0 / 60.0);
    let up = scroll(&runtime);
    assert!(up.offset <= start.offset + 1.0, "Shift+Space pages back: {up:?}");

    // Rune owns PageDown/PageUp: one key, one page, never two.
    runtime.key(key(Key::PageDown));
    runtime.frame(1.0 / 60.0);
    let native = scroll(&runtime);
    assert!(
        (native.offset - (up.offset + page).min(native.max)).abs() < 2.0,
        "PageDown pages once through Rune: {} vs {}",
        native.offset,
        up.offset + page
    );
}

#[test]
fn space_still_pages_after_a_click_in_the_article() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let start = scroll(&runtime);

    // A plain click in the reading column starts a (collapsed) text selection.
    runtime.pointer_move(640.0, 400.0);
    runtime.pointer_down(640.0, 400.0);
    runtime.pointer_up(640.0, 400.0);
    runtime.frame(1.0 / 60.0);

    runtime.key(text(" "));
    runtime.frame(1.0 / 60.0);
    let page = scroll(&runtime).offset - start.offset;
    let focused: Vec<_> = runtime.elements().into_iter().filter(|e| e.focused).map(|e| (e.key, e.role)).collect();
    assert!((page - start.viewport * 0.9).abs() < 2.0, "Space still pages the article: {page}, focused {focused:?}");
}

// ---- ./ , (R-08) ---------------------------------------------------------

#[test]
fn dot_visits_every_heading_including_the_last() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let list = heading_list(&runtime);
    assert!(list.len() >= 10, "the reader scene has an outline: {}", list.len());

    let mut visited = Vec::new();
    for _ in 0..list.len() {
        runtime.key(text("."));
        runtime.frame(1.0 / 60.0);
        runtime.frame(1.0 / 60.0);
        visited.push(pinned(&runtime));
    }
    let expected: Vec<Option<String>> = list.iter().map(|index| Some(heading_key(*index))).collect();
    assert_eq!(visited, expected, "`.` steps every outline heading in order");

    // The last heading cannot reach the top: the page clamps at the end.
    settle(&mut runtime, 90);
    let end = scroll(&runtime);
    assert!((end.offset - end.max).abs() < 1.0, "the last heading lands at the end: {end:?}");
    runtime.key(text("."));
    settle(&mut runtime, 3);
    assert!((scroll(&runtime).offset - end.offset).abs() < 1.0, "past the last: no move");
}

#[test]
fn comma_walks_the_previous_headings_and_stops_at_the_first() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let list = heading_list(&runtime);

    for _ in 0..3 {
        runtime.key(text("."));
        runtime.frame(1.0 / 60.0);
        runtime.frame(1.0 / 60.0);
    }
    assert_eq!(pinned(&runtime), Some(heading_key(list[2])));

    runtime.key(text(","));
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
    assert_eq!(pinned(&runtime), Some(heading_key(list[1])));

    runtime.key(text(","));
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
    assert_eq!(pinned(&runtime), Some(heading_key(list[0])));

    settle(&mut runtime, 90);
    let first = scroll(&runtime);
    runtime.key(text(","));
    settle(&mut runtime, 3);
    assert!((scroll(&runtime).offset - first.offset).abs() < 1.0, "nothing before the first: the page stays");
}

#[test]
fn a_step_does_not_record_a_back_place() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    assert!(!runtime.state.nav_can_back());

    runtime.key(text("."));
    settle(&mut runtime, 4);
    assert!(!runtime.state.nav_can_back(), "the original's reading-key steps are not jumps");
}

// ---- g g / G (R-09) ------------------------------------------------------

#[test]
fn a_lone_g_does_nothing_and_g_g_goes_to_the_top() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_progress(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 40);
    let start = scroll(&runtime);
    assert!(start.offset > 300.0, "the scene is inside the page: {start:?}");

    runtime.key(text("g"));
    settle(&mut runtime, 40);
    assert!((scroll(&runtime).offset - start.offset).abs() < 1.0, "a single g does nothing");

    runtime.key(text("g"));
    settle(&mut runtime, 2);
    assert!((scroll(&runtime).offset - start.offset).abs() < 1.0, "the pause's g opened a new window");

    runtime.key(text("g"));
    settle(&mut runtime, 90);
    assert!(scroll(&runtime).offset <= 1.0, "g g reaches the top: {:?}", scroll(&runtime));
}

#[test]
fn g_goes_to_the_end() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let start = scroll(&runtime);

    runtime.key(shifted("g"));
    settle(&mut runtime, 120);
    let end = scroll(&runtime);
    assert!((end.offset - end.max).abs() < 1.0, "G reaches the document's end: {end:?}");
    assert!(end.offset > start.offset + 100.0);
}

// ---- A-15 ----------------------------------------------------------------

#[test]
fn reduced_motion_jumps_instead_of_gliding() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    runtime.set_system_reduced_motion(true);
    runtime.frame(1.0 / 60.0);

    let list = heading_list(&runtime);
    let target = heading_key(*list.last().expect("a heading"));
    let before = scroll(&runtime);
    let top = runtime.state.heading_top(*list.last().expect("a heading")).expect("measured");
    let destination = heading_destination(before, top, JUMP_INSET);

    runtime.update(|state, tx| state.jump_to_heading(&target, tx));
    runtime.frame(1.0 / 60.0);
    let landed = scroll(&runtime);
    assert!((landed.offset - destination).abs() < 1.0, "the jump is a cut: {} vs {destination}", landed.offset);

    runtime.key(text("g"));
    runtime.key(text("g"));
    runtime.frame(1.0 / 60.0);
    assert!(scroll(&runtime).offset <= 1.0, "g g cuts to the top");

    runtime.key(text("G"));
    runtime.frame(1.0 / 60.0);
    let end = scroll(&runtime);
    assert!((end.offset - end.max).abs() < 1.0, "G cuts to the end");
}

// ---- typing focus --------------------------------------------------------

#[test]
fn the_find_field_owns_the_reading_keys() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let start = scroll(&runtime);

    runtime.key(command('f'));
    runtime.frame(1.0 / 60.0);
    assert!(
        runtime.state.active_tab().is_some_and(|tab| tab.view.reader_find.open),
        "Ctrl+F opened the reading find bar"
    );

    for value in ["j", "k", "G", " ", "."] {
        runtime.key(text(value));
        runtime.frame(1.0 / 60.0);
    }
    assert_eq!(scroll(&runtime).offset, start.offset, "the article never moved");
    let query = runtime.state.active_tab().map(|tab| tab.view.reader_find.query.text.clone()).unwrap_or_default();
    assert_eq!(query, "jkG .", "the field typed every character");
}

#[test]
fn the_outline_filter_owns_the_reading_keys() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_outline(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let start = scroll(&runtime);

    runtime.update(|state, tx| {
        tx.focus("outline-filter", false);
        state.outline_filter.replace_text("");
    });
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);

    for value in ["j", "g", " "] {
        runtime.key(text(value));
        runtime.frame(1.0 / 60.0);
    }
    assert_eq!(runtime.state.outline_filter.text, "jg ", "the filter field typed them");
    assert_eq!(scroll(&runtime).offset, start.offset, "the article never moved");
}

// ---- O-14: navigation chords over a focused field -----------------------

#[test]
fn alt_left_navigates_while_a_text_field_has_focus() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open_preview(state);
    settle(&mut runtime, 4);

    // A jump records a place to go back to (O-12); let its glide land first.
    let list = heading_list(&runtime);
    let target = *list.last().expect("a heading");
    runtime.update(|state, tx| state.jump_to_heading(&heading_key(target), tx));
    settle(&mut runtime, 120);
    let jumped = scroll(&runtime).offset;
    assert!(runtime.state.nav_can_back(), "the jump recorded a place");

    // The reading find field takes the keyboard. Without the root preview its
    // own Alt+Left word-move would consume the chord instead of navigating.
    runtime.key(command('f'));
    runtime.frame(1.0 / 60.0);
    let focused = runtime.elements().into_iter().find(|element| element.focused).and_then(|element| element.key);
    assert_eq!(focused.as_deref(), Some(crate::reader_find::READER_FIND_QUERY_KEY), "the field owns the keyboard");
    let before = runtime.state.active_tab().map(|tab| tab.view.reader_find.query.text.clone()).unwrap_or_default();

    runtime.key(KeyEvent { key: Key::Left, mods: Mods { alt: true, ..Mods::default() } });
    settle(&mut runtime, 120);
    assert!(!runtime.state.nav_can_back(), "Back used the place and took the chord");
    assert!(scroll(&runtime).offset < jumped - 100.0, "the article went back: {jumped} -> {:?}", scroll(&runtime));
    let after = runtime.state.active_tab().map(|tab| tab.view.reader_find.query.text.clone()).unwrap_or_default();
    assert_eq!(after, before, "the find field never word-moved");
}

// ---- O-04: the pin and the glide ----------------------------------------

#[test]
fn an_outline_jump_glides_and_still_records_the_place_left() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);

    let list = heading_list(&runtime);
    let target = *list.last().expect("a heading");
    let key = heading_key(target);
    let before = scroll(&runtime);
    let top = runtime.state.heading_top(target).expect("the last frame measured it");
    let destination = heading_destination(before, top, JUMP_INSET);
    assert!(destination > before.offset + 100.0, "a jump worth gliding: {destination}");

    runtime.update(|state, tx| state.jump_to_heading(&key, tx));
    runtime.frame(1.0 / 60.0);
    let mid = scroll(&runtime).offset;
    assert!(mid > before.offset, "the glide started");
    assert!(mid < destination - 1.0, "the first frame is still on its way: {mid} vs {destination}");
    assert_eq!(pinned(&runtime), Some(key.clone()), "the jumped heading is pinned while it lands");

    settle(&mut runtime, 120);
    let landed = scroll(&runtime).offset;
    assert!((landed - destination).abs() < 2.0, "landed at {landed}, wanted {destination}");
    assert!(runtime.state.nav_can_back(), "an outline jump records the place left (O-12)");
}

#[test]
fn an_anchor_jump_glides_and_records_back() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);

    let doc = runtime.state.active_reader().expect("a model");
    let target = toc_headings(&doc).last().map(|(index, _)| index).expect("a heading");
    let fragment = doc.headings.get(target).map(|heading| heading.id.clone()).expect("an id");
    let before = scroll(&runtime);
    let top = runtime.state.heading_top(target).expect("measured");
    let destination = heading_destination(before, top, JUMP_INSET);

    runtime.update(|state, tx| state.open_link(&format!("#{fragment}"), tx));
    runtime.frame(1.0 / 60.0);
    assert!(scroll(&runtime).offset > before.offset, "the anchor glides");
    assert!(scroll(&runtime).offset < destination - 1.0, "not a snap");
    settle(&mut runtime, 120);
    assert!((scroll(&runtime).offset - destination).abs() < 2.0);
    assert!(runtime.state.nav_can_back(), "L-01's anchor records the place left");
}

#[test]
fn the_outline_keeps_the_heading_until_the_reader_scrolls() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_outline(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let list = heading_list(&runtime);
    let key = heading_key(list[3]);

    runtime.update(|state, tx| state.jump_to_heading(&key, tx));
    settle(&mut runtime, 3);
    assert_eq!(pinned(&runtime), Some(key.clone()), "the pin holds through the glide");
    let row = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(format!("outline-{key}").as_str()))
        .expect("the outline row");
    assert!(row.item_selected, "the outline shows the heading it is flying to");

    // A wheel is unmistakably the reader's: the pin goes and the spy resumes.
    runtime.wheel_event(WheelEvent::new(0.0, 120.0).at(400.0, 400.0));
    settle(&mut runtime, 6);
    assert_eq!(pinned(&runtime), None, "a wheel releases the pin (O-04)");
}

// ---- per-window state ----------------------------------------------------

#[test]
fn a_tab_switch_a_new_document_and_edit_mode_forget_the_keys() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("One", "# One\n\n## Two\n\nBody\n");
    let second = state.open_synthetic("Two", "# Other\n\nBody\n");
    assert!(state.activate_tab(first), "the first tab comes forward");
    let doc = state.active_reader().expect("a model");

    let mut tx = Tx::default();
    state.reading.scroll.set(ScrollSnapshot { offset: 100.0, max: 1000.0, viewport: 700.0 });
    assert!(state.reading_key(&text("j"), &mut tx));
    assert_eq!(state.reading_keys.target.map(|target| target.target), Some(180.0));
    assert!(state.reading_key(&text("g"), &mut tx));
    assert!(state.reading_keys.last_g.is_some());
    state.scroll_to_heading(&heading_key(1), &mut tx);
    assert!(state.reading_keys.pin.is_some());
    assert_eq!(state.pinned_heading(&doc), Some(heading_key(1)));

    assert!(state.activate_tab(second));
    assert!(state.reading_keys.target.is_none(), "a tab switch drops the j/k target");
    assert!(state.reading_keys.last_g.is_none(), "and the g g window");
    assert!(state.reading_keys.pin.is_none(), "and the pin");

    // A new document replaces the place, even inside one tab.
    assert!(state.activate_tab(first));
    assert!(state.reading_key(&text("j"), &mut tx));
    assert!(state.reading_keys.target.is_some());
    let third = state.open_synthetic("Three", "# Three\n\nBody\n");
    assert!(state.reading_keys.target.is_none(), "a new document resets the keys");
    assert!(state.activate_tab(third));

    // Entering edit mode is not reading.
    let doc = state.active_reader().expect("a model");
    state.scroll_to_heading(&heading_key(0), &mut tx);
    assert!(state.reading_keys.pin.is_some());
    assert!(state.enter_editor(None, &mut tx), "the synthetic document can be edited");
    assert!(state.reading_keys.pin.is_none(), "edit mode resets the keys");
    assert!(!state.reading_key(&text("j"), &mut tx), "and reading keys do nothing there");
    assert!(!doc.headings.is_empty());
}

#[test]
fn a_scene_reset_forgets_the_keys() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "# One\n\n## Two\n\nBody\n");
    let mut tx = Tx::default();
    state.scroll_to_heading(&heading_key(1), &mut tx);
    assert!(state.reading_keys.pin.is_some());
    state.reset_to_start_page();
    assert!(state.reading_keys.pin.is_none());
    assert!(state.reading_keys.target.is_none());
    assert!(state.reading_keys.last_g.is_none());
}

#[test]
fn a_modal_owns_the_keys() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "# One\n\nBody\n");
    let mut tx = Tx::default();
    assert!(state.reading_key(&text("j"), &mut tx));
    state.open_help(crate::window::HelpView::Shortcuts);
    assert!(!state.reading_key(&text("j"), &mut tx), "the help sheet holds the keyboard");
    state.close_help();
    assert!(state.reading_key(&text("j"), &mut tx));
}

#[test]
fn the_key_target_starts_over_after_the_window_expires() {
    let mut keys = ReadingKeys::default();
    keys.note_scroll(120.0, 0.0);
    assert_eq!(keys.scroll_from(KEY_ACCUMULATE_SECONDS - 0.001, 20.0), 120.0);
    assert_eq!(keys.scroll_from(KEY_ACCUMULATE_SECONDS, 20.0), 20.0, "the page is the start again");
    assert_eq!(keys.scroll_from(DOUBLE_G_SECONDS, 20.0), 20.0);
}

// ---- landing geometry ----------------------------------------------------

#[test]
fn a_heading_jump_under_the_section_line_holds_the_line_away() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 60);
    assert!(runtime.state.section_line.visible(), "the line is on screen before the jump");

    let doc = runtime.state.active_reader().expect("a model");
    let target = doc.headings.iter().position(|heading| heading.text == "Reading positions").expect("the h2");
    let key = heading_key(target);
    let before = scroll(&runtime);
    let top = runtime.state.heading_top(target).expect("the last frame measured the heading");

    runtime.update(|state, tx| state.jump_to_heading(&key, tx));
    assert!(runtime.state.section_line.holding(), "O-11: a heading jump steps the line aside, no reserve");
    settle(&mut runtime, 90);

    let destination = heading_destination(before, top, JUMP_INSET);
    assert!((scroll(&runtime).offset - destination).abs() < 2.0, "the heading lands at the plain inset");
    let elements = runtime.elements();
    let viewport = elements
        .iter()
        .find(|element| element.key.as_deref() == Some(ARTICLE_SCROLL_KEY))
        .expect("the article viewport");
    let heading =
        elements.iter().find(|element| element.key.as_deref() == Some(key.as_str())).expect("the landed heading");
    let landed_top = heading.rect[1] - viewport.rect[1];
    assert!(landed_top < crate::section::SECTION_LINE_HEIGHT, "the heading sits where the line would be: {landed_top}");
    assert!(!runtime.state.section_line.visible(), "the heading owns the top band, so the line is not painted over it");
}

#[test]
fn a_heading_lands_an_inset_below_the_top_and_clamps_to_the_document() {
    let scroll = ScrollSnapshot { offset: 200.0, max: 1000.0, viewport: 700.0 };
    assert_eq!(JUMP_INSET, 24.0, "L-01's top margin");
    assert_eq!(heading_destination(scroll, 40.0, JUMP_INSET), 216.0);
    assert_eq!(heading_destination(scroll, -500.0, JUMP_INSET), 0.0);
    assert_eq!(heading_destination(scroll, 2000.0, JUMP_INSET), 1000.0);
}

// ---- notched wheel smoothing --------------------------------------------

#[test]
fn a_notched_wheel_glides_and_accumulates_from_its_target() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let start = scroll(&runtime);

    runtime.wheel_event(WheelEvent::new(0.0, -40.0).at(600.0, 400.0));
    runtime.frame(1.0 / 60.0);
    assert_eq!(
        runtime.state.reading_keys.target.map(|target| target.target),
        Some(start.offset + 40.0),
        "the notch is a glide request, not a native jump"
    );
    let mid = scroll(&runtime).offset;
    assert!(mid > start.offset && mid < start.offset + 40.0, "the wheel glides: {mid}");

    runtime.wheel_event(WheelEvent::new(0.0, -40.0).at(600.0, 400.0));
    runtime.frame(1.0 / 60.0);
    assert_eq!(
        runtime.state.reading_keys.target.map(|target| target.target),
        Some(start.offset + 80.0),
        "the second notch adds from the first target"
    );
    settle(&mut runtime, 120);
    assert!((scroll(&runtime).offset - (start.offset + 80.0)).abs() < 2.0);
}

#[test]
fn a_trackpad_wheel_stays_runes() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let start = scroll(&runtime);

    runtime.wheel_event(WheelEvent::new(0.0, -40.0).at(600.0, 400.0).precise(true));
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.reading_keys.target.is_none(), "precise deltas are not smoothed by the app");
    assert!(scroll(&runtime).offset > start.offset, "Rune scrolled it natively");
}

#[test]
fn a_modified_or_horizontal_wheel_is_left_to_rune() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Note", "# One\n\nBody\n");
    let mut tx = Tx::default();
    let ctrl = WheelEvent::new(0.0, -40.0).mods(Mods { ctrl: true, ..Mods::default() });
    assert!(!state.article_wheel(&ctrl, &mut tx), "Ctrl+wheel is a zoom/pinch gesture");
    assert!(!state.article_wheel(&WheelEvent::new(0.0, -40.0).precise(true), &mut tx));
    assert!(!state.article_wheel(&WheelEvent::new(30.0, 0.0), &mut tx), "horizontal");
    assert!(!state.article_wheel(&WheelEvent::new(0.0, 0.0), &mut tx), "nothing moved");
    assert!(state.article_wheel(&WheelEvent::new(0.0, -40.0), &mut tx));
    assert!(state.reading_keys.target.is_some());
    state.enter_editor(None, &mut tx);
    assert!(!state.article_wheel(&WheelEvent::new(0.0, -40.0), &mut tx), "not a reading surface");
}

#[test]
fn a_wheel_releases_the_heading_pin_and_starts_a_new_target() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let list = heading_list(&runtime);
    let key = heading_key(list[3]);
    runtime.update(|state, tx| state.jump_to_heading(&key, tx));
    settle(&mut runtime, 2);
    assert_eq!(pinned(&runtime), Some(key), "the jump's pin is on");
    assert!(runtime.state.section_line.holding(), "the line waits for the jump");
    runtime.key(text("j"));
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.reading_keys.target.is_some());
    assert!(!runtime.state.section_line.holding(), "a reading key takes the page from the jump");

    // A pause longer than the wheel window makes the next notch a new gesture.
    settle(&mut runtime, (WHEEL_ACCUMULATE_SECONDS * 60.0) as usize + 2);
    runtime.wheel_event(WheelEvent::new(0.0, -40.0).at(600.0, 400.0));
    runtime.frame(1.0 / 60.0);
    assert_eq!(pinned(&runtime), None, "the wheel is the reader's own (O-04)");
    let target = runtime.state.reading_keys.target.expect("a wheel target").target;
    let page = scroll(&runtime).offset;
    assert!((target - (page + 40.0)).abs() < 2.0, "a fresh wheel starts from the page, not the key target");
}

#[test]
fn question_mark_toggles_the_keys_overlay_in_the_reading_view() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let question = KeyEvent { key: Key::Text("?".to_owned()), mods: Mods { shift: true, ..Mods::default() } };
    runtime.key(question.clone());
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.help_view(), Some(crate::window::HelpView::Shortcuts), "`?` opens the overlay (R-11)");
    runtime.key(question);
    settle(&mut runtime, 2);
    assert!(runtime.state.help_view().is_none(), "`?` again closes it");
}

#[test]
fn the_keys_overlay_traps_focus_and_ignores_reading_keys() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    runtime.key(shifted("?"));
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.help_view(), Some(crate::window::HelpView::Shortcuts), "`?` opens it (R-11)");

    // Rune's modal stack opens on the dialog's content and keeps Tab inside it.
    let focused = |rt: &Runtime<WindowState>| {
        rt.elements().into_iter().find(|element| element.focused).and_then(|element| element.key)
    };
    assert_eq!(focused(&runtime).as_deref(), Some("close"), "focus lands on the dialog's Close button");
    for _ in 0..6 {
        runtime.key(key(Key::Tab));
        settle(&mut runtime, 1);
        assert_eq!(focused(&runtime).as_deref(), Some("close"), "Tab never leaves the dialog");
    }

    // R-11: “while open, most keys are ignored”. The reading keys must not move
    // the article behind the scrim.
    let start = scroll(&runtime).offset;
    for _ in 0..5 {
        runtime.key(text("j"));
    }
    settle(&mut runtime, 8);
    assert_eq!(scroll(&runtime).offset, start, "`j` is ignored while the overlay is open");

    // Nor may a window command run behind it: Ctrl+T would add a tab and
    // Ctrl+Q would start the quit sequence.
    let tabs = runtime.state.tabs().len();
    runtime.key(command('t'));
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.tabs().len(), tabs, "Ctrl+T is ignored");
    runtime.key(command('q'));
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.tabs().len(), tabs, "Ctrl+Q is ignored");
    assert!(runtime.state.prompt().is_none(), "no close or quit prompt opened");
    assert_eq!(runtime.state.help_view(), Some(crate::window::HelpView::Shortcuts), "the overlay stays");

    // Escape closes it and the article gets its keys back.
    runtime.key(key(Key::Escape));
    settle(&mut runtime, 2);
    assert!(runtime.state.help_view().is_none(), "Esc closes the overlay");
    runtime.key(text("j"));
    settle(&mut runtime, 8);
    assert!(scroll(&runtime).offset > start, "the reading keys work again after closing");
}

#[test]
fn the_keys_overlay_closes_on_a_backdrop_click_and_returns_focus() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    runtime.key(shifted("?"));
    settle(&mut runtime, 2);
    runtime.pointer_move(5.0, 5.0);
    runtime.pointer_down(5.0, 5.0);
    runtime.pointer_up(5.0, 5.0);
    settle(&mut runtime, 2);
    assert!(runtime.state.help_view().is_none(), "a click outside the dialog closes it");

    let article = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(ARTICLE_SCROLL_KEY))
        .expect("the article");
    assert!(
        !runtime.elements().iter().any(|element| element.focused && element.key.as_deref() == Some("close")),
        "the dialog's focus is gone"
    );
    assert!(article.rect[3] > 0.0);
}

#[test]
fn a_fast_wheel_burst_keeps_the_article_and_lands_on_its_target() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.reset_to_start_page();
    let mut text = String::from("# Burst\n\n");
    for section in 0..60 {
        text.push_str(&format!("## Section {section}\n\nA paragraph to scroll past.\n\n- one\n- two\n\n"));
    }
    state.open_synthetic("Burst", &text);
    state.left.set_open(false);
    state.right.set_open(false);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let start = scroll(&runtime);

    // Sixty notches at once: the article is reused frame after frame (it has no
    // picture or diagram), and every heading is still drawn after the burst.
    for _ in 0..60 {
        runtime.wheel_event(WheelEvent::new(0.0, -53.0).at(600.0, 400.0));
        runtime.frame(1.0 / 60.0);
    }
    settle(&mut runtime, 60);
    let headless = runtime.state.active_reader().expect("a model").headings.len();
    assert!(headless > 0);
    let elements = runtime.elements();
    let headings = elements.iter().filter(|element| element.role == "heading").count();
    assert!(headings > 0, "the article is still drawn after the burst");
    assert!(elements.iter().any(|element| element.key.as_deref() == Some(crate::reader::ARTICLE_KEY)));
    let landed = scroll(&runtime);
    assert!(landed.offset > start.offset, "the burst scrolled the page: {start:?} -> {landed:?}");
    assert!((landed.offset - (start.offset + 60.0 * 53.0)).abs() < 3.0, "and landed where the wheel asked");
}

#[test]
fn opening_the_front_matter_card_builds_the_article_again() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.reset_to_start_page();
    state.open_synthetic("Front matter", "---\ntitle: Hello\n---\n\n# Page\n\nBody.\n");
    state.left.set_open(false);
    state.right.set_open(false);
    let mut runtime = open(state);
    settle(&mut runtime, 6);
    let has_entries = |runtime: &Runtime<WindowState>| {
        runtime.elements().iter().any(|element| element.key.as_deref() == Some("entries") && !element.exiting)
    };
    assert!(!has_entries(&runtime), "the card starts closed");

    // The reused article must still be built again for an open card: the card's
    // table is app state the build reads, not a scroll.
    runtime.update(|state: &mut WindowState, _tx| {
        state.active_tab_mut().expect("a tab").view.details_open.insert(lmdv_render::FRONT_MATTER_KEY.to_owned());
    });
    runtime.frame(1.0 / 60.0);
    assert!(has_entries(&runtime), "the open card's table is drawn");

    runtime.update(|state: &mut WindowState, _tx| {
        state.active_tab_mut().expect("a tab").view.details_open.clear();
    });
    runtime.frame(1.0 / 60.0);
    assert!(!has_entries(&runtime), "and closing it takes the table away again");
}

#[test]
fn the_keys_overlay_holds_the_window_keys_on_the_start_page_too() {
    // R-11: “works even with no document”; the modal's key hold applies there
    // as well, where no article owns the reading keys.
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.reset_to_start_page();
    let mut runtime = open(state);
    settle(&mut runtime, 2);
    runtime.key(shifted("?"));
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.help_view(), Some(crate::window::HelpView::Shortcuts));

    let tabs = runtime.state.tabs().len();
    runtime.key(command('t'));
    runtime.key(command('p'));
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.tabs().len(), tabs, "Ctrl+T is ignored");
    assert!(!runtime.state.share.busy(), "Ctrl+P did not start a print");
    assert_eq!(runtime.state.help_view(), Some(crate::window::HelpView::Shortcuts));

    runtime.key(key(Key::Escape));
    settle(&mut runtime, 2);
    assert!(runtime.state.help_view().is_none(), "Esc still closes it");
}
