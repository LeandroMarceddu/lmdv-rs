//! P3-04a: block-mode editing, layout, undo across blocks and split/merge.
//!
//! These tests drive `WindowState` directly and through a headless Rune
//! runtime, with real files in a temporary directory where saving matters.

use std::time::{Duration, Instant};

use lmdv_core::{DocStatus, LargeFileMode};
use rune::{Env, Key, KeyEvent, Mods, Runtime, Tx};
use tempfile::TempDir;

use crate::editor::ESCAPE_TAB_WINDOW;
use crate::test_support;
use crate::view::{block_scene_markdown, view};
use crate::window::{Mode, Tab, WindowState};

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

fn open_loaded(state: &mut WindowState, path: &std::path::Path) {
    state.open_path(path.to_path_buf());
    poll_until(state, "the file load", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| matches!(*doc.status.borrow(), DocStatus::Ready))
    });
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

fn block_text(state: &WindowState, index: usize) -> String {
    state
        .active_tab()
        .and_then(|tab| tab.view.editor.as_ref())
        .and_then(|editor| editor.block_editor())
        .and_then(|blocks| blocks.block(index))
        .map(|block| block.edit.text.clone())
        .unwrap_or_default()
}

fn large_markdown() -> String {
    block_scene_markdown()
}

fn plain_key(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn shift_tab_key() -> KeyEvent {
    KeyEvent { key: Key::Tab, mods: Mods { shift: true, ..Mods::default() } }
}

fn single_range(state: &WindowState) -> (usize, usize) {
    state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|editor| editor.edit.range()).unwrap_or((0, 0))
}

fn command(ch: char, shift: bool) -> KeyEvent {
    KeyEvent { key: Key::Command(ch), mods: Mods { cmd: true, ctrl: true, shift, ..Mods::default() } }
}

#[test]
fn auto_selects_block_mode_only_above_the_threshold() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);

    state.open_synthetic("Small", "# Small\n\nA short note.\n");
    enter_editor(&mut state);
    assert!(
        !state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.block_mode_active())
    );

    let mut tx = Tx::default();
    state.leave_editor(&mut tx);
    state.open_synthetic("Large", &large_markdown());
    enter_editor(&mut state);
    let editor = state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("editor");
    assert!(editor.block_mode_active(), "Auto crosses 64 KiB");
    assert!(editor.block_editor().is_some_and(|blocks| blocks.blocks().len() > 100), "many block fields");
}

#[test]
fn the_block_preference_forces_block_mode_for_a_small_document() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Small", "# Small\n\nA short note.\n");
    enter_editor(&mut state);
    assert!(
        state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.block_mode_active())
    );
    assert_eq!(state.block_count(), 2, "heading and paragraph are two blank-line blocks");

    let mut tx = Tx::default();
    state.leave_editor(&mut tx);
    set_large_file_mode(&state, LargeFileMode::Auto);
    enter_editor(&mut state);
    assert!(
        !state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.block_mode_active())
    );
}

#[test]
fn block_mode_survives_enter_and_leave_and_keeps_the_text() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("large.md");
    std::fs::write(&path, large_markdown()).unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    let before = editor_text(&state);
    assert!(!before.is_empty());

    let mut tx = Tx::default();
    state.leave_editor(&mut tx);
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Viewer));
    assert_eq!(
        state.active_tab().and_then(Tab::document).map(|doc| doc.text().text().to_owned()),
        Some(before.clone())
    );

    enter_editor(&mut state);
    assert!(
        state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.block_mode_active())
    );
    assert_eq!(editor_text(&state), before);
}

#[test]
fn typing_commits_the_block_and_saves_the_round_trip() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("large.md");
    let original = large_markdown();
    std::fs::write(&path, &original).unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(1));
    state.active_editor_edit().set_caret(0);
    state.active_editor_edit().insert("Edited ");
    state.editor_changed();

    assert!(state.active_tab().is_some_and(|tab| tab.view.dirty), "the block edit marks the tab dirty");
    assert!(
        editor_text(&state).starts_with("# A long document\n\nEdited A generated document"),
        "{:?}",
        &editor_text(&state)[..80]
    );
    let expected = editor_text(&state);

    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    poll_until(&mut state, "the save", |state| {
        state.worker().is_empty()
            && state.active_document_id().is_some_and(|doc| !state.editor.save_texts.contains_key(&doc))
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected, "the saved file carries the block edit");
    assert!(!state.active_tab().is_some_and(|tab| tab.view.dirty), "saving cleans the buffer");
}

