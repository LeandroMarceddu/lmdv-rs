//! P3-04b: live task checkboxes in the reader (M-07, E-08).
//!
//! These tests drive the real parser and the real jobs: marker location is pure
//! (`MdDoc` + `TaskRef`), while toggles use a temporary file and the save job's
//! completion, exactly as the window does.

use std::path::Path;
use std::time::{Duration, Instant};

use lmdv_core::{DocStatus, DocumentId};
use lmdv_md::{Block, MdDoc};
use lmdv_render::TaskRef;
use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime, Tx};
use tempfile::TempDir;

use crate::editor::Prompt;
use crate::tasks::{Candidate, TASK_UNDO_SECONDS, candidates_for_block, locate_marker, task_hit_key};
use crate::test_support;
use crate::view::view;
use crate::window::{Mode, NoticeAction, Tab, WindowState};

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

fn open_loaded(state: &mut WindowState, path: &Path) -> String {
    state.open_path(path.to_path_buf());
    poll_until(state, "the file load", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| matches!(*doc.status.borrow(), DocStatus::Ready))
    });
    poll_until(state, "the first parse", |state| state.active_tab().is_some_and(|tab| tab.view.reader.is_some()));
    task_list_key(state)
}

fn task_list_key(state: &WindowState) -> String {
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the parsed model");
    for (index, _) in model.blocks.iter().enumerate() {
        let key = model.block_key(index).expect("a block key").to_owned();
        if !candidates_for_block(&model, &key).is_empty() {
            return key;
        }
    }
    panic!("the fixture has no task block");
}

fn model(state: &WindowState) -> std::sync::Arc<MdDoc> {
    state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the parsed model")
}

fn task(state: &WindowState, block: &str, label: &str) -> TaskRef {
    let candidate = candidates_for_block(&model(state), block)
        .into_iter()
        .find(|candidate| candidate.label == label)
        .unwrap_or_else(|| panic!("no task labelled {label:?} in {block}"));
    TaskRef {
        block: block.to_owned(),
        item: candidate.item,
        checked: candidate.checked,
        label: candidate.label,
        source: candidate.marker,
    }
}

fn wait_for_task_save(state: &mut WindowState) {
    poll_until(state, "the task save", |state| state.tasks.pending() == 0 && state.worker().is_empty());
}

fn candidates(doc: &MdDoc, block: &str) -> Vec<Candidate> {
    candidates_for_block(doc, block)
}

fn marker_text(doc: &MdDoc, marker: &std::ops::Range<usize>) -> String {
    doc.source[marker.clone()].to_owned()
}

#[test]
fn markers_are_found_for_first_second_nested_math_and_links() {
    let source = "- [ ] first\n- [x] second\n- [ ] math $E = mc^2$ stays\n  - [ ] nested child\n- [ ] read [the note](note.md) now\n";
    let doc = lmdv_md::parse(source);
    let key = doc.block_key(0).expect("block key");

    let what = candidates(&doc, key);
    assert_eq!(what.len(), 5, "every task item is a candidate: {what:#?}");
    let labels: Vec<_> = what.iter().map(|candidate| candidate.label.as_str()).collect();
    assert_eq!(labels, ["first", "second", "math stays", "nested child", "read the note now"]);
    assert_eq!(marker_text(&doc, &what[0].marker), "[ ]", "the first item's own marker is blank");
    assert_eq!(marker_text(&doc, &what[1].marker), "[x]");

    let task = TaskRef {
        block: key.to_owned(),
        item: 1,
        checked: true,
        label: "second".to_owned(),
        source: what[1].marker.clone(),
    };
    assert_eq!(marker_text(&doc, &locate_marker(&doc, &task).expect("second")), "[x]");

    // The nested item repeats the outer list's index 0; its source range
    // identifies it without relying on the reconstructed label (OI-092).
    let nested = TaskRef {
        block: key.to_owned(),
        item: 0,
        checked: false,
        label: "nested child".to_owned(),
        source: what[3].marker.clone(),
    };
    assert_eq!(&doc.source[locate_marker(&doc, &nested).expect("nested")], "[ ]");
}

