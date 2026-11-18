//! P6-S integration: the section line, the progress hairline and the tab's
//! reading time, drawn by the real view in a `rune::Runtime` (R-03–R-05,
//! O-07–O-10, T-01/T-16/T-35).
//!
//! Frames are advanced on the virtual clock (1/60 s), which is what lets the
//! 120 ms settle and the 250 ms hold expire exactly as they do in a window.

use lmdv_core::ScrollAnchor;
use rune::{ElementInfo, Key, Runtime};
use tempfile::TempDir;

use crate::section::{PROGRESS_FILL_KEY, PROGRESS_KEY, SECTION_LINE_KEY, SECTION_SEGMENT_PREFIX, SECTION_SHIELD_KEY};
use crate::test_support;
use crate::view::{READER_MARKDOWN, scene_reader, scene_reader_progress, scene_reader_section, scene_tabs, view};
use crate::window::{Mode, WindowState};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn open(state: WindowState) -> Runtime<WindowState> {
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    // The AppSpec's hook: a bare runtime does not run the window's poll by itself.
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime
}

fn settle(runtime: &mut Runtime<WindowState>, frames: usize) {
    for _ in 0..frames {
        runtime.frame(1.0 / 60.0);
    }
}

fn press(runtime: &mut Runtime<WindowState>, x: f32, y: f32) {
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(1.0 / 60.0);
}

fn has_key(elements: &[ElementInfo], key: &str) -> bool {
    elements.iter().any(|element| element.key.as_deref() == Some(key))
}

fn element<'a>(elements: &'a [ElementInfo], key: &str) -> Option<&'a ElementInfo> {
    elements.iter().find(|element| element.key.as_deref() == Some(key))
}

fn tab_text(elements: &[ElementInfo], needle: &str) -> bool {
    elements
        .iter()
        .filter(|element| element.role == "text")
        .any(|element| element.text.as_deref().is_some_and(|text| text.contains(needle)))
}

#[test]
fn the_hairline_follows_the_scroll_and_takes_no_layout() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_progress(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 30);

    let elements = runtime.elements();
    let line = element(&elements, PROGRESS_KEY).expect("the progress hairline");
    let fill = element(&elements, PROGRESS_FILL_KEY).expect("its filled part");
    assert!((line.rect[3] - 2.0).abs() < 0.01, "the hairline is 2 points tall");
    let share = fill.rect[2] / line.rect[2];
    assert!(share > 0.1 && share < 1.0, "part way through the page: {share}");
    let scroll = runtime.state.reading.scroll.get();
    assert!(scroll.offset > 24.0 && scroll.max > 24.0, "the scene really scrolled: {scroll:?}");
    let expected = (scroll.offset / scroll.max).clamp(0.0, 1.0);
    assert!((share - expected).abs() < 0.02, "width follows offset/max: {share} vs {expected}");

    // The article still starts at the top: the overlay took no layout.
    let article = element(&elements, "article-scroll").expect("the article viewport");
    assert!(article.rect[1] > 0.0);
}

#[test]
fn a_short_document_and_the_top_of_a_long_one_have_no_hairline() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    // `scene_reader` says the remembered anchor is the top, so nothing scrolled.
    let mut runtime = open(state);
    settle(&mut runtime, 3);
    let elements = runtime.elements();
    assert!(!has_key(&elements, PROGRESS_KEY), "at the top the width would be zero");

    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Short", "# Tiny\n\nOne line.\n");
    let mut runtime = open(state);
    settle(&mut runtime, 3);
    assert!(!has_key(&runtime.elements(), PROGRESS_KEY), "a document with nowhere to scroll");
}