#[test]
fn drafts_and_the_preview_see_committed_block_text() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("large.md");
    std::fs::write(&path, large_markdown()).unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(0));
    state.active_editor_edit().set_caret(0);
    state.active_editor_edit().insert("Draft ");
    state.editor_changed();

    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert!(
        doc.text().text().starts_with("Draft # A long document"),
        "the shared document is current before any flush"
    );

    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    state.flush_active_draft();
    assert!(state.shared().flush_store(Duration::from_secs(2)));
    let drafts = lmdv_io::store::load_drafts(state.shared().dirs());
    assert!(
        drafts.iter().any(|(_, draft)| draft.text.starts_with("Draft # A long document")),
        "the draft carries the committed block text: {:?}",
        drafts.iter().map(|(_, draft)| &draft.text[..draft.text.len().min(30)]).collect::<Vec<_>>()
    );
}

#[test]
fn enter_twice_splits_a_block_and_merge_joins_it_back() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Split", "onetwo\n\nthree\n");
    enter_editor(&mut state);
    assert_eq!(state.block_count(), 2);
    assert_eq!(block_text(&state, 0), "onetwo\n\n");

    // Caret after "one": Enter twice makes a paragraph break and a new block.
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(0));
    state.block_editor_mut_for_test(0).set_caret(3);
    let mut tx = Tx::default();
    assert!(state.block_key(0, &plain_key(Key::Enter), &mut tx));
    assert!(state.block_key(0, &plain_key(Key::Enter), &mut tx));
    assert_eq!(state.block_count(), 3, "Enter twice split the block");
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()), Some(1));
    assert_eq!(editor_text(&state), "one\n\ntwo\n\nthree\n", "no text is lost");

    // Backspace at the start of the new block merges it back.
    assert!(state.block_key(1, &plain_key(Key::Backspace), &mut tx));
    assert_eq!(state.block_count(), 2);
    assert_eq!(editor_text(&state), "one\n\ntwo\n\nthree\n");
    assert_eq!(block_text(&state, 0), "one\n\ntwo\n\n");

    // Undo walks back through the merge, the split and the newline.
    assert!(state.block_key(0, &command('z', false), &mut tx));
    assert_eq!(state.block_count(), 3, "the merge undone");
    assert!(state.block_key(1, &command('z', false), &mut tx));
    assert_eq!(state.block_count(), 2, "the split undone");
    assert!(state.block_key(0, &command('z', false), &mut tx));
    assert_eq!(editor_text(&state), "onetwo\n\nthree\n", "the paragraph break undone too");
}

#[test]
fn delete_at_the_end_of_a_block_merges_the_next_one() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Merge", "one\n\ntwo\n");
    enter_editor(&mut state);
    assert_eq!(state.block_count(), 2);
    let end = block_text(&state, 0).len();
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(0));
    state.block_editor_mut_for_test(0).set_caret(end);

    let mut tx = Tx::default();
    assert!(state.block_key(0, &plain_key(Key::Delete), &mut tx));
    assert_eq!(state.block_count(), 1);
    assert_eq!(editor_text(&state), "one\n\ntwo\n");
    assert_eq!(block_text(&state, 0), "one\n\ntwo\n");
}