#[test]
fn ordered_and_bullet_lists_keep_their_own_index() {
    let source = "1. [ ] one\n2. [x] two\n\n- [ ] three\n";
    let doc = lmdv_md::parse(source);
    assert_eq!(doc.blocks.len(), 2, "two top-level lists");
    let first = candidates(&doc, doc.block_key(0).unwrap());
    let second = candidates(&doc, doc.block_key(1).unwrap());
    assert_eq!(first.iter().map(|c| c.item).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(second[0].item, 0, "the second list counts from zero again");
    assert_eq!(marker_text(&doc, &first[1].marker), "[x]");
    assert_eq!(marker_text(&doc, &second[0].marker), "[ ]");
}

#[test]
fn an_ambiguous_marker_is_not_written() {
    let source = "> - [ ] same\n>\n> between\n>\n> - [ ] same\n";
    let doc = lmdv_md::parse(source);
    let key = doc.block_key(0).expect("block key");
    assert_eq!(candidates(&doc, key).len(), 2, "two lists in one quote, both at item 0");
    let task = TaskRef { block: key.to_owned(), item: 0, checked: false, label: "same".to_owned(), source: 0..0 };
    assert!(locate_marker(&doc, &task).is_none(), "without a source range the two candidates are indistinguishable");

    // A renderer-provided source range resolves it without the label (OI-092).
    let keyed = TaskRef { source: candidates(&doc, key)[1].marker.clone(), ..task };
    assert_eq!(&doc.source[locate_marker(&doc, &keyed).expect("the second marker")], "[ ]");
}

#[test]
fn a_checkbox_tap_writes_the_file_and_its_undo_restores_it() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("tasks.md");
    let original = "# Work\n\n- [ ] first\n- [ ] second\n";
    std::fs::write(&path, original).unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let first = task(&state, &block, "first");
    let second = task(&state, &block, "second");

    state.toggle_task(&first);
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Work\n\n- [x] first\n- [ ] second\n");
    assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.action.is_some()), "the notice offers Undo");
    assert!(matches!(
        state.ui.notice.as_ref().and_then(|notice| notice.action.clone()),
        Some(NoticeAction::TaskUndo(_))
    ));

    // A second tick replaces the pending undo: only the newest toggle goes back.
    state.toggle_task(&second);
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Work\n\n- [x] first\n- [x] second\n");
    let Some(NoticeAction::TaskUndo(undo)) = state.ui.notice.as_ref().and_then(|notice| notice.action.clone()) else {
        panic!("the newest toggle has an undo");
    };
    assert_eq!(undo.range.start + 1, std::fs::read_to_string(&path).unwrap().rfind("[x]").unwrap() + 1);
    state.run_notice_action();
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Work\n\n- [x] first\n- [ ] second\n");
    assert!(state.ui.notice.is_none(), "undo dismisses the notice");

    // The first item is still checked, and its own Undo is gone.
    state.toggle_task(&first);
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Work\n\n- [ ] first\n- [ ] second\n");
}

#[test]
fn a_checked_task_becomes_unchecked_and_back() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("checked.md");
    std::fs::write(&path, "- [x] done\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let done = task(&state, &block, "done");

    state.toggle_task(&done);
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] done\n");

    state.run_notice_action();
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [x] done\n");
}

#[test]
fn crlf_is_preserved_and_a_bom_is_not_readded() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("crlf.md");
    std::fs::write(&path, b"\xEF\xBB\xBF- [ ] one\r\n- [x] two\r\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    state.toggle_task(&task(&state, &block, "one"));
    wait_for_task_save(&mut state);

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes, b"- [x] one\r\n- [x] two\r\n", "CRLF survives and the BOM is never written back");
}

#[test]
fn a_successful_tick_cleans_the_base_and_reparses_the_article() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("reparse.md");
    std::fs::write(&path, "- [ ] one\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    state.toggle_task(&task(&state, &block, "one"));
    wait_for_task_save(&mut state);
    poll_until(&mut state, "the reparse", |state| {
        let doc = state.active_tab().and_then(Tab::document).expect("document");
        state.active_tab().and_then(|tab| tab.view.reader_revision) == Some(doc.revision())
    });
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert!(!doc.is_dirty(), "the write became the saved base");
    assert!(
        state.shared().docs().find_identity(&doc.saved.borrow().meta).is_some_and(|found| found.id == doc.id),
        "the registry follows the atomic write's inode"
    );
    assert!(state.shared().latest_parsed(doc.id).is_some_and(|parsed| parsed.revision == doc.revision()));
    let candidate = candidates_for_block(&model(&state), &block)
        .into_iter()
        .find(|candidate| candidate.label == "one")
        .expect("the task item");
    assert!(candidate.checked, "the reparsed model shows the tick");
}