#[test]
fn the_section_line_names_the_heading_that_scrolled_out() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    let mut runtime = open(state);
    // 120 ms settle plus 120 ms after the restore's fast scroll.
    settle(&mut runtime, 40);

    let elements = runtime.elements();
    assert!(has_key(&elements, SECTION_LINE_KEY), "the path is held at the top");
    assert!(has_key(&elements, &format!("{SECTION_SEGMENT_PREFIX}0")));
    let parts: Vec<String> = elements
        .iter()
        .filter(|element| element.path.contains("section-seg-"))
        .filter_map(|element| element.text.clone())
        .collect();
    assert!(!parts.is_empty(), "the parts draw their names");
    assert!(
        parts.iter().any(|part| part == "A first section" || part == "What this page shows"),
        "a real heading path: {parts:?}"
    );
    // Not a Tab stop: the parts are plain spans, as O-10 asks.
    for index in 0..4 {
        let key = format!("{SECTION_SEGMENT_PREFIX}{index}");
        if let Some(segment) = element(&elements, &key) {
            assert!(!segment.focusable, "{key} must not be tabbable");
        }
    }
    let scroll = runtime.state.reading.scroll.get();
    assert!(scroll.offset > 100.0, "the scene is inside the page: {scroll:?}");
}

#[test]
fn the_line_stays_away_at_the_top_while_the_outline_shows_and_while_editing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 5);
    assert!(!has_key(&runtime.elements(), SECTION_LINE_KEY), "a heading owns the top band at the top");

    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    state.right.set_open(true);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = false;
    }
    let mut runtime = open(state);
    settle(&mut runtime, 40);
    assert!(!has_key(&runtime.elements(), SECTION_LINE_KEY), "the outline is in sight");

    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.mode = Mode::Editor;
    }
    state.section_line.reset();
    let mut runtime = open(state);
    settle(&mut runtime, 30);
    let elements = runtime.elements();
    assert!(!has_key(&elements, SECTION_LINE_KEY), "the editor changes the view");
    assert!(!has_key(&elements, PROGRESS_KEY), "and the hairline is a reading surface");
}

#[test]
fn clicking_a_part_jumps_like_the_outline_and_the_second_press_is_swallowed() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 40);

    let elements = runtime.elements();
    let segment = element(&elements, &format!("{SECTION_SEGMENT_PREFIX}0")).expect("a part").rect;
    let point = (segment[0] + segment[2] / 2.0, segment[1] + segment[3] / 2.0);
    let before = runtime.state.reading.scroll.get();
    press(&mut runtime, point.0, point.1);
    settle(&mut runtime, 3);
    let after = runtime.state.reading.scroll.get();
    assert!(after.offset < before.offset - 20.0, "the jump went up: {} then {}", before.offset, after.offset);
    assert!(runtime.state.section_line.swallowing(runtime.state.ui.last_now), "the 500 ms guard is armed");
    assert!(has_key(&runtime.elements(), SECTION_SHIELD_KEY), "the guard covers the line's old place");

    // The second press lands where the line was: the guard swallows it, so the
    // page keeps settling to the first jump's heading and no text under the line
    // starts selecting. The jump glides now (OI-156), so the offset is still
    // moving; what matters is that it never turns back to the text below.
    settle(&mut runtime, 22);
    let landed = runtime.state.reading.scroll.get().offset;
    press(&mut runtime, point.0, point.1);
    settle(&mut runtime, 3);
    let settling = runtime.state.reading.scroll.get().offset;
    assert!(settling <= landed + 0.5, "one jump for a double click: {landed} then {settling}");
    assert!(runtime.state.section_line.swallowing(runtime.state.ui.last_now), "the guard still holds");
    assert!(
        !runtime.elements().iter().any(|element| element.selected.is_some()),
        "the second press did not start selecting the text under the line"
    );

    // After the hold the line waits for the page and comes back.
    settle(&mut runtime, 40);
    let state = &runtime.state;
    assert!(state.ui.last_now > 1.0);
    // The jumped-to heading owns the top band, so the line is hidden there; that
    // is the same rule an outline click obeys (O-09).
    assert!(!has_key(&runtime.elements(), SECTION_LINE_KEY));
}