#[test]
fn undo_walks_across_blocks_in_commit_order() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Undo", "one\n\ntwo\n");
    enter_editor(&mut state);
    let original = editor_text(&state);

    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(0));
    state.active_editor_edit().set_caret(3);
    state.active_editor_edit().insert("!");
    state.editor_changed();
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(1));
    state.active_editor_edit().set_caret(3);
    state.active_editor_edit().insert("?");
    state.editor_changed();
    assert_eq!(editor_text(&state), "one!\n\ntwo?\n");

    let mut tx = Tx::default();
    assert!(state.block_key(1, &command('z', false), &mut tx));
    assert_eq!(editor_text(&state), "one!\n\ntwo\n", "the last edit, in block B, undoes first");
    assert!(state.block_key(1, &command('z', false), &mut tx));
    assert_eq!(editor_text(&state), original, "then block A's");
    assert!(state.block_key(1, &command('z', true), &mut tx));
    assert_eq!(editor_text(&state), "one!\n\ntwo\n", "Ctrl+Shift+Z redoes");
    assert_eq!(
        state.active_tab().and_then(Tab::document).map(|doc| doc.text().text().to_owned()),
        Some("one!\n\ntwo\n".to_owned()),
        "undo/redo reaches the shared document"
    );
}

#[test]
fn ctrl_z_through_the_runtime_undoes_typing_in_two_blocks() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Keys", "alpha\n\nbeta\n");
    enter_editor(&mut state);
    let original = editor_text(&state);

    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|e| e.block_mode_active()));
    assert_eq!(
        runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()),
        Some(0),
        "entry focused the caret's block"
    );

    runtime.key(plain_key(Key::Text("X".to_owned())));
    runtime.frame(1.0 / 60.0);
    assert!(editor_text(&runtime.state).starts_with("Xalpha"));

    // Move to the second block the way a click does: focus its field.
    runtime.update(|state, tx| {
        state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(1));
        let field = state.block_editor_mut_for_test(1);
        field.set_caret(0);
        field.focused = true;
        state.focus_block(1, tx);
    });
    runtime.frame(1.0 / 60.0);
    runtime.key(plain_key(Key::Text("Y".to_owned())));
    runtime.frame(1.0 / 60.0);
    assert!(editor_text(&runtime.state).contains("\nYbeta"), "second block edited: {:?}", editor_text(&runtime.state));

    runtime.key(command('z', false));
    runtime.frame(1.0 / 60.0);
    assert!(editor_text(&runtime.state).contains("\nbeta"), "{:?}", editor_text(&runtime.state));
    runtime.key(command('z', false));
    runtime.frame(1.0 / 60.0);
    assert_eq!(editor_text(&runtime.state), original, "both block edits undone in order");
}

#[test]
fn focusing_a_deep_block_reveals_and_builds_its_row() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_editor_block(&mut state);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.update(|state, tx| state.focus_block(500, tx));
    for _ in 0..6 {
        runtime.frame(1.0 / 60.0);
    }
    let row = runtime.elements().into_iter().find(|element| element.key.as_deref() == Some("editor-block-500"));
    let row = row.expect("the focused deep row is built");
    assert!(row.focused, "the deep field took the focus");
    assert_eq!(
        runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()),
        Some(500)
    );
}

#[test]
fn the_block_scene_builds_several_rows_and_the_mode_label() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_editor_block(&mut state);
    assert!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|e| e.block_mode_active()));

    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    let elements = runtime.elements();
    let label = elements.iter().find(|element| element.key.as_deref() == Some("editor-block-mode"));
    assert!(label.is_some_and(|element| element.text.as_deref() == Some("Block mode")), "the header names block mode");
    let fields = elements
        .iter()
        .filter(|element| {
            element.key.as_deref().is_some_and(|found| {
                found.starts_with("editor-block-") && found != "editor-block-mode" && found != "editor-block-count"
            })
        })
        .count();
    assert!(fields >= 2, "at least two block fields drawn, got {fields}");
    assert!(
        !elements.iter().any(|element| element.key.as_deref() == Some("editor-source")),
        "the single field is not drawn in block mode"
    );
}

#[test]
fn the_find_bar_focuses_the_block_it_selected() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Find", "one\n\ntwo\n\nthree\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.open_find(false, &mut tx);
    state.set_find_query("three");
    state.find_next_cmd(&mut tx);
    let editor = state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("editor");
    assert_eq!(editor.active_block(), Some(2), "find selected the third block");
    assert_eq!(editor.selection(), 10..15);
}