#[test]
fn a_write_error_reverts_the_tick_and_reports() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("error.md");
    std::fs::write(&path, "- [ ] one\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();

    state.toggle_task(&task(&state, &block, "one"));
    wait_for_task_save(&mut state);
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.text().text(), "- [ ] one\n", "the failed tick was reverted");
    assert!(!doc.is_dirty());
    assert!(
        state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("Could not update")),
        "{:?}",
        state.ui.notice
    );
}

#[test]
fn a_conflict_restores_the_buffer_and_offers_overwrite_or_keep_theirs() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("conflict.md");
    std::fs::write(&path, "- [ ] mine\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let mine = task(&state, &block, "mine");
    std::fs::write(&path, "- [ ] theirs\n").unwrap();

    state.toggle_task(&mine);
    poll_until(&mut state, "the task conflict prompt", |state| {
        matches!(state.prompt(), Some(Prompt::TaskConflict { .. }))
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] theirs\n", "nothing was overwritten");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.text().text(), "- [ ] mine\n", "the optimistic tick was reverted");
    assert!(!doc.is_dirty(), "no unsaved viewer write is left behind");

    let mut tx = Tx::default();
    state.prompt_primary(&mut tx);
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [x] mine\n", "Overwrite writes the tick");
}

#[test]
fn keep_theirs_reloads_the_disk_version() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("keep.md");
    std::fs::write(&path, "- [ ] mine\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let mine = task(&state, &block, "mine");
    std::fs::write(&path, "- [ ] theirs\n").unwrap();

    state.toggle_task(&mine);
    poll_until(&mut state, "the task conflict prompt", |state| {
        matches!(state.prompt(), Some(Prompt::TaskConflict { .. }))
    });
    let mut tx = Tx::default();
    state.prompt_secondary(&mut tx);
    poll_until(&mut state, "the reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.text().text() == "- [ ] theirs\n")
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] theirs\n");
    assert!(state.prompt().is_none());
}

#[test]
fn a_text_click_never_toggles_or_writes() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("text.md");
    let original = "- [ ] open item\n";
    std::fs::write(&path, original).unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let wire = task_hit_key(&task(&state, &block, "open item"));
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);
    assert!(runtime.find(&wire).is_some(), "the checkbox hit target exists");
    let text = runtime
        .elements()
        .into_iter()
        .find(|element| {
            element.role == "text" && element.text.as_deref().is_some_and(|text| text.contains("open item"))
        })
        .expect("the item text");
    let (x, y) = text_center(&text);
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    for _ in 0..4 {
        runtime.update(|state, tx| {
            state.poll(0.0, tx);
        });
        runtime.frame(1.0 / 60.0);
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "a text click selects, it never writes");
    assert_eq!(runtime.state.tasks.pending(), 0);
    assert!(runtime.state.ui.notice.is_none());
}

