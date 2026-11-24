//! P3-04c1: preview → source navigation (E-35 double-click, E-04 “Edit from
//! here”).
//!
//! The tests drive the real renderer and Rune's real pointer selection: a
//! double-click selects the word natively, the block wire hands over the block
//! and its drawn text, and the app places the source caret. Clicks land on the
//! first word of a drawn text element, whose rect starts where the text starts.

use lmdv_core::LargeFileMode;
use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime, Tx};
use tempfile::TempDir;

use crate::test_support;
use crate::view::view;
use crate::window::{Mode, WindowState};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn enter_editor(state: &mut WindowState) {
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx), "edit mode");
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));
}

fn set_large_file_mode(state: &WindowState, mode: LargeFileMode) {
    let mut prefs = state.shared().prefs().clone();
    prefs.editor.large_file_mode = mode;
    state.shared().set_prefs(prefs);
}

fn editor_text(state: &WindowState) -> String {
    state
        .active_tab()
        .and_then(|tab| tab.view.editor.as_ref())
        .map(|editor| editor.text().to_owned())
        .unwrap_or_default()
}

fn single_selection(state: &WindowState) -> (usize, usize) {
    state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|editor| editor.edit.range()).unwrap_or((0, 0))
}

fn block_selection(state: &WindowState, index: usize) -> (usize, usize) {
    state
        .active_tab()
        .and_then(|tab| tab.view.editor.as_ref())
        .and_then(|editor| editor.block_editor())
        .and_then(|blocks| blocks.block(index))
        .map(|block| block.edit.range())
        .unwrap_or((0, 0))
}

fn active_block(state: &WindowState) -> Option<usize> {
    state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|editor| editor.active_block())
}

fn element_with_text(runtime: &Runtime<WindowState>, needle: &str) -> ElementInfo {
    runtime
        .elements()
        .into_iter()
        .find(|element| element.role == "text" && element.text.as_deref().is_some_and(|text| text.contains(needle)))
        .unwrap_or_else(|| panic!("no text element containing {needle:?}"))
}

fn element_by_role(runtime: &Runtime<WindowState>, role: &str) -> ElementInfo {
    runtime.elements().into_iter().find(|element| element.role == role).unwrap_or_else(|| panic!("no {role} element"))
}

/// The left edge of a text element, a few points into its first word.
fn first_word_point(element: &ElementInfo) -> (f32, f32) {
    (element.rect[0] + 6.0, element.rect[1] + element.rect[3] * 0.5)
}

fn double_click(runtime: &mut Runtime<WindowState>, x: f32, y: f32) {
    for _ in 0..2 {
        runtime.pointer_move(x, y);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
    }
    runtime.frame(1.0 / 60.0);
}

fn secondary_click(runtime: &mut Runtime<WindowState>, x: f32, y: f32) {
    runtime.secondary_down(x, y);
    runtime.secondary_up(x, y);
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
}

fn click(runtime: &mut Runtime<WindowState>, x: f32, y: f32) {
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(1.0 / 60.0);
}

fn focused_key(runtime: &Runtime<WindowState>) -> Option<String> {
    runtime.elements().into_iter().find(|element| element.focused).and_then(|element| element.key)
}

#[test]
fn a_press_on_a_blocks_padding_lands_on_its_preview_line() {
    use crate::editor::scroll_sync::{PreviewBlock, PreviewGeometry, line_from_preview_y};
    use lmdv_render::BlockRef;

    assert_eq!(line_from_preview_y(100.0, 100.0, 0, 4, 150.0), 2, "the middle of four lines");
    assert_eq!(line_from_preview_y(100.0, 100.0, 0, 4, 90.0), 0, "above the block clamps");
    assert_eq!(line_from_preview_y(100.0, 0.0, 0, 4, 150.0), 0, "no measurement: the first line");

    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Padding", "one\ntwo\nthree\nfour\n");
    enter_editor(&mut state);
    let block = BlockRef {
        key: "b0".to_owned(),
        line: 0,
        end_line: 4,
        code: false,
        text: "one\ntwo\nthree\nfour\n".to_owned(),
        texts: Vec::new(),
    };
    *state.editor.preview.borrow_mut() = PreviewGeometry {
        blocks: vec![PreviewBlock {
            index: 0,
            key: "b0".to_owned(),
            top: 100.0,
            height: 100.0,
            next_top: 200.0,
            heading: None,
            text_boxes: vec![[0.0, 100.0, 60.0, 20.0]],
        }],
        viewport_top: 100.0,
        viewport: 400.0,
        ..PreviewGeometry::default()
    };
    state.editor.preview_point.set(Some([30.0, 150.0]));
    state.editor.preview_block.set(Some(0));
    let mut tx = Tx::default();
    assert!(state.place_caret_on_preview_padding(&block, &mut tx));
    assert_eq!(single_selection(&state), (8, 8), "the middle source line, by the block's height");

    // A press on a link or picture is inside a text box and keeps its click.
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_selection(0..0);
    state.editor.preview_point.set(Some([30.0, 110.0]));
    assert!(!state.place_caret_on_preview_padding(&block, &mut tx), "text with no selection keeps its click");
    assert_eq!(single_selection(&state), (0, 0));
}