#[test]
fn applying_a_format_action_never_touches_block_zero_from_elsewhere() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Format", "one\n\ntwo\n");
    enter_editor(&mut state);
    {
        let editor = state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor");
        editor.set_active_block(Some(1));
        if let Some(block) = editor.block_editor_mut().and_then(|blocks| blocks.block_mut(1)) {
            block.set_selection(0, 3);
        }
    }
    assert!(state.apply_format(crate::edit_ui::FormatAction::Wrap("**")));
    assert_eq!(block_text(&state, 0), "one\n\n", "block 0 is untouched");
    assert_eq!(block_text(&state, 1), "**two**\n");
    assert_eq!(editor_text(&state), "one\n\n**two**\n");

    let editor = state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor");
    editor.set_active_block(None);
    assert!(!state.apply_format(crate::edit_ui::FormatAction::Wrap("*")), "no active block, no silent block 0 edit");
}

#[test]
fn a_fenced_block_with_blank_lines_belongs_to_one_field() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Fence", "before\n\n```\n\ncode\n\n```\n\nafter\n");
    enter_editor(&mut state);
    assert_eq!(state.block_count(), 3, "blank lines inside the fence do not split");
    assert_eq!(block_text(&state, 1), "```\n\ncode\n\n```\n\n");

    // Enter twice inside the fence must not split it apart.
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(1));
    state.block_editor_mut_for_test(1).set_caret(9);
    let mut tx = Tx::default();
    assert!(state.block_key(1, &plain_key(Key::Enter), &mut tx));
    assert!(state.block_key(1, &plain_key(Key::Enter), &mut tx));
    assert_eq!(state.block_count(), 3, "still one fenced block");
    assert!(block_text(&state, 1).contains("code"));
}

#[test]
fn a_blank_only_block_can_be_typed_into_and_merged_away() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Blank", "a\n\n\n");
    enter_editor(&mut state);
    assert_eq!(state.block_count(), 1, "all trailing blank lines belong to the content block");

    // Enter on a line that already follows a blank run splits off a
    // blank-only block at the caret.
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(0));
    state.block_editor_mut_for_test(0).set_caret(3);
    let mut tx = Tx::default();
    assert!(state.block_key(0, &plain_key(Key::Enter), &mut tx));
    assert_eq!(state.block_count(), 2);
    assert_eq!(block_text(&state, 1), "\n");

    // Typing at its start makes it a normal block; the separator stays.
    state.active_editor_edit().set_caret(0);
    state.active_editor_edit().insert("z");
    state.editor_changed();
    assert_eq!(block_text(&state, 1), "z\n");
    assert_eq!(editor_text(&state), "a\n\n\nz\n");
    assert_eq!(
        state.active_tab().and_then(Tab::document).map(|doc| doc.text().text().to_owned()),
        Some("a\n\n\nz\n".to_owned())
    );

    state.block_editor_mut_for_test(1).set_caret(0);
    assert!(state.block_key(1, &plain_key(Key::Backspace), &mut tx));
    assert_eq!(state.block_count(), 1);
    assert_eq!(editor_text(&state), "a\n\n\nz\n", "merging abandons no text");
}

#[test]
fn reload_keeps_block_mode_and_restores_the_block_caret() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("reload-block.md");
    std::fs::write(&path, "one\n\ntwo\n").unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    assert!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|e| e.block_mode_active()));

    // Caret one byte into the second block.
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(1));
    state.block_editor_mut_for_test(1).set_selection(1, 1);

    std::fs::write(&path, "two\n\nfour\n").unwrap();
    let mut tx = Tx::default();
    state.request_reload(&mut tx);
    poll_until(&mut state, "the reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.text().text() == "two\n\nfour\n")
    });
    let editor = state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("editor");
    assert!(editor.block_mode_active(), "a reload never flips the layout mid-session");
    assert_eq!(editor.block_editor().map(|blocks| blocks.blocks().len()), Some(2));
    assert_eq!(editor.active_block(), Some(1), "the caret's block survives the reload");
    assert_eq!(editor.selection(), 6..6, "the local caret is restored after the new block start");
}