#[test]
fn the_checkbox_hit_target_toggles_through_a_scripted_tap() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("tap.md");
    std::fs::write(&path, "- [ ] tap me\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let key = task_hit_key(&task(&state, &block, "tap me"));
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);
    let (x, y) = runtime.find(&key).expect("the hit target is laid out");
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(1.0 / 60.0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.state.tasks.pending() > 0 {
        assert!(Instant::now() < deadline, "the tap's save never finished");
        runtime.update(|state, tx| {
            state.poll(0.0, tx);
        });
        runtime.frame(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [x] tap me\n");
}

#[test]
fn the_viewer_tree_has_one_real_checkbox_per_item() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("boxes.md");
    std::fs::write(&path, "- [ ] first\n- [x] second\n  - [ ] nested child\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let keys: Vec<String> = ["first", "second", "nested child"]
        .into_iter()
        .map(|label| task_hit_key(&task(&state, &block, label)))
        .collect();
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    let boxes: Vec<ElementInfo> = runtime.elements().into_iter().filter(|element| element.role == "checkbox").collect();
    assert_eq!(boxes.len(), 3, "no overlay duplicates the checkboxes");
    for key in &keys {
        let found: Vec<_> = boxes.iter().filter(|element| element.key.as_deref() == Some(key.as_str())).collect();
        assert_eq!(found.len(), 1, "exactly one control carries {key}");
        assert!(found[0].focusable, "the real checkbox takes keyboard focus");
        assert!(found[0].label.as_deref().is_some_and(|label| !label.is_empty()));
    }
    assert!(boxes.iter().any(|element| element.label.as_deref() == Some("nested child")));
}

#[test]
fn space_and_enter_press_the_focused_checkbox() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("keys.md");
    std::fs::write(&path, "- [ ] first\n- [ ] second\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let first = task_hit_key(&task(&state, &block, "first"));
    let second = task_hit_key(&task(&state, &block, "second"));
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);

    runtime.update(|_state, tx| tx.focus(second.clone(), false));
    runtime.frame(1.0 / 60.0);
    runtime.key(plain_key(Key::Enter));
    runtime.frame(1.0 / 60.0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.state.tasks.pending() > 0 {
        assert!(Instant::now() < deadline, "the Enter save never finished");
        runtime.update(|state, tx| {
            state.poll(0.0, tx);
        });
        runtime.frame(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] first\n- [x] second\n");

    runtime.update(|_state, tx| tx.focus(first.clone(), false));
    runtime.frame(1.0 / 60.0);
    runtime.key(plain_key(Key::Text(" ".to_owned())));
    runtime.frame(1.0 / 60.0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.state.tasks.pending() > 0 {
        assert!(Instant::now() < deadline, "the Space save never finished");
        runtime.update(|state, tx| {
            state.poll(0.0, tx);
        });
        runtime.frame(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [x] first\n- [x] second\n");
}

#[test]
fn the_notice_button_runs_the_undo_write() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("notice-undo.md");
    std::fs::write(&path, "- [ ] one\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    state.toggle_task(&task(&state, &block, "one"));
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [x] one\n");

    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);
    let (x, y) = runtime.find("notice-action").expect("the notice's Undo button");
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(1.0 / 60.0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.state.tasks.pending() > 0 {
        assert!(Instant::now() < deadline, "the undo's save never finished");
        runtime.update(|state, tx| {
            state.poll(0.0, tx);
        });
        runtime.frame(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] one\n");
    assert!(runtime.state.ui.notice.is_none(), "the notice closed");
}

#[test]
fn edit_mode_has_no_live_task_wire() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("edit.md");
    std::fs::write(&path, "- [ ] still text\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx));
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));
    let elements = run_view(state);
    assert!(
        !elements.iter().any(|element| element.key.as_deref().is_some_and(|key| key.starts_with("task-hit-"))),
        "the editor's preview has no task hit targets"
    );
    let checkboxes: Vec<_> = elements.iter().filter(|element| element.role == "checkbox").collect();
    assert!(!checkboxes.is_empty(), "the preview still draws read-only checkboxes");
    assert!(checkboxes.iter().all(|element| !element.focusable), "they cannot take keyboard focus in edit mode");
}

#[test]
fn a_checkbox_tap_in_the_editor_preview_never_writes() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("edit-click.md");
    std::fs::write(&path, "- [ ] read only\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx));
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);
    let checkbox = runtime
        .elements()
        .into_iter()
        .find(|element| element.role == "checkbox")
        .expect("the preview draws the checkbox");
    let (x, y) = text_center(&checkbox);
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    for _ in 0..4 {
        runtime.update(|state, tx| {
            state.poll(0.0, tx);
        });
        runtime.frame(1.0 / 60.0);
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] read only\n");
    assert_eq!(runtime.state.tasks.pending(), 0);
}

#[test]
fn a_dirty_document_keeps_the_tick_in_the_buffer_without_writing() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("dirty.md");
    std::fs::write(&path, "- [ ] first\n- [ ] second\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let first = task(&state, &block, "first");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    let len = doc.text().text().len();
    doc.apply(len..len, "<!-- draft -->\n");
    state.sync_tab_dirty(&doc);
    let before = std::fs::read_to_string(&path).unwrap();

    state.toggle_task(&first);
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "a dirty editor owns the write");
    assert!(doc.text().text().contains("- [x] first"));
    let Some(NoticeAction::TaskUndo(undo)) = state.ui.notice.as_ref().and_then(|notice| notice.action.clone()) else {
        panic!("a buffer-only tick still offers Undo (OI-093)");
    };
    assert!(undo.buffer_only, "the inverse does not write the dirty buffer to disk");
    assert_eq!(undo.expected_meta, Some(doc.saved.borrow().meta));
    assert_eq!(undo.path, path);

    state.run_notice_action();
    assert!(doc.text().text().contains("- [ ] first"), "the byte is back in the buffer");
    assert!(doc.text().text().contains("<!-- draft -->"), "the user's own edits survive");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("buffer") && notice.action.is_none()));
}

#[test]
fn the_notice_expires_the_undo_action() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("expire.md");
    std::fs::write(&path, "- [ ] first\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    state.toggle_task(&task(&state, &block, "first"));
    wait_for_task_save(&mut state);
    assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.action.is_some()));
    let mut tx = Tx::default();
    state.poll(TASK_UNDO_SECONDS + 1.0, &mut tx);
    assert!(state.ui.notice.is_none(), "the undo window closes");
}