#[test]
fn a_double_click_maps_a_paragraph_word_to_its_own_block_line() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Two paragraphs", "budget one\n\nbudget two\n");
    enter_editor(&mut state);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let paragraph = element_with_text(&runtime, "budget two");
    let (x, y) = first_word_point(&paragraph);
    double_click(&mut runtime, x, y);

    let text = editor_text(&runtime.state);
    let (start, end) = single_selection(&runtime.state);
    assert_eq!(&text[start..end], "budget", "the clicked word is selected");
    assert_eq!(start, 12, "the second paragraph's word, not the first one's");
    assert_eq!(focused_key(&runtime), Some("editor-source".to_owned()));
}

#[test]
fn a_repeated_word_picks_the_same_occurrence_of_its_own_text_element() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Repeated", "- budget alpha\n- budget beta\n");
    enter_editor(&mut state);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let item = element_with_text(&runtime, "budget beta");
    let (x, y) = first_word_point(&item);
    double_click(&mut runtime, x, y);

    let text = editor_text(&runtime.state);
    let (start, end) = single_selection(&runtime.state);
    assert_eq!(&text[start..end], "budget");
    assert_eq!(start, 17, "the second item's occurrence, not the first: {text:?}");
    assert_eq!(text.match_indices("budget").map(|(at, _)| at).collect::<Vec<_>>(), [2, 17]);
}

#[test]
fn block_mode_places_the_local_caret_of_the_blocks_field() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Blocks", "one budget alpha\n\nbudget two\n");
    enter_editor(&mut state);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let second = element_with_text(&runtime, "budget two");
    let (x, y) = first_word_point(&second);
    double_click(&mut runtime, x, y);

    assert_eq!(active_block(&runtime.state), Some(1), "the second blank-line block");
    assert_eq!(block_selection(&runtime.state, 1), (0, 6), "a local selection in that field");
    assert_eq!(focused_key(&runtime), Some("editor-block-1".to_owned()));
}

#[test]
fn a_link_keeps_its_own_double_click() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Link", "[budget](https://example.invalid/page)\n");
    enter_editor(&mut state);
    state.active_editor_edit().set_caret(0);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let link = element_with_text(&runtime, "budget");
    let (x, y) = first_word_point(&link);
    double_click(&mut runtime, x, y);

    assert_eq!(single_selection(&runtime.state), (0, 0), "the editor caret did not move");
}

#[test]
fn a_task_box_keeps_its_own_double_click() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    // The task block starts on line 2: a caret that moved would be visible.
    state.open_synthetic("Tasks", "first paragraph\n\n- [ ] budget\n");
    enter_editor(&mut state);
    state.active_editor_edit().set_caret(0);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let checkbox = element_by_role(&runtime, "checkbox");
    let (x, y) = (checkbox.rect[0] + checkbox.rect[2] * 0.5, checkbox.rect[1] + checkbox.rect[3] * 0.5);
    double_click(&mut runtime, x, y);
    assert_eq!(single_selection(&runtime.state), (0, 0), "the checkbox is not edit-from-here");
}

#[test]
fn a_picture_keeps_its_own_double_click() {
    use lmdv_core::DocStatus;

    let temp = TempDir::new().unwrap();
    std::fs::write(temp.path().join("pic.png"), include_bytes!("../../../tests/fixtures/img/test.png")).unwrap();
    let path = temp.path().join("picture.md");
    std::fs::write(&path, "budget one\n\n![budget](pic.png)\n").unwrap();

    let mut state = window(&temp);
    state.open_path(path);
    let mut tx = Tx::default();
    while state.active_tab().and_then(|tab| tab.view.reader.as_ref()).is_none() {
        state.poll(0.0, &mut tx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        state.active_tab().and_then(|tab| tab.document()).is_some_and(|doc| *doc.status.borrow() == DocStatus::Ready)
    );
    enter_editor(&mut state);
    state.active_editor_edit().set_caret(0);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);

    assert_eq!(single_selection(&runtime.state), (0, 0), "the caret starts at zero");
    let picture = element_by_role(&runtime, "image");
    let (x, y) = (picture.rect[0] + picture.rect[2] * 0.5, picture.rect[1] + picture.rect[3] * 0.5);
    assert!(runtime.element_at(x, y).is_some(), "the picture is under the point");
    double_click(&mut runtime, x, y);
    assert_eq!(single_selection(&runtime.state), (0, 0), "the picture keeps its own click");
}