#[test]
fn a_heading_jump_holds_the_line_and_flashes_the_heading_once_it_lands() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let ink = runtime.state.current_appearance(false).theme.highlight_landing;
    let doc = runtime.state.active_reader().expect("a model");
    let target = crate::reader::toc_headings(&doc).nth(3).map(|(index, _)| index).expect("a heading");
    let key = lmdv_render::heading_key(target);

    runtime.update(|state, tx| state.jump_to_heading(&key, tx));
    // The glide: the line stays away for as long as it runs.
    for _ in 0..12 {
        runtime.frame(1.0 / 60.0);
        assert!(!has_key(&runtime.elements(), SECTION_LINE_KEY), "the line is away during the glide");
    }
    // A long glide: the open-ended hold still runs well past the old 250 ms.
    settle(&mut runtime, 20);
    assert!(runtime.state.section_line.holding(), "the hold follows the glide, not a fixed 250 ms");
    // It lands, the heading lights up, and the line waits for its own settle.
    settle(&mut runtime, 70);
    let elements = runtime.elements();
    let band = elements.iter().find(|element| element.role == "mark" && element.fill == ink);
    assert!(band.is_some(), "the landed heading is painted with the accent band (O-10)");
    assert!(!has_key(&elements, SECTION_LINE_KEY), "the jumped-to heading owns the top band");

    // The band is a moment, not a state: it clears 1.2 s after it appears.
    settle(&mut runtime, 120);
    assert!(
        !runtime.elements().iter().any(|element| element.role == "mark" && element.fill == ink),
        "the landing band expired"
    );
}

#[test]
fn the_line_yields_to_the_keyboard_focus_ring_under_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 40);
    assert!(has_key(&runtime.elements(), SECTION_LINE_KEY), "the line is on screen first");
    let index = runtime.state.reading.reader_block.get().expect("the block at the viewport top");
    let block = runtime.state.active_reader().and_then(|doc| doc.block_key(index).map(str::to_owned));
    let block = block.expect("a block key");
    // The keyboard is the modality (focus-visible), then the block takes the ring.
    runtime.key(Key::Tab.into());
    runtime.frame(1.0 / 60.0);
    runtime.update({
        let block = block.clone();
        move |_, tx| tx.focus(block, true)
    });
    settle(&mut runtime, 20);
    assert!(
        !runtime.elements().iter().any(|element| element.key.as_deref() == Some(SECTION_LINE_KEY) && !element.exiting),
        "a focus ring under the line hides it (OI-141)"
    );
}

#[test]
fn the_tab_shows_the_full_time_alone_and_the_short_form_among_tabs() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Reader", READER_MARKDOWN);
    let mut runtime = open(state);
    settle(&mut runtime, 3);
    let stats = runtime.state.active_tab().and_then(|tab| tab.view.reading).expect("a reading time");
    assert!(stats.minutes >= 1 && stats.words > 100);
    assert!(tab_text(&runtime.elements(), &stats.full()), "one tab says the full form");

    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_tabs(&mut state);
    // Three tabs with the panes closed leave the strip room for the short form.
    state.left.set_open(false);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    let mut runtime = open(state);
    settle(&mut runtime, 3);
    let stats = runtime.state.active_tab().and_then(|tab| tab.view.reading).expect("a reading time");
    let elements = runtime.elements();
    assert!(tab_text(&elements, &stats.short()), "several tabs say the short form");
    assert!(!tab_text(&elements, &format!("{}  {}", "Reading notes", stats.full())), "not the long one");
    let id = runtime.state.active_tab().expect("a tab").id;
    let tab = element(&elements, &format!("t-{}", crate::window::tab_key(id))).expect("the active tab");
    assert_eq!(tab.help.as_deref(), Some(stats.description().as_str()), "the card keeps the full time");
}