#[test]
fn a_missing_marker_does_not_write() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("missing.md");
    std::fs::write(&path, "- [ ] present\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let gone = TaskRef { block, item: 9, checked: false, label: "not here".to_owned(), source: 0..0 };
    state.toggle_task(&gone);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] present\n");
    assert_eq!(state.tasks.pending(), 0);
    assert!(state.ui.notice.is_some());
}

#[test]
fn the_undo_of_a_different_document_is_refused() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("moved.md");
    std::fs::write(&path, "- [ ] one\n").unwrap();

    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    state.toggle_task(&task(&state, &block, "one"));
    wait_for_task_save(&mut state);
    let Some(NoticeAction::TaskUndo(mut undo)) = state.ui.notice.as_ref().and_then(|notice| notice.action.clone())
    else {
        panic!("an undo");
    };
    undo.doc = DocumentId(9999);
    state.undo_task(undo);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [x] one\n", "nothing was written");
}

fn text_center(element: &ElementInfo) -> (f32, f32) {
    (element.rect[0] + element.rect[2] * 0.5, element.rect[1] + element.rect[3] * 0.5)
}

fn plain_key(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn run_view(state: WindowState) -> Vec<ElementInfo> {
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.elements()
}

#[test]
fn the_task_block_helper_finds_the_fixture_list() {
    // A guard for the test helper itself: the first task list in the house
    // fixture is a top-level list with four items.
    let doc = lmdv_md::parse(include_str!("../../../tests/fixtures/test-features.md"));
    let mut found = None;
    for (index, block) in doc.blocks.iter().enumerate() {
        if matches!(block, Block::List { .. }) && !candidates(&doc, doc.block_key(index).unwrap()).is_empty() {
            found = Some(doc.block_key(index).unwrap().to_owned());
            break;
        }
    }
    let key = found.expect("the fixture has a task list");
    assert_eq!(candidates(&doc, &key).len(), 4);
}

#[test]
fn the_tick_notice_is_localized_and_actionable() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("notice.md");
    std::fs::write(&path, "- [ ] one\n").unwrap();
    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    state.toggle_task(&task(&state, &block, "one"));
    wait_for_task_save(&mut state);
    let notice = state.ui.notice.as_ref().expect("a notice");
    assert!(notice.text.contains("Task checked"), "{}", notice.text);
    assert_eq!(notice.action.as_ref().map(NoticeAction::label), Some("Undo"));
    assert!(notice.deadline > state.ui.last_now);
    assert!((notice.deadline - state.ui.last_now - TASK_UNDO_SECONDS).abs() < 0.001);
}

#[test]
fn a_pasted_tick_offers_an_undo_that_stays_in_the_buffer() {
    let temp = TempDir::new().unwrap();
    let state = window(&temp);
    let board = rune::clipboard::memory();
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.use_clipboard(&board);
    board.set_text("- [ ] pasted item\n");
    runtime.update(|state, tx| {
        assert!(state.run_command("file.paste", tx), "paste is available from the start page");
    });
    runtime.frame(0.0);
    poll_until(&mut runtime.state, "the pasted parse", |state| {
        state.active_tab().is_some_and(|tab| tab.view.reader.is_some())
    });
    let block = task_list_key(&runtime.state);
    let pasted = task(&runtime.state, &block, "pasted item");
    let doc = runtime.state.active_tab().and_then(Tab::document).cloned().expect("document");

    runtime.state.toggle_task(&pasted);
    assert_eq!(doc.text().text(), "- [x] pasted item\n");
    assert_eq!(runtime.state.tasks.pending(), 0, "pasted text has no file to write");
    let Some(NoticeAction::TaskUndo(undo)) = runtime.state.ui.notice.as_ref().and_then(|notice| notice.action.clone())
    else {
        panic!("the pasted tick offers Undo (OI-093)");
    };
    assert!(undo.buffer_only, "the inverse never starts a save for untitled text");
    assert!(undo.expected_meta.is_none(), "there is no file metadata to guard");
    assert!(undo.path.as_os_str().is_empty());

    runtime.state.run_notice_action();
    assert_eq!(doc.text().text(), "- [ ] pasted item\n");
    assert!(runtime.state.ui.notice.as_ref().is_some_and(|notice| notice.action.is_none()));
}