#[test]
fn save_flushes_a_direct_block_field_edit() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("flush.md");
    std::fs::write(&path, "one\n\ntwo\n").unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);

    // A field edit that never went through `on_input_change`: the save path
    // must still commit it before handing the text to the job.
    let end = state.block_editor_mut_for_test(1).text.trim_end_matches('\n').len();
    let field = state.block_editor_mut_for_test(1);
    field.set_caret(end);
    field.insert("!");
    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    poll_until(&mut state, "the save", |state| {
        state.worker().is_empty()
            && state.active_document_id().is_some_and(|doc| !state.editor.save_texts.contains_key(&doc))
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n\ntwo!\n");
}

#[test]
fn an_empty_document_has_one_typable_block() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Empty", "");
    enter_editor(&mut state);
    assert_eq!(state.block_count(), 1);
    assert_eq!(block_text(&state, 0), "");
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()), Some(0));

    state.active_editor_edit().set_caret(0);
    state.active_editor_edit().insert("first");
    state.editor_changed();
    assert_eq!(editor_text(&state), "first");
    assert_eq!(block_text(&state, 0), "first");

    let mut tx = Tx::default();
    assert!(state.block_key(0, &command('z', false), &mut tx));
    assert_eq!(editor_text(&state), "");
}

#[test]
fn focusing_a_block_field_makes_it_active_through_the_accessor() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Click", "one\n\ntwo\n\nthree\n");
    enter_editor(&mut state);

    // What a pointer press does: Rune focuses the block's field, then asks its
    // `on_edit` accessor for the model.
    let field = state.block_editor_mut_for_test(2);
    field.focused = true;
    field.set_selection(0, 5);
    let _ = state.active_block_edit(2);
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()), Some(2));

    assert!(state.apply_format(crate::edit_ui::FormatAction::Wrap("**")));
    assert_eq!(block_text(&state, 0), "one\n\n");
    assert_eq!(block_text(&state, 2), "**three**\n");
}

#[test]
fn tab_indents_and_shift_tab_outdents_in_the_single_field() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Tab", "one\ntwo\n");
    enter_editor(&mut state);
    assert!(!state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|e| e.block_mode_active()));

    state.active_editor_edit().set_caret(1);
    let mut tx = Tx::default();
    assert!(state.editor_key(&plain_key(Key::Tab), &mut tx), "Tab is consumed, never a focus change");
    assert_eq!(editor_text(&state), "  one\ntwo\n");
    assert_eq!(single_range(&state), (3, 3), "the caret rides its line");

    assert!(state.editor_key(&shift_tab_key(), &mut tx));
    assert_eq!(editor_text(&state), "one\ntwo\n");
    assert_eq!(single_range(&state), (1, 1));

    // A multi-line selection indents every line it touches, as one undo step.
    state.active_editor_edit().set_selection(0, 7);
    assert!(state.editor_key(&plain_key(Key::Tab), &mut tx));
    assert_eq!(editor_text(&state), "  one\n  two\n");
    assert_eq!(single_range(&state), (2, 11));
    let undone =
        state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).is_some_and(lmdv_edit::EditorSession::undo);
    assert!(undone, "one Tab is one history step");
    state.editor_changed();
    assert_eq!(editor_text(&state), "one\ntwo\n");
}

#[test]
fn tab_indents_only_the_active_block_and_is_one_journal_step() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Block tab", "one\n\ntwo\n");
    enter_editor(&mut state);
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(0));
    state.block_editor_mut_for_test(0).set_caret(3);
    let mut tx = Tx::default();

    assert!(state.block_key(0, &plain_key(Key::Tab), &mut tx), "Tab is consumed in block mode");
    assert_eq!(block_text(&state, 0), "  one\n\n", "only the active block is indented");
    assert_eq!(block_text(&state, 1), "two\n", "the neighbour is untouched");
    assert_eq!(
        state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()),
        Some(0),
        "Tab does not change the active block"
    );
    assert_eq!(
        state
            .active_tab()
            .and_then(|tab| tab.view.editor.as_ref())
            .map(|e| e.block_editor().and_then(|b| b.block(0)).map(|b| b.edit.range())),
        Some(Some((5, 5)))
    );

    assert!(state.block_key(0, &command('z', false), &mut tx), "the indent is undoable");
    assert_eq!(block_text(&state, 0), "one\n\n");

    assert!(state.block_key(0, &plain_key(Key::Tab), &mut tx));
    assert!(state.block_key(0, &shift_tab_key(), &mut tx));
    assert_eq!(block_text(&state, 0), "one\n\n", "Shift+Tab outdents the same block");
    assert_eq!(block_text(&state, 1), "two\n");
}