#[test]
fn editing_hides_the_time_but_the_card_keeps_it_and_a_full_strip_drops_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let id = state.open_synthetic("Reader", READER_MARKDOWN);
    // The poll that follows a load would do this; the editor path only hides the
    // inline time, the card keeps what the reading view counted.
    assert!(state.refresh_reading_stats());
    let stats = state.active_tab().and_then(|tab| tab.view.reading).expect("a reading time");
    let description = stats.description();
    let mut tx = rune::Tx::default();
    assert!(state.enter_editor(None, &mut tx), "the synthetic document can be edited");
    let mut runtime = open(state);
    settle(&mut runtime, 2);
    let elements = runtime.elements();
    assert!(!tab_text(&elements, "min"), "no time while editing");
    let tab = element(&elements, &format!("t-{}", crate::window::tab_key(id))).expect("the active tab");
    assert_eq!(tab.help.as_deref(), Some(description.as_str()), "the card still has it");

    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    for index in 0..12 {
        state.open_synthetic(&format!("A rather long document name number {index}"), "# Head\n\nText.\n");
    }
    let mut runtime = open(state);
    settle(&mut runtime, 4);
    let id = runtime.state.active_tab().map(|tab| tab.id).expect("a tab");
    let stats = runtime.state.active_tab().and_then(|tab| tab.view.reading).expect("a reading time");
    let elements = runtime.elements();
    assert!(!tab_text(&elements, "min"), "a full strip drops the inline time (T-35)");
    let tab = element(&elements, &format!("t-{}", crate::window::tab_key(id))).expect("the active tab");
    assert_eq!(tab.help.as_deref(), Some(stats.description().as_str()), "the card keeps it");
}

#[test]
fn a_restore_into_the_middle_names_the_section_once_it_settles() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    // The scene is a raw remembered position, not a jump: no hold, only settle.
    let mut runtime = open(state);
    settle(&mut runtime, 2);
    assert!(!has_key(&runtime.elements(), SECTION_LINE_KEY), "the first frames are still settling");
    settle(&mut runtime, 30);
    assert!(has_key(&runtime.elements(), SECTION_LINE_KEY));
}

#[test]
fn a_tab_switch_starts_the_line_again_from_nothing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    let first = state.active_tab().map(|tab| tab.id).expect("a tab");
    state.open_synthetic("Other", "# Other\n\nWords.\n");
    let mut runtime = open(state);
    settle(&mut runtime, 30);
    let elements = runtime.elements();
    assert!(!has_key(&elements, SECTION_LINE_KEY), "the new tab starts at the top, with no old path");
    assert!(runtime.state.tab(first).is_some(), "the scrolled document is still open behind it");
}

#[test]
fn a_dirty_tab_card_says_unsaved_instead_of_the_time() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let id = state.open_synthetic("Reader", READER_MARKDOWN);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.dirty = true;
    }
    let stats = {
        state.refresh_reading_stats();
        state.active_tab().and_then(|tab| tab.view.reading).expect("a reading time")
    };
    let mut runtime = open(state);
    settle(&mut runtime, 3);
    let elements = runtime.elements();
    let tab = element(&elements, &format!("t-{}", crate::window::tab_key(id))).expect("the active tab");
    assert_eq!(tab.help.as_deref(), Some(lmdv_i18n::t!("tab.unsaved")), "T-16's unsaved card");
    assert_ne!(tab.help.as_deref(), Some(stats.description().as_str()));
}

#[test]
fn the_section_scene_anchors_on_a_real_heading() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    let pending = state.reading.pending.clone().expect("a pending restore");
    assert!(!pending.id.is_empty(), "the scene names a heading, not a raw offset");
    let doc = state.active_reader().expect("the parsed model");
    assert!(doc.resolve_heading(&pending.id).is_some(), "the id resolves: {}", pending.id);
    assert!(pending.offset > 0.0);
    let raw = ScrollAnchor { id: String::new(), offset: 900.0 };
    assert!(raw.offset > 24.0, "the progress scene has somewhere to go");
}

#[test]
fn edit_from_here_uses_the_block_at_the_viewport_top() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_section(&mut state);
    let mut runtime = open(state);
    settle(&mut runtime, 40);
    let block = runtime.state.reading.reader_block.get().expect("the reading block");
    let line = runtime
        .state
        .active_reader()
        .and_then(|doc| doc.block_spans.get(block).map(|span| span.line))
        .expect("a block span");
    runtime.update(|state, tx| {
        assert!(state.edit_from_reading(tx));
    });
    settle(&mut runtime, 3);
    assert_eq!(runtime.state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));
    let editor = runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("the editor");
    let expected = crate::preview_nav::line_start(editor.text(), line);
    assert_eq!(editor.selection(), expected..expected, "the caret is at the reading anchor's block");
}