#[test]
fn a_viewer_double_click_only_selects_and_never_edits() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Reading", "budget one budget two\n");
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let paragraph = element_with_text(&runtime, "budget one budget two");
    let (x, y) = first_word_point(&paragraph);
    double_click(&mut runtime, x, y);

    assert_eq!(runtime.state.active_tab().map(|tab| tab.view.mode), Some(Mode::Viewer), "R-27: no mode change");
    assert!(runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_none());
}

#[test]
fn the_block_menu_enters_edit_at_the_clicked_line() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Menu", "budget one\n\nbudget two\n");
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let paragraph = element_with_text(&runtime, "budget two");
    let (x, y) = first_word_point(&paragraph);
    secondary_click(&mut runtime, x, y);
    assert!(runtime.state.block_menu().is_some(), "the context menu opened on the block");

    let (x, y) = runtime.find("block-menu-item0").expect("the Edit from here item");
    click(&mut runtime, x, y);

    assert!(runtime.state.block_menu().is_none(), "choosing closes the menu");
    assert_eq!(runtime.state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));
    assert_eq!(single_selection(&runtime.state), (12, 12), "the block's 0-based line start");
    assert_eq!(focused_key(&runtime), Some("editor-source".to_owned()));
}

#[test]
fn the_viewer_block_menu_finds_in_document_and_the_editor_menu_does_not() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Find", "The quiet page\n\nSecond paragraph\n");
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let paragraph = element_with_text(&runtime, "The quiet page");
    let (x, y) = first_word_point(&paragraph);
    secondary_click(&mut runtime, x, y);
    let elements = runtime.elements();
    assert!(
        elements.iter().any(|element| element.text.as_deref() == Some(lmdv_i18n::t!("ctx.findInDoc"))),
        "R-17: the reading menu offers Find in document"
    );
    let (x, y) = runtime.find("block-menu-item1").expect("the Find in document row");
    click(&mut runtime, x, y);
    let tab = runtime.state.active_tab().expect("the tab");
    assert!(tab.view.reader_find.open, "the reading find bar opened");
    assert!(runtime.state.block_menu().is_none(), "the menu closed behind the bar");

    // The editor's preview keeps its menu to source actions.
    enter_editor(&mut runtime.state);
    runtime.frame(0.0);
    runtime.frame(0.0);
    let paragraph = element_with_text(&runtime, "The quiet page");
    let (x, y) = first_word_point(&paragraph);
    secondary_click(&mut runtime, x, y);
    assert!(runtime.state.block_menu().is_some());
    let texts: Vec<String> = runtime.elements().iter().filter_map(|element| element.text.clone()).collect();
    assert!(
        !texts.iter().any(|text| text == lmdv_i18n::t!("ctx.findInDoc")),
        "no reading-find row while editing: {texts:?}"
    );
}

#[test]
fn the_block_menu_closes_on_escape_and_cancel() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Dismiss", "a paragraph\n");
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let paragraph = element_with_text(&runtime, "a paragraph");
    let (x, y) = first_word_point(&paragraph);
    secondary_click(&mut runtime, x, y);
    assert!(runtime.state.block_menu().is_some());
    runtime.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.block_menu().is_none(), "Escape closes it");

    secondary_click(&mut runtime, x, y);
    assert!(runtime.state.block_menu().is_some());
    let (x, y) = runtime.find("block-menu-item1").expect("the Cancel item");
    click(&mut runtime, x, y);
    assert!(runtime.state.block_menu().is_none());
}

#[test]
fn edit_from_here_moves_the_caret_when_already_editing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Move", "budget one\n\nbudget two\n");
    enter_editor(&mut state);
    state.active_editor_edit().set_caret(3);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let paragraph = element_with_text(&runtime, "budget two");
    let (x, y) = first_word_point(&paragraph);
    secondary_click(&mut runtime, x, y);
    let (x, y) = runtime.find("block-menu-item0").expect("the Edit from here item");
    click(&mut runtime, x, y);

    assert_eq!(single_selection(&runtime.state), (12, 12), "the caret moved to the block's line");
}

#[test]
fn shift_f10_opens_the_focused_blocks_menu() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Keyboard", "budget one\n\nbudget two\n");
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    // A click focuses the block (it is focusable but out of the Tab order).
    let paragraph = element_with_text(&runtime, "budget two");
    let (x, y) = first_word_point(&paragraph);
    click(&mut runtime, x, y);
    assert_eq!(focused_key(&runtime), Some("b1".to_owned()), "the block took the keyboard");

    runtime.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.block_menu().is_some(), "Shift+F10 opens the block's menu");

    runtime.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.block_menu().is_none(), "Escape closes it");
    assert_eq!(focused_key(&runtime), Some("b1".to_owned()), "focus returns to the block");
}