#[test]
fn tab_does_not_move_focus_between_block_fields() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Block focus", "one\n\ntwo\n");
    enter_editor(&mut state);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    assert_eq!(
        runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()),
        Some(0)
    );

    runtime.key(plain_key(Key::Tab));
    runtime.frame(1.0 / 60.0);
    let elements = runtime.elements();
    assert!(elements.iter().any(|element| element.key.as_deref() == Some("editor-block-0") && element.focused));
    assert!(!elements.iter().any(|element| element.key.as_deref() == Some("editor-block-1") && element.focused));
    assert_eq!(block_text(&runtime.state, 0), "  one\n\n");
    assert_eq!(
        runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()),
        Some(0),
        "focus stays in block 0"
    );

    runtime.key(shift_tab_key());
    runtime.frame(1.0 / 60.0);
    assert_eq!(block_text(&runtime.state, 0), "one\n\n");
    assert_eq!(
        runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()),
        Some(0)
    );
}

#[test]
fn escape_then_tab_leaves_the_source_without_indenting() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Leave", "one\n\ntwo\n");
    enter_editor(&mut state);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    assert!(
        runtime.elements().iter().any(|element| element.key.as_deref() == Some("editor-block-0") && element.focused)
    );

    runtime.key(plain_key(Key::Escape));
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.editor.last_escape_at.is_some(), "Escape armed the leave gesture");
    assert!(
        runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|e| {
            e.block_editor().and_then(|blocks| blocks.block(0)).is_some_and(|block| block.edit.focused)
        }),
        "Escape alone keeps the caret in the field until Tab"
    );

    runtime.key(plain_key(Key::Tab));
    for _ in 0..4 {
        runtime.frame(1.0 / 60.0);
    }
    let elements = runtime.elements();
    assert!(
        elements.iter().any(|element| element.key.as_deref() == Some("toggle-preview") && element.focused),
        "focus moved out of the editor, to the preview toggle"
    );
    assert!(
        !elements.iter().any(|element| {
            element.focused
                && element
                    .key
                    .as_deref()
                    .is_some_and(|key| key.starts_with("editor-block-") && key != "editor-block-mode")
        }),
        "no block field keeps the focus"
    );
    assert_eq!(block_text(&runtime.state, 0), "one\n\n", "Esc then Tab never indents");
    assert!(runtime.state.editor.last_escape_at.is_none(), "the gesture was consumed");
}

#[test]
fn typing_after_escape_cancels_the_leave_gesture() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Cancel", "one\n\ntwo\n");
    enter_editor(&mut state);
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);

    runtime.key(plain_key(Key::Escape));
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.editor.last_escape_at.is_some());

    // Typing is not part of the gesture: the next Tab indents again.
    runtime.key(plain_key(Key::Text("x".to_owned())));
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.editor.last_escape_at.is_none(), "a typed key cancels the armed leave");

    runtime.key(plain_key(Key::Tab));
    runtime.frame(1.0 / 60.0);
    assert_eq!(block_text(&runtime.state, 0), "  xone\n\n", "Tab indents after the cancelled gesture");
    let elements = runtime.elements();
    assert!(elements.iter().any(|element| element.key.as_deref() == Some("editor-block-0") && element.focused));
}