#[test]
fn a_buffer_undo_refuses_a_changed_saved_base() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("stale-base.md");
    std::fs::write(&path, "- [ ] one\n").unwrap();
    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    let len = doc.text().text().len();
    doc.apply(len..len, "<!-- draft -->\n");
    state.sync_tab_dirty(&doc);
    state.toggle_task(&task(&state, &block, "one"));
    let Some(NoticeAction::TaskUndo(undo)) = state.ui.notice.as_ref().and_then(|notice| notice.action.clone()) else {
        panic!("a buffer-only Undo");
    };

    // Another window saved in the meantime: the base the Undo was offered
    // against is gone, so the byte must stay as the reader shows it.
    let mut meta = doc.saved.borrow().meta;
    meta.len += 7;
    doc.set_saved(meta, doc.hash(), path.clone());
    state.undo_task(undo);
    assert!(doc.text().text().contains("- [x] one"), "the stale undo was refused");
    assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("Undo failed")));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] one\n");
}

#[test]
fn a_second_tick_while_saving_is_queued_and_both_ticks_land() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("queued.md");
    let original = "- [ ] first\n- [ ] second\n";
    std::fs::write(&path, original).unwrap();
    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let first = task(&state, &block, "first");
    let second = task(&state, &block, "second");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");

    state.toggle_task(&first);
    assert_eq!(state.tasks.pending(), 1);
    assert_eq!(state.tasks.queued(doc.id), 0);
    state.toggle_task(&second);
    assert_eq!(state.tasks.pending(), 1, "the second tick did not start a competing job");
    assert_eq!(state.tasks.queued(doc.id), 1, "the second tick was queued instead of refused");
    assert!(doc.text().text().contains("- [x] first"));
    assert!(doc.text().text().contains("- [x] second"));

    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [x] first\n- [x] second\n");
    assert_eq!(state.tasks.queued(doc.id), 0);
    let Some(NoticeAction::TaskUndo(undo)) = state.ui.notice.as_ref().and_then(|notice| notice.action.clone()) else {
        panic!("the queued tick's save offers Undo");
    };
    assert_eq!(undo.range, second.source, "the notice names the last tick");
    assert!(!undo.buffer_only);
    assert_eq!(undo.expected_meta, Some(doc.saved.borrow().meta));

    state.run_notice_action();
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [x] first\n- [ ] second\n");
}

#[test]
fn two_ticks_on_one_item_while_saving_leave_the_disk_untouched() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("same-item.md");
    let original = "- [ ] one\n";
    std::fs::write(&path, original).unwrap();
    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let one = task(&state, &block, "one");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");

    state.toggle_task(&one);
    assert_eq!(state.tasks.pending(), 1);
    state.toggle_task(&one);
    assert_eq!(state.tasks.queued(doc.id), 1);
    assert_eq!(doc.text().text(), original, "the second click put the byte back");

    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "the follow-up rewrote the original bytes");
    assert!(!doc.is_dirty());
    let Some(NoticeAction::TaskUndo(undo)) = state.ui.notice.as_ref().and_then(|notice| notice.action.clone()) else {
        panic!("the second click has its own Undo");
    };
    assert_eq!(undo.range, one.source);
    assert_eq!((undo.before, undo.after), (b'x', b' '));
    state.run_notice_action();
    wait_for_task_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [x] one\n", "Undo restores the first tick");
}

#[test]
fn a_conflict_with_a_queued_tick_reverts_every_optimistic_byte() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("queued-conflict.md");
    let original = "- [ ] first\n- [ ] second\n";
    std::fs::write(&path, original).unwrap();
    let mut state = window(&temp);
    let block = open_loaded(&mut state, &path);
    let first = task(&state, &block, "first");
    let second = task(&state, &block, "second");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");

    state.toggle_task(&first);
    assert_eq!(state.tasks.pending(), 1, "the first save is in flight");
    // Change disk while that save is provably in flight, before the queued
    // tick is added. Otherwise the save can finish first and no conflict
    // happens (the race this test used to flake on).
    std::fs::write(&path, "- [ ] theirs\n").unwrap();
    state.toggle_task(&second);
    assert_eq!(state.tasks.queued(doc.id), 1, "the second tick was queued");
    poll_until(&mut state, "the task conflict", |state| matches!(state.prompt(), Some(Prompt::TaskConflict { .. })));
    assert_eq!(doc.text().text(), original, "neither optimistic byte is left behind");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] theirs\n", "the disk version wins");
    assert_eq!(state.tasks.queued(doc.id), 0, "the pending follow-up was dropped");
}