#[test]
fn escape_then_tab_in_the_single_field_does_not_indent() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Leave single", "one two\n");
    enter_editor(&mut state);
    state.active_editor_edit().set_caret(0);
    let mut tx = Tx::default();

    assert!(state.editor_key(&plain_key(Key::Escape), &mut tx));
    assert!(state.editor.last_escape_at.is_some());
    assert!(state.editor_key(&plain_key(Key::Tab), &mut tx), "Tab is consumed by the leave gesture");
    assert_eq!(editor_text(&state), "one two\n", "the source did not indent");
    assert!(state.editor.last_escape_at.is_none(), "the gesture was consumed");
}

#[test]
fn the_first_escape_clears_a_selection_and_does_not_arm_the_leave() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Escape", "one two\n");
    enter_editor(&mut state);
    state.active_editor_edit().set_selection(0, 3);
    let mut tx = Tx::default();

    assert!(state.editor_key(&plain_key(Key::Escape), &mut tx));
    assert_eq!(single_range(&state), (3, 3), "the first Escape collapses the selection");
    assert!(state.editor.last_escape_at.is_none(), "and does not arm the leave");
    assert!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.edit.focused));

    assert!(state.editor_key(&plain_key(Key::Escape), &mut tx));
    assert!(state.editor.last_escape_at.is_some(), "the second Escape arms it");
    assert!(
        state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.edit.focused),
        "the caret stays in the field until Tab"
    );

    // A stale timestamp lets Tab fall back to Rune's focus traversal.
    state.editor.last_escape_at = Some(Instant::now() - ESCAPE_TAB_WINDOW - Duration::from_millis(1));
    assert!(!state.escape_then_tab(&mut tx));
    assert!(state.editor.last_escape_at.is_none());
}

#[test]
fn switching_tabs_keeps_the_block_journal() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    let first = state.open_synthetic("First", "one\n\ntwo\n");
    enter_editor(&mut state);
    state.active_editor_edit().set_caret(3);
    state.active_editor_edit().insert("!");
    state.editor_changed();
    let second = state.open_synthetic("Second", "other\n");
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(second));

    assert!(state.activate_tab(first));
    let snapshot_text = editor_text(&state);
    assert_eq!(snapshot_text, "one!\n\ntwo\n");
    let mut tx = Tx::default();
    assert!(state.block_key(0, &command('z', false), &mut tx));
    assert_eq!(editor_text(&state), "one\n\ntwo\n", "the tab's block journal came back with it");
}

#[test]
fn typing_after_a_separator_moves_the_text_and_caret_to_the_next_field() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Boundary", "a\n\nb\n");
    enter_editor(&mut state);
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(0));
    state.block_editor_mut_for_test(0).set_caret(3);

    let mut tx = Tx::default();
    state.active_editor_edit().insert("x");
    state.block_changed(0, &mut tx);
    assert_eq!(block_text(&state, 0), "a\n\n");
    assert_eq!(block_text(&state, 1), "xb\n", "the typed text joined the canonical next block");
    assert_eq!(editor_text(&state), "a\n\nxb\n");
    assert_eq!(
        state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()),
        Some(1),
        "the active field followed the caret"
    );

    assert!(state.block_key(1, &command('z', false), &mut tx));
    assert_eq!(block_text(&state, 0), "a\n\n");
    assert_eq!(block_text(&state, 1), "b\n");
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()), Some(0));
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|e| e.selection()), Some(3..3));

    assert!(state.block_key(0, &command('z', true), &mut tx));
    assert_eq!(block_text(&state, 1), "xb\n");
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()), Some(1));
}

#[test]
fn typing_into_the_last_trailing_blank_line_splits_the_field() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Trailing", "a\n\n");
    enter_editor(&mut state);
    assert_eq!(state.block_count(), 1);
    state.block_editor_mut_for_test(0).set_caret(3);

    let mut tx = Tx::default();
    state.active_editor_edit().insert("x");
    state.block_changed(0, &mut tx);
    assert_eq!(state.block_count(), 2);
    assert_eq!(block_text(&state, 0), "a\n\n");
    assert_eq!(block_text(&state, 1), "x");
    assert_eq!(editor_text(&state), "a\n\nx");
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()), Some(1));

    assert!(state.block_key(1, &command('z', false), &mut tx));
    assert_eq!(state.block_count(), 1);
    assert_eq!(editor_text(&state), "a\n\n");
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()), Some(0));
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|e| e.selection()), Some(3..3));
}

#[test]
fn merge_undo_and_redo_restore_the_caret_at_the_merged_boundary() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Merge caret", "a\n\nb\n\nc\n");
    enter_editor(&mut state);
    assert_eq!(state.block_count(), 3);
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(0));
    state.block_editor_mut_for_test(0).set_caret(3);

    let mut tx = Tx::default();
    assert!(state.block_key(0, &plain_key(Key::Delete), &mut tx));
    assert_eq!(state.block_count(), 2);
    assert_eq!(block_text(&state, 0), "a\n\nb\n\n");
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()), Some(0));
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|e| e.selection()), Some(3..3));

    assert!(state.block_key(0, &command('z', false), &mut tx));
    assert_eq!(state.block_count(), 3, "undo splits the merge again");
    assert_eq!(
        state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()),
        Some(1),
        "the right field owns the boundary caret"
    );
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|e| e.selection()), Some(3..3));

    assert!(state.block_key(1, &command('z', true), &mut tx));
    assert_eq!(state.block_count(), 2, "redo merges again");
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(|e| e.active_block()), Some(0));
    assert_eq!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|e| e.selection()), Some(3..3));
}

#[test]
fn undo_of_a_split_focuses_the_merged_field_through_the_runtime() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    set_large_file_mode(&state, LargeFileMode::Block);
    state.open_synthetic("Caret", "onetwo\n\nthree\n");
    enter_editor(&mut state);
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").set_active_block(Some(0));
    state.block_editor_mut_for_test(0).set_caret(3);

    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    runtime.key(plain_key(Key::Enter));
    runtime.frame(1.0 / 60.0);
    runtime.key(plain_key(Key::Enter));
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.block_count(), 3);
    let focused = runtime.elements();
    assert!(
        focused.iter().any(|element| element.key.as_deref() == Some("editor-block-1") && element.focused),
        "the split focused the new field"
    );

    runtime.key(command('z', false));
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.block_count(), 2, "the split is undone with the newline");
    let elements = runtime.elements();
    assert!(
        elements.iter().any(|element| element.key.as_deref() == Some("editor-block-0") && element.focused),
        "undo focuses the merged field"
    );
    let editor = runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("editor");
    assert_eq!(editor.active_block(), Some(0));
    assert_eq!(editor.selection(), 4..4);

    runtime.key(command('z', true));
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.block_count(), 3, "redo splits again");
    let elements = runtime.elements();
    assert!(
        elements.iter().any(|element| element.key.as_deref() == Some("editor-block-1") && element.focused),
        "redo focuses the right field"
    );
    let editor = runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("editor");
    assert_eq!(editor.active_block(), Some(1));
    assert_eq!(editor.selection(), 5..5);

    // Backspace at the start of the right field merges it back; undo and redo
    // keep the boundary caret and focus in the field that owns it.
    runtime.key(plain_key(Key::Backspace));
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.block_count(), 2, "Backspace merged the block");
    let elements = runtime.elements();
    assert!(
        elements.iter().any(|element| element.key.as_deref() == Some("editor-block-0") && element.focused),
        "the merged field took the focus"
    );
    runtime.key(command('z', false));
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.block_count(), 3, "undo splits the merge again");
    let elements = runtime.elements();
    assert!(
        elements.iter().any(|element| element.key.as_deref() == Some("editor-block-1") && element.focused),
        "the right field owns the boundary caret after undo"
    );
    let editor = runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("editor");
    assert_eq!(editor.selection(), 5..5);
    runtime.key(command('z', true));
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.block_count(), 2, "redo merges again");
    let elements = runtime.elements();
    assert!(
        elements.iter().any(|element| element.key.as_deref() == Some("editor-block-0") && element.focused),
        "the merged field keeps the focus after redo"
    );
    let editor = runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("editor");
    assert_eq!(editor.selection(), 5..5);
}
