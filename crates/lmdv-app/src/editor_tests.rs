//! P3 integration: edit mode, document sync, saving, EOL, conflicts, drafts,
//! the watcher and the close guards.
//!
//! These tests drive `WindowState` directly (no window), with real files in a
//! temporary directory. Jobs run on Rune's task pool exactly as they do in the
//! window; the helpers poll until the completion lands.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lmdv_core::{DocStatus, Draft, Eol, LargeFileMode, ScrollAnchor};
use lmdv_edit::spell::{Availability, DictionaryFiles, Spell};
use lmdv_io::watch::FileEvent;
use rune::{ElementInfo, Tx};
use rune_kit::syntax::SyntaxTheme;
use tempfile::{NamedTempFile, TempDir};

use crate::editor::Prompt;
use crate::reader;
use crate::shared::AppShared;
use crate::test_support;
use crate::view::view;
use crate::window::{Mode, Tab, WindowState};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

/// Two windows on one shared state, as the P4 runtime windows are.
fn two_windows(temp: &TempDir) -> (Rc<AppShared>, WindowState, WindowState) {
    let shared = test_support::shared(temp);
    let first = WindowState::new(shared.clone());
    let second = WindowState::new(shared.clone());
    (shared, first, second)
}

fn poll_once(state: &mut WindowState) {
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
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

fn open_loaded(state: &mut WindowState, path: &Path) {
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

fn type_text(state: &mut WindowState, text: &str) {
    state.active_editor_edit().set_caret(0);
    state.active_editor_edit().insert(text);
    state.editor_changed();
}

fn wait_for_save(state: &mut WindowState) {
    poll_until(state, "the save", |state| {
        !state.editor.after_save.contains_key(&state.active_document_id().unwrap_or(lmdv_core::DocumentId(0)))
            && state.worker().is_empty()
    });
}

fn flush_store(state: &WindowState) {
    assert!(state.shared().flush_store(Duration::from_secs(2)));
}

#[test]
fn edit_marks_dirty_and_save_writes_the_file_and_cleans_it() {
    let temp = TempDir::new().unwrap();
    let mut file = NamedTempFile::new_in(temp.path()).unwrap();
    write!(file, "# Title\n\nBody\n").unwrap();
    file.flush().unwrap();
    let path = file.path().to_path_buf();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "New ");

    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert!(state.active_tab().is_some_and(|tab| tab.view.dirty), "the tab reads dirty");
    assert!(doc.is_dirty(), "the shared document is dirty");
    assert!(doc.text().text().starts_with("New # Title"), "the editor delta reached the document");
    assert!(state.window_title().contains('•'), "the title carries the dirty dot");

    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    wait_for_save(&mut state);

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "New # Title\n\nBody\n");
    assert!(!doc.is_dirty(), "saving recorded the new base");
    let saved_meta = doc.saved.borrow().meta;
    assert!(
        state.shared().docs().find_identity(&saved_meta).is_some_and(|found| found.id == doc.id),
        "the registry follows the atomic write's new inode"
    );
    assert!(state.active_tab().is_some_and(|tab| !tab.view.dirty));
    assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.text == "Saved"));
    assert!(state.saved_flash(), "the saved check is showing");
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor), "saving stays in edit mode");
}

#[test]
fn crlf_and_a_bom_round_trip_without_readding_the_bom() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("crlf.md");
    std::fs::write(&path, b"\xEF\xBB\xBFone\r\ntwo\r\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.text().text(), "one\ntwo\n", "the buffer is LF");
    assert_eq!(doc.text().eol(), Eol::CrLf);
    enter_editor(&mut state);
    let len = state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|editor| editor.text().len()).unwrap();
    state.active_editor_edit().set_caret(len);
    state.active_editor_edit().insert("three");
    state.editor_changed();

    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    wait_for_save(&mut state);

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes, b"one\r\ntwo\r\nthree", "CRLF is restored, the final newline policy is kept");
    assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "the BOM is never written back");
    assert_eq!(doc.text().eol(), Eol::CrLf, "the document keeps its line ending");
}

#[test]
fn a_conflict_asks_first_and_overwrite_wins() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("conflict.md");
    std::fs::write(&path, "mine\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "edited ");
    std::fs::write(&path, "theirs\n").unwrap();

    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    poll_until(&mut state, "the conflict prompt", |state| matches!(state.prompt(), Some(Prompt::SaveConflict { .. })));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n", "nothing was overwritten");

    state.prompt_primary(&mut tx);
    wait_for_save(&mut state);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "edited mine\n");
    assert!(state.prompt().is_none());
}

#[test]
fn a_conflict_keep_theirs_leaves_the_buffer_dirty_and_the_disk_copy_alone() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("keep.md");
    std::fs::write(&path, "mine\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "edited ");
    std::fs::write(&path, "theirs\n").unwrap();
    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    poll_until(&mut state, "the conflict prompt", |state| matches!(state.prompt(), Some(Prompt::SaveConflict { .. })));

    state.prompt_secondary(&mut tx);
    assert!(state.prompt().is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert!(doc.is_dirty(), "the buffer stays unsaved");
}

#[test]
fn a_crash_draft_is_offered_and_restore_enters_edit_mode_without_writing_the_file() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("draft.md");
    std::fs::write(&path, "on disk\n").unwrap();
    let dirs = test_support::dirs(&temp);
    let draft_text;

    {
        let shared = crate::shared::AppShared::start(dirs.clone());
        let mut state = WindowState::new(shared.clone());
        open_loaded(&mut state, &path);
        enter_editor(&mut state);
        type_text(&mut state, "draft ");
        draft_text = state.active_tab().and_then(Tab::document).map(|doc| doc.text().text().to_owned()).unwrap();
        state.flush_active_draft();
        flush_store(&state);
    }

    let shared = crate::shared::AppShared::start(dirs.clone());
    let mut state = WindowState::new(shared);
    open_loaded(&mut state, &path);
    poll_until(&mut state, "the draft offer", |state| matches!(state.prompt(), Some(Prompt::Draft { .. })));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "on disk\n", "the file was never touched");

    let mut tx = Tx::default();
    state.prompt_primary(&mut tx);
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.text().text(), draft_text);
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor), "Restore enters edit mode");
    assert!(doc.is_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "on disk\n");
}

#[test]
fn a_deleted_file_still_offers_its_draft_and_restore_leads_to_save_again() {
    let temp = TempDir::new().unwrap();
    let gone = temp.path().join("gone.md");
    let dirs = test_support::dirs(&temp);
    let handle = lmdv_io::store::StoreHandle::start(dirs.clone()).unwrap();
    let key = gone.to_string_lossy().into_owned();
    handle.patch(lmdv_io::store::StorePatch::Draft { key: key.clone(), draft: Draft::new("recovered text\n", "", 1) });
    assert!(handle.flush(Duration::from_secs(2)));
    drop(handle);

    let shared = crate::shared::AppShared::start(dirs.clone());
    let mut state = WindowState::new(shared);
    state.open_path(gone.clone());
    poll_until(&mut state, "the failed load's draft offer", |state| {
        matches!(state.prompt(), Some(Prompt::Draft { .. }))
    });
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("the error tab keeps a document");
    assert!(matches!(*doc.status.borrow(), DocStatus::Error(_)), "the missing file is the error it was");

    let mut tx = Tx::default();
    state.prompt_primary(&mut tx);
    assert_eq!(doc.text().text(), "recovered text\n", "Restore keeps the draft as the buffer");
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));
    assert!(doc.is_dirty());
    assert!(matches!(*doc.status.borrow(), DocStatus::Ready), "the restored buffer is a normal document");
    assert!(!gone.exists(), "Restore never writes the file");

    // The first save finds the file gone and asks Save again; that writes it.
    state.request_save_active(&mut tx);
    poll_until(&mut state, "the deleted-file save question", |state| {
        matches!(state.prompt(), Some(Prompt::FileDeleted { .. }))
    });
    state.prompt_primary(&mut tx);
    poll_until(&mut state, "Save again", |_| gone.exists());
    assert_eq!(std::fs::read_to_string(&gone).unwrap(), "recovered text\n");
    poll_until(&mut state, "the save completion", |state| !state.active_tab().is_some_and(|tab| tab.view.dirty));
    assert!(!doc.is_dirty(), "Save again made the buffer clean");
    // The draft removal is queued on the store thread; flush it before reading
    // the file, or a slow CI runner sees the old draft.
    assert!(state.shared().flush_store(Duration::from_secs(2)), "the draft removal is written");
    assert!(lmdv_io::store::load_draft(&dirs, &key).is_none(), "the saved draft is gone");
}

#[test]
fn a_draft_equal_to_the_file_is_dropped_silently() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("same.md");
    std::fs::write(&path, "same\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    let doc = state.active_document_id().expect("document");
    let key = path.to_string_lossy().into_owned();

    assert!(!state.handle_draft_loaded(doc, key.clone(), Some(Draft::new("same\n", "older\n", 1)), None));
    assert!(state.prompt().is_none(), "an identical draft is not offered");
    flush_store(&state);
    assert!(lmdv_io::store::load_draft(&test_support::dirs(&temp), &key).is_none(), "and it is removed");
}

#[test]
fn a_draft_is_not_offered_while_another_window_holds_the_edit_lease() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("shared-draft.md");
    std::fs::write(&path, "on disk\n").unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);
    open_loaded(&mut first, &path);
    open_loaded(&mut second, &path);
    enter_editor(&mut second);
    let doc = second.active_tab().and_then(Tab::document).cloned().expect("the shared document");
    assert_eq!(doc.lease_holder().map(|lease| lease.window), Some(second.id()), "the second window edits it");

    let key = path.to_string_lossy().into_owned();
    let draft = || Draft::new("draft text\n", "on disk\n", 1);
    assert!(
        !first.handle_draft_loaded(doc.id, key.clone(), Some(draft()), None),
        "E-17: another window's edit lease suppresses the offer"
    );
    assert!(first.prompt().is_none(), "nothing was offered over the other window's buffer");

    // Once that window leaves edit mode the draft is offered as usual.
    let mut tx = Tx::default();
    second.leave_editor(&mut tx);
    assert!(doc.lease_holder().is_none(), "the lease was released");
    assert!(first.handle_draft_loaded(doc.id, key, Some(draft()), None), "the draft is offered now");
    assert!(matches!(first.prompt(), Some(Prompt::Draft { .. })));
}

#[test]
fn a_clean_document_reloads_on_a_watch_change_and_keeps_its_anchor() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("watched.md");
    std::fs::write(&path, "one\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.anchor = ScrollAnchor { id: String::new(), offset: 120.0 };
    }
    std::fs::write(&path, "two\n").unwrap();
    assert!(state.apply_file_event(FileEvent::Changed { path: path.clone() }));
    poll_until(&mut state, "the watcher reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.text().text() == "two\n")
    });
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert_eq!(state.active_tab().map(|tab| tab.view.anchor.offset), Some(120.0), "the reading anchor survived");
}

#[test]
fn an_mtime_only_touch_updates_the_baseline_without_reloading() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("touched.md");
    std::fs::write(&path, "one\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    poll_until(&mut state, "the first parse", |state| {
        state.active_tab().is_some_and(|tab| tab.view.reader_revision.is_some())
    });
    if let Some(tab) = state.active_tab_mut() {
        tab.view.anchor = ScrollAnchor { id: String::new(), offset: 64.0 };
    }
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    let revision = doc.revision();
    let parse_revision = state.active_tab().and_then(|tab| tab.view.reader_revision);
    let old_meta = doc.saved.borrow().meta;

    // R-21: a `touch`-style metadata bump — same bytes, new mtime.
    let touched = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(touched))
        .unwrap();
    let touched_ns = std::fs::metadata(&path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    assert_ne!(old_meta.mtime_ns, touched_ns, "the test really changed the mtime");

    assert!(state.apply_file_event(FileEvent::Changed { path: path.clone() }));
    poll_until(&mut state, "the quiet reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.saved.borrow().meta.mtime_ns == touched_ns)
    });
    assert_eq!(doc.text().text(), "one\n", "the text did not change");
    assert_eq!(doc.revision(), revision, "the buffer was not replaced");
    assert_eq!(
        state.active_tab().and_then(|tab| tab.view.reader_revision),
        parse_revision,
        "the same text was not parsed again"
    );
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert_eq!(state.active_tab().map(|tab| tab.view.anchor.offset), Some(64.0), "the anchor was not restored");
    assert!(state.ui.notice.is_none(), "an attribute touch says nothing");
    assert!(state.prompt().is_none());
    assert!(!doc.is_dirty(), "the new baseline still matches the buffer");
    assert!(state.editor.compare_reloads.is_empty(), "the quiet reload completed");

    // Different text still reloads and reparses.
    std::fs::write(&path, "two\n").unwrap();
    assert!(state.apply_file_event(FileEvent::Changed { path }));
    poll_until(&mut state, "the real reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.text().text() == "two\n")
    });
}

#[test]
fn a_reopen_of_a_dirty_file_keeps_the_buffer_and_asks_about_a_newer_disk_copy() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("reopen.md");
    std::fs::write(&path, "on disk\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "edited ");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    let revision = doc.revision();
    assert_eq!(doc.text().text(), "edited on disk\n");

    // A same-spelling reopen with the same disk: nothing asks, nothing moves.
    state.open_path(path.clone());
    poll_once(&mut state);
    assert!(state.prompt().is_none(), "an unchanged disk asks nothing");
    assert_eq!(doc.text().text(), "edited on disk\n", "the dirty buffer stayed");

    // The disk changed: the buffer is still never read over; the existing
    // file-changed question and the header mark say so instead.
    std::fs::write(&path, "changed on disk\n").unwrap();
    state.open_path(path.clone());
    poll_until(&mut state, "the reopen's disk check", |state| {
        matches!(state.prompt(), Some(Prompt::DiskChanged { .. }))
    });
    assert_eq!(doc.text().text(), "edited on disk\n", "E-15: a dirty buffer is never read over");
    assert_eq!(doc.revision(), revision, "the reopen did not replace the buffer");
    assert!(state.active_tab().is_some_and(|tab| tab.view.disk_changed), "the header says the disk changed");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "changed on disk\n", "nothing was written");
}

#[test]
fn a_dirty_reopen_whose_buffer_already_matches_the_disk_keeps_the_buffer_dirty() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("matched.md");
    std::fs::write(&path, "on disk\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "edited ");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert!(doc.is_dirty());

    // Another program writes exactly what the dirty buffer holds. The saved
    // base is still older, so the buffer stays the user's to save; only the
    // baseline metadata follows the disk. The watch is dropped so this test
    // sees the reopen's own answer, not the watcher's concurrent one.
    state.shared().unwatch_path(&path);
    std::fs::write(&path, "edited on disk\n").unwrap();
    state.open_path(path.clone());
    poll_until(&mut state, "the reopen's disk check", |state| {
        state.editor.reopen_checks.is_empty() && state.worker().is_empty()
    });
    assert_eq!(doc.text().text(), "edited on disk\n");
    assert!(doc.is_dirty(), "the older saved base keeps the buffer dirty");
    assert!(state.prompt().is_none(), "an equal buffer needs no conflict question");
    assert_eq!(
        doc.saved.borrow().meta.len,
        std::fs::metadata(&path).unwrap().len(),
        "the baseline metadata followed the disk"
    );
}

#[test]
fn a_reopen_of_a_clean_file_reloads_it_at_its_reading_anchor() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("clean.md");
    std::fs::write(&path, "one\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.anchor = ScrollAnchor { id: String::new(), offset: 120.0 };
    }

    std::fs::write(&path, "two\n").unwrap();
    state.open_path(path.clone());
    poll_until(&mut state, "the reopen's reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.text().text() == "two\n")
    });
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert_eq!(state.active_tab().map(|tab| tab.view.anchor.offset), Some(120.0), "the reading anchor survived");
    assert!(!state.active_tab().is_some_and(|tab| tab.view.disk_changed), "a clean reload clears the mark");
}

#[test]
fn a_reopen_without_a_disk_change_keeps_the_buffer_and_its_parse() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("same.md");
    std::fs::write(&path, "one\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    poll_until(&mut state, "the first parse", |state| {
        state.active_tab().is_some_and(|tab| tab.view.reader_revision.is_some())
    });
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    let revision = doc.revision();
    let parsed = state.active_tab().and_then(|tab| tab.view.reader_revision);

    state.open_path(path);
    poll_until(&mut state, "the reopen's check", |state| {
        state.worker().is_empty() && state.editor.reopen_checks.is_empty()
    });
    assert_eq!(doc.revision(), revision, "the same text was not reparsed or replaced");
    assert_eq!(state.active_tab().and_then(|tab| tab.view.reader_revision), parsed);
    assert!(state.prompt().is_none());
    assert!(state.ui.notice.is_none());
}

#[test]
fn a_line_target_on_an_open_editing_tab_scrolls_the_source_and_the_preview() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("lines.md");
    std::fs::write(&path, scrolling_markdown(80)).unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    state.editor.sync_suppressed_until = None;
    state.editor.last_offset = Some(0.0);
    state.editor.last_caret = Some(0);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    assert_eq!(article_offset(&runtime), 0.0, "the preview starts at the top");
    assert_eq!(runtime.state.editor.scroll.get().offset, 0.0);

    // The same file reopened with `--line`: the tab is already in front, so the
    // target has to reach both panes by itself.
    runtime.update(|state, _| {
        state.open_launch(&lmdv_io::Launch { path: Some(path.clone()), line: Some(60), ..lmdv_io::Launch::default() });
    });
    for _ in 0..10 {
        runtime.frame(1.0 / 60.0);
    }
    assert!(runtime.state.editor.scroll.get().offset > 100.0, "the source scrolled to the line");
    assert!(article_offset(&runtime) > 100.0, "the preview scrolled to the block");
    let caret_line = {
        let editor = runtime.state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("the session");
        let caret = editor.selection().start;
        let text = editor.text();
        text[..caret.min(text.len())].bytes().filter(|&byte| byte == b'\n').count() + 1
    };
    assert_eq!(caret_line, 60, "the source caret moved to the requested line");
}

#[test]
fn a_watcher_rename_names_the_new_file_and_rehighlights_the_tree() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("notes");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("before-rename.md");
    let renamed = root.join("after-rename.md");
    std::fs::write(&path, "text\n").unwrap();

    let mut state = window(&temp);
    let list = |state: &mut WindowState| {
        state.browser.root = Some(root.clone());
        state.browser.root_name = "notes".to_owned();
        state.browser.tree_generation += 1;
        state.left.set_open(true);
        let generation = state.browser.tree_generation;
        let listing = lmdv_io::tree::list_tree(&root).unwrap();
        assert!(state.finish_tree_listing(generation, root.clone(), Ok(listing)));
    };
    list(&mut state);
    open_loaded(&mut state, &path);
    // `open_path`'s poll already ran `follow_browser_active`.
    assert_eq!(state.browser.active(), Some(path.to_string_lossy().as_ref()), "the open file is highlighted");

    std::fs::rename(&path, &renamed).unwrap();
    assert!(state.apply_file_event(FileEvent::Renamed { from: path.clone(), to: renamed.clone() }));
    assert_eq!(
        state.ui.notice.as_ref().map(|notice| notice.text.clone()),
        Some(lmdv_i18n::t!("msg.renamedTo", "name" => "after-rename.md")),
        "R-23: the rename is announced with the new name"
    );

    // The tree watcher re-lists the folder (F-08/A-11); the moved document
    // keeps its row highlighted under the new name.
    list(&mut state);
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert_eq!(state.browser.active(), Some(renamed.to_string_lossy().as_ref()));

    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.frame(0.0);
    let elements: Vec<ElementInfo> = runtime.elements();
    let row = elements
        .iter()
        .find(|element| element.key.as_deref() == Some(renamed.to_string_lossy().as_ref()))
        .expect("the renamed file has a tree row");
    assert!(row.item_selected, "the tree row carries the active document's highlight");
    assert!(
        !elements.iter().any(|element| element.key.as_deref() == Some(path.to_string_lossy().as_ref())),
        "the old row is gone"
    );
}

#[test]
fn a_dirty_document_never_auto_reloads_it_asks() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("dirty-watch.md");
    std::fs::write(&path, "one\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "mine ");
    std::fs::write(&path, "theirs\n").unwrap();
    assert!(state.apply_file_event(FileEvent::Changed { path: path.clone() }));

    assert!(matches!(state.prompt(), Some(Prompt::DiskChanged { .. })));
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.text().text(), "mine one\n", "the buffer was kept");
    let mut tx = Tx::default();
    state.prompt_secondary(&mut tx);
    assert!(state.prompt().is_none());
    assert_eq!(doc.text().text(), "mine one\n");
}

#[test]
fn a_rename_follows_the_document_registry_recents_and_positions() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("before.md");
    let renamed = temp.path().join("after.md");
    std::fs::write(&path, "text\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    state.shared().remember_position(&path, &ScrollAnchor { id: String::new(), offset: 64.0 });
    std::fs::rename(&path, &renamed).unwrap();

    assert!(state.apply_file_event(FileEvent::Renamed { from: path.clone(), to: renamed.clone() }));
    assert!(state.shared().docs().find_path(&path).is_none(), "the old path is forgotten");
    assert!(state.shared().docs().find_path(&renamed).is_some(), "the new path finds the document");
    assert_eq!(state.active_tab().map(|tab| tab.full_title()).as_deref(), Some("after.md"));
    assert_eq!(state.shared().positions().get(&renamed).map(|anchor| anchor.offset), Some(64.0));
    assert_eq!(state.shared().recents().first().map(|recent| recent.path.clone()), Some(renamed));
}

#[test]
fn a_rename_keeps_an_unsaved_buffer_dirty() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("dirty-before.md");
    let renamed = temp.path().join("dirty-after.md");
    std::fs::write(&path, "disk\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "mine ");
    std::fs::rename(&path, &renamed).unwrap();
    assert!(state.apply_file_event(FileEvent::Renamed { from: path, to: renamed.clone() }));

    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(*doc.key.borrow(), lmdv_core::DocumentKey::File(renamed.clone()));
    assert!(doc.is_dirty(), "the rename must not mark the buffer saved");
    assert!(state.active_tab().is_some_and(|tab| tab.view.dirty));
}

#[test]
fn reload_after_the_dirty_warning_replaces_the_buffer_and_clears_the_draft() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("reload.md");
    std::fs::write(&path, "disk\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "mine ");
    std::fs::write(&path, "external\n").unwrap();
    // R-22: the watcher warns; its Reload discards the edits (R-20's F5 does
    // nothing on a dirty buffer, tested below).
    assert!(state.apply_file_event(FileEvent::Changed { path: path.clone() }));
    let mut tx = Tx::default();
    assert!(matches!(state.prompt(), Some(Prompt::DiskChanged { .. })));
    state.prompt_primary(&mut tx);
    poll_until(&mut state, "the forced reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.text().text() == "external\n")
    });
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert!(!doc.is_dirty());
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor), "still editing the reloaded text");
}

#[test]
fn f5_reloads_a_clean_file_at_the_same_place() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("f5-clean.md");
    std::fs::write(&path, "one\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.anchor = ScrollAnchor { id: String::new(), offset: 120.0 };
    }
    std::fs::write(&path, "two\n").unwrap();

    let mut tx = Tx::default();
    let f5 = rune::KeyEvent { key: rune::Key::Function(5), mods: rune::Mods::default() };
    assert!(crate::app::on_key(&mut state, &f5, &mut tx), "F5 is the window's reload command");
    poll_until(&mut state, "the F5 reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.text().text() == "two\n")
    });
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert_eq!(state.active_tab().map(|tab| tab.view.anchor.offset), Some(120.0), "the reading anchor survived");
    assert!(state.ui.notice.is_none(), "a reload says nothing extra");
}

#[test]
fn ctrl_r_reloads_a_clean_edit_mode_file_at_the_same_place() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("ctrl-r-clean.md");
    std::fs::write(&path, "one\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.anchor = ScrollAnchor { id: String::new(), offset: 64.0 };
    }
    std::fs::write(&path, "two\n").unwrap();

    let mut tx = Tx::default();
    let ctrl_r = rune::KeyEvent {
        key: rune::Key::Command('r'),
        mods: rune::Mods { cmd: true, ctrl: true, ..rune::Mods::default() },
    };
    assert!(crate::app::on_key(&mut state, &ctrl_r, &mut tx), "Ctrl+R is the editor's reload");
    poll_until(&mut state, "the Ctrl+R reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.text().text() == "two\n")
    });
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor), "a clean reload stays in edit mode");
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert_eq!(state.active_tab().map(|tab| tab.view.anchor.offset), Some(64.0), "the reading anchor survived");
}

#[test]
fn f5_does_nothing_on_a_dirty_edit_mode_buffer() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("f5-dirty.md");
    std::fs::write(&path, "disk\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "mine ");
    std::fs::write(&path, "theirs\n").unwrap();

    let mut tx = Tx::default();
    let f5 = rune::KeyEvent { key: rune::Key::Function(5), mods: rune::Mods::default() };
    assert!(crate::app::on_key(&mut state, &f5, &mut tx), "the key is consumed");
    assert!(state.prompt().is_none(), "R-20: no disk-changed question from an explicit reload");

    for _ in 0..20 {
        state.poll(0.0, &mut tx);
        std::thread::sleep(Duration::from_millis(2));
    }
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.text().text(), "mine disk\n", "the dirty buffer was untouched");
    assert!(doc.is_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n", "the disk was untouched");
    assert!(state.editor.compare_reloads.is_empty(), "nothing was queued to reload");
}

#[test]
fn f5_does_nothing_on_pasted_text() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Pasted", "# Pasted\n");
    let before = state.active_document_id();

    let mut tx = Tx::default();
    let f5 = rune::KeyEvent { key: rune::Key::Function(5), mods: rune::Mods::default() };
    assert!(crate::app::on_key(&mut state, &f5, &mut tx), "the key is consumed");
    assert!(state.ui.notice.is_none(), "R-20: pasted text has no file and does nothing");
    assert!(state.prompt().is_none());
    assert_eq!(state.active_document_id(), before, "the same pasted tab stays in front");
    assert!(state.editor.compare_reloads.is_empty(), "no reload was queued for a file that does not exist");

    // The same in edit mode: the editor's own F5 path also does nothing.
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx));
    assert!(crate::app::on_key(&mut state, &f5, &mut tx), "the editor consumes F5");
    assert!(state.ui.notice.is_none());
    assert!(state.prompt().is_none());
    assert!(state.editor.compare_reloads.is_empty());
}

#[test]
fn a_deleted_file_keeps_the_buffer_and_save_again_recreates_it() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("gone.md");
    std::fs::write(&path, "before\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "edited ");
    std::fs::remove_file(&path).unwrap();
    assert!(state.apply_file_event(FileEvent::Removed { path: path.clone() }));

    assert!(matches!(state.prompt(), Some(Prompt::FileDeleted { .. })));
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.text().text(), "edited before\n", "the buffer survived the delete");
    assert!(matches!(*doc.status.borrow(), DocStatus::Gone));

    let mut tx = Tx::default();
    state.prompt_primary(&mut tx);
    poll_until(&mut state, "Save again", |_| path.exists());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "edited before\n");
    poll_until(&mut state, "the document to be ready", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| !doc.is_dirty())
    });
}

#[test]
fn closing_a_dirty_tab_asks_and_dont_save_discards_the_draft() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("close.md");
    std::fs::write(&path, "saved\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "unsaved ");
    let tab = state.active_tab().map(|tab| tab.id).expect("tab");

    state.request_close_tab(tab);
    assert!(matches!(state.prompt(), Some(Prompt::CloseTab { .. })), "the close guard asked first");
    assert!(state.tab(tab).is_some(), "the tab is still open");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved\n");

    let mut tx = Tx::default();
    state.prompt_secondary(&mut tx);
    assert!(state.tab(tab).is_none(), "Don't save closed the tab");
    assert_eq!(state.tabs().len(), 1, "a start page replaced the last document");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved\n", "nothing was written");
}

#[test]
fn a_window_close_names_every_dirty_document_and_cancel_keeps_it_open() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "one");
    state.open_synthetic("Two", "two");
    for tab in state.tabs().to_vec() {
        if let Some(doc) = tab.document() {
            doc.apply(0..3, "changed");
        }
    }
    for tab in state.tabs().to_vec() {
        if let Some(doc) = tab.document() {
            state.sync_tab_dirty(doc.as_ref());
        }
    }

    state.request_close_window();
    assert!(matches!(state.prompt(), Some(Prompt::CloseWindow)));
    assert_eq!(state.dirty_document_names().len(), 2, "both documents are named");
    state.prompt_cancel();
    assert!(state.prompt().is_none());
    assert!(!state.ui.close_requested);
    assert_eq!(state.tabs().len(), 2);
}

#[test]
fn a_dirty_document_another_window_still_shows_closes_this_view_without_a_question() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("shared.md");
    std::fs::write(&path, "base\n").unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);

    open_loaded(&mut first, &path);
    let doc = first.active_tab().and_then(Tab::document).cloned().expect("document");
    let second_tab = second.attach_document(doc.clone());
    enter_editor(&mut first);
    type_text(&mut first, "edited ");
    poll_once(&mut first);
    poll_once(&mut second);
    assert!(doc.is_dirty());

    first.request_close_window();
    assert!(first.prompt().is_none(), "a document another window owns asks nothing here");
    assert!(first.ui.close_requested, "the window closes silently");
    assert!(second.tab(second_tab).is_some(), "the other view stays");
    assert_eq!(doc.text().text(), "edited base\n", "the shared buffer survives");

    drop(first);
    assert!(doc.is_dirty(), "the other view's buffer is still unsaved");
    assert_eq!(doc.subscribers.borrow().len(), 1);
    assert_eq!(doc.lease_holder(), None, "the closed view released its lease");
}

#[cfg(unix)]
#[test]
fn a_deleted_file_behind_a_symlink_offers_its_canonical_draft() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("real");
    std::fs::create_dir(&folder).unwrap();
    let real = folder.join("note.md");
    std::fs::write(&real, "disk\n").unwrap();
    let alias = temp.path().join("alias.md");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let dirs = test_support::dirs(&temp);
    let canonical = std::fs::canonicalize(&real).unwrap();

    {
        let shared = crate::shared::AppShared::start(dirs.clone());
        let mut state = WindowState::new(shared.clone());
        // Opening the link makes the canonical path the document's key; the
        // draft is therefore stored under the canonical spelling.
        open_loaded(&mut state, &alias);
        let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
        assert_eq!(*doc.key.borrow(), lmdv_core::DocumentKey::File(canonical.clone()));
        enter_editor(&mut state);
        type_text(&mut state, "draft ");
        state.flush_active_draft();
        flush_store(&state);
    }
    assert!(
        lmdv_io::store::load_draft(&dirs, &canonical.to_string_lossy()).is_some(),
        "the draft lives under the canonical key"
    );

    // The target is deleted: the link stays but no longer resolves, and a new
    // open can only spell the lexical path. Resolving it needs I/O, so the
    // draft job does it (OI-171).
    std::fs::remove_file(&real).unwrap();
    assert!(std::fs::canonicalize(&alias).is_err(), "the link is broken");

    let shared = crate::shared::AppShared::start(dirs.clone());
    let mut state = WindowState::new(shared);
    state.open_path(alias.clone());
    poll_until(&mut state, "the broken link's draft offer", |state| {
        matches!(state.prompt(), Some(Prompt::Draft { .. }))
    });
    let offered = match state.prompt() {
        Some(Prompt::Draft { draft, .. }) => draft.text.clone(),
        _ => unreachable!(),
    };
    assert_eq!(offered, "draft disk\n", "the canonical draft was found through the lexical spelling");

    let mut tx = Tx::default();
    state.prompt_primary(&mut tx);
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.text().text(), "draft disk\n");
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));
    assert_eq!(
        lmdv_io::store::load_draft(&dirs, &canonical.to_string_lossy()),
        None,
        "the draft moved to the key this document will save and discard under"
    );
}

#[test]
fn closing_one_view_of_a_shared_dirty_document_keeps_its_draft() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("shared-draft.md");
    std::fs::write(&path, "base\n").unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);

    open_loaded(&mut first, &path);
    let doc = first.active_tab().and_then(Tab::document).cloned().expect("document");
    let second_tab = second.attach_document(doc.clone());
    enter_editor(&mut first);
    type_text(&mut first, "draft ");
    let tab = first.active_tab().map(|tab| tab.id).expect("tab");
    poll_once(&mut first);
    first.poll(1.1, &mut Tx::default());
    flush_store(&first);

    first.request_close_tab(tab);
    assert!(first.prompt().is_none(), "the other view keeps the buffer");
    assert!(first.tab(tab).is_none(), "only this view closed");
    assert!(second.tab(second_tab).is_some());
    assert!(doc.is_dirty());
    assert_eq!(doc.lease_holder(), None, "the closing view released the lease");
    let draft = lmdv_io::store::load_draft(&test_support::dirs(&temp), &path.to_string_lossy())
        .expect("the other view's draft survived");
    assert_eq!(draft.text, "draft base\n");
}

#[test]
fn closing_one_view_never_releases_another_windows_lease() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("lease.md");
    std::fs::write(&path, "base\n").unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);

    open_loaded(&mut first, &path);
    let doc = first.active_tab().and_then(Tab::document).cloned().expect("document");
    let second_tab = second.attach_document(doc.clone());
    enter_editor(&mut second);
    type_text(&mut second, "edited ");
    let lease = doc.lease_holder().expect("the second window edits");
    assert_eq!(lease.window, second.id());
    assert_eq!(lease.tab, second_tab);
    let first_tab = first.active_tab().map(|tab| tab.id).expect("tab");

    first.request_close_tab(first_tab);
    assert!(first.prompt().is_none());
    assert_eq!(doc.lease_holder(), Some(lease), "the other window's lease is untouched");
    assert_eq!(doc.subscribers.borrow().len(), 1);
}

#[test]
fn the_last_view_of_a_dirty_document_asks_and_cancel_keeps_everything() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("last.md");
    std::fs::write(&path, "base\n").unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);

    open_loaded(&mut first, &path);
    let doc = first.active_tab().and_then(Tab::document).cloned().expect("document");
    second.attach_document(doc.clone());
    enter_editor(&mut first);
    type_text(&mut first, "edited ");
    let tab = first.active_tab().map(|tab| tab.id).expect("tab");
    first.request_close_tab(tab);
    assert!(first.prompt().is_none(), "the shared view closed silently");
    poll_once(&mut second);

    second.request_close_window();
    assert!(matches!(second.prompt(), Some(Prompt::CloseWindow)), "the last view asks");
    assert!(second.shared().windows().get(second.id()).is_some_and(|entry| entry.closing));
    assert_eq!(second.shared().window_with_prompt(Some(second.id())), None);
    poll_once(&mut second);
    assert!(second.shared().windows().get(second.id()).is_some_and(|entry| entry.prompt_open));

    second.prompt_cancel();
    assert!(second.prompt().is_none());
    assert!(!second.ui.close_requested);
    assert_eq!(second.tabs().len(), 1, "cancel keeps the tab");
    poll_once(&mut second);
    assert!(!second.shared().windows().get(second.id()).is_some_and(|entry| entry.closing));
    assert!(doc.is_dirty());
}

#[test]
fn a_clean_pasted_document_still_asks_on_window_close() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Pasted", "text");
    state.request_close_window();

    assert!(matches!(state.prompt(), Some(Prompt::CloseWindow)));
    assert!(state.dirty_document_names().is_empty(), "nothing was edited");
    let question = state.close_window_question();
    assert!(question.contains("Pasted"), "{question}");
    assert!(question.to_ascii_lowercase().contains("pasted text"), "{question}");

    state.prompt_cancel();
    assert_eq!(state.tabs().len(), 1, "cancel keeps the pasted tab");
}

#[test]
fn dont_save_on_a_shared_pasted_document_keeps_the_other_views_buffer_and_draft() {
    let temp = TempDir::new().unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);
    let tab = first.open_synthetic("Pasted", "text");
    let doc = first.tab(tab).and_then(Tab::document).cloned().expect("document");
    let second_tab = second.attach_document(doc.clone());
    enter_editor(&mut first);
    type_text(&mut first, "edited ");
    poll_once(&mut first);
    first.poll(1.1, &mut Tx::default());
    flush_store(&first);
    let dirs = test_support::dirs(&temp);
    let key = crate::editor::draft_key(&doc).expect("pasted draft key");
    assert!(lmdv_io::store::load_draft(&dirs, &key).is_some(), "the draft exists before the close");
    poll_once(&mut second);

    first.request_close_window();
    assert!(matches!(first.prompt(), Some(Prompt::CloseWindow)), "pasted text always asks");
    let mut tx = Tx::default();
    first.prompt_secondary(&mut tx);

    assert!(first.ui.close_requested);
    assert!(doc.is_dirty(), "the other view's buffer is not marked clean");
    assert_eq!(doc.text().text(), "edited text");
    assert!(second.tab(second_tab).is_some(), "the other view stays open");
    assert_eq!(doc.lease_holder(), None, "this window's lease was released");
    flush_store(&first);
    assert!(lmdv_io::store::load_draft(&dirs, &key).is_some(), "the other view's draft was not removed");
}

#[test]
fn a_second_close_request_never_stacks_a_second_question() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("stack.md");
    std::fs::write(&path, "base\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "edited ");

    state.request_close_window();
    assert!(matches!(state.prompt(), Some(Prompt::CloseWindow)));
    state.request_close_window();
    assert!(matches!(state.prompt(), Some(Prompt::CloseWindow)), "the same question stays");
    assert!(!state.ui.close_requested, "no close was issued");
    state.request_quit();
    assert!(matches!(state.prompt(), Some(Prompt::CloseWindow)), "a quit does not replace it");
    assert!(!state.editor.quit.saving);

    state.prompt_cancel();
    assert!(state.prompt().is_none());
    assert!(!state.shared().windows().get(state.id()).is_some_and(|entry| entry.closing));
}

#[test]
fn aggregate_quit_names_every_window_document_once_and_saves_once() {
    let temp = TempDir::new().unwrap();
    let path_a = temp.path().join("a.md");
    let path_b = temp.path().join("b.md");
    std::fs::write(&path_a, "alpha\n").unwrap();
    std::fs::write(&path_b, "beta\n").unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);

    open_loaded(&mut first, &path_a);
    let doc_a = first.active_tab().and_then(Tab::document).cloned().expect("a");
    second.attach_document(doc_a.clone());
    open_loaded(&mut second, &path_b);
    enter_editor(&mut second);
    type_text(&mut second, "edited ");
    enter_editor(&mut first);
    type_text(&mut first, "shared ");
    poll_once(&mut first);
    poll_once(&mut second);
    assert!(doc_a.is_dirty());

    first.request_quit();
    assert!(matches!(first.prompt(), Some(Prompt::Quit)));
    assert_eq!(first.editor.quit.docs.len(), 2, "the shared document is one entry");
    assert_eq!(first.quit_document_names().len(), 2);
    let names = first.quit_document_names();
    assert!(names.iter().any(|name| name == "a.md"), "{names:?}");
    assert!(names.iter().any(|name| name == "b.md"), "{names:?}");
    assert!(first.shared().windows().get(first.id()).is_some_and(|entry| entry.closing));

    first.prompt_cancel();
    assert!(first.prompt().is_none(), "cancel aborts the quit");
    assert!(!first.shared().is_quitting());
    assert!(!first.shared().windows().get(first.id()).is_some_and(|entry| entry.closing));
    first.request_quit();
    assert!(matches!(first.prompt(), Some(Prompt::Quit)));

    let mut tx = Tx::default();
    first.prompt_primary(&mut tx);
    assert_eq!(first.editor.after_save.len(), 1, "one document saves at a time");
    poll_until(&mut first, "the aggregate quit", |state| state.shared().is_quitting());

    assert_eq!(std::fs::read_to_string(&path_a).unwrap(), "shared alpha\n");
    assert_eq!(std::fs::read_to_string(&path_b).unwrap(), "edited beta\n");
    assert!(!doc_a.is_dirty());
    assert!(first.quit_document_names().is_empty());
}

#[test]
fn a_quit_dont_save_does_not_let_another_window_rewrite_the_draft() {
    let temp = TempDir::new().unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);
    let tab = first.open_synthetic("Pasted", "text");
    let doc = first.tab(tab).and_then(Tab::document).cloned().expect("document");
    enter_editor(&mut first);
    type_text(&mut first, "edited ");
    poll_once(&mut first);
    first.poll(1.1, &mut Tx::default());
    flush_store(&first);
    let dirs = test_support::dirs(&temp);
    let key = crate::editor::draft_key(&doc).expect("pasted draft key");
    assert!(lmdv_io::store::load_draft(&dirs, &key).is_some());
    poll_once(&mut second);

    second.request_quit();
    assert!(matches!(second.prompt(), Some(Prompt::Quit)));
    assert_eq!(second.quit_document_names(), vec!["Pasted".to_owned()]);
    second.prompt_secondary(&mut Tx::default());
    assert!(second.shared().is_quitting());

    drop(first);
    drop(second);
    assert!(lmdv_io::store::load_draft(&dirs, &key).is_none(), "the editor's window did not rewrite a discarded draft");
}

#[test]
fn a_quit_while_another_window_asks_focuses_that_question_instead() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("busy.md");
    std::fs::write(&path, "base\n").unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);

    open_loaded(&mut first, &path);
    enter_editor(&mut first);
    type_text(&mut first, "edited ");
    first.request_close_window();
    assert!(matches!(first.prompt(), Some(Prompt::CloseWindow)));
    poll_once(&mut first);

    second.request_quit();
    assert!(second.prompt().is_none(), "no second question is opened");
    assert!(matches!(first.prompt(), Some(Prompt::CloseWindow)));
    assert!(second.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("Another window")));
    assert_eq!(second.shared().windows().focus_order().first(), Some(&first.id()));
    assert!(!second.shared().is_quitting());
}

#[test]
fn saving_before_closing_a_tab_then_closes_it() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("save-close.md");
    std::fs::write(&path, "base\n").unwrap();

    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "saved ");
    let tab = state.active_tab().map(|tab| tab.id).expect("tab");
    state.request_close_tab(tab);
    let mut tx = Tx::default();
    state.prompt_primary(&mut tx);
    poll_until(&mut state, "the tab to close after saving", |state| state.tab(tab).is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved base\n");
}

#[test]
fn the_editor_scene_has_source_and_preview_and_a_narrow_window_keeps_the_source_only() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_editor(&mut state);
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));

    let elements = render(state, (1100.0, 760.0));
    assert!(has_key(&elements, "editor-source"), "the source field is there");
    assert!(has_key(&elements, "editor-split"), "the split is drawn");
    assert!(has_key(&elements, "editor-divider"), "the divider is there");
    assert!(has_key(&elements, "article-scroll"), "the live preview is drawn");
    assert!(
        elements.iter().any(|element| element.role == "textbox" && element.key.as_deref() == Some("editor-source")),
        "the source field keeps its textbox role"
    );

    let mut state = window(&temp);
    crate::view::scene_editor(&mut state);
    assert!(state.editor.preview_visible, "the wide default is the split");
    let elements = render_twice(state, (700.0, 760.0));
    assert!(has_key(&elements, "editor-source"));
    assert!(!has_key(&elements, "article-scroll"), "below 760 pt the source is alone (E-02)");
    assert!(has_key(&elements, "editor-preview-open"), "and the preview has its toggle");

    let mut state = window(&temp);
    crate::view::scene_editor(&mut state);
    state.show_narrow_preview();
    let elements = render_twice(state, (700.0, 760.0));
    assert!(!has_key(&elements, "editor-source"), "the preview replaces the source");
    assert!(has_key(&elements, "article-scroll"));
    assert!(has_key(&elements, "editor-source-open"), "with a way back to the source");
}

#[test]
fn each_tab_keeps_its_editor_history_across_a_switch() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "one two");
    enter_editor(&mut state);
    type_text(&mut state, "edited ");
    let second = state.open_synthetic("Second", "other");
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(second));
    assert_eq!(state.tab(second).map(|tab| tab.view.mode), Some(Mode::Viewer));

    assert!(state.activate_tab(first));
    assert_eq!(state.tab(first).map(|tab| tab.view.mode), Some(Mode::Editor), "the tab returns in edit mode");
    assert_eq!(
        state.tab(first).and_then(Tab::document).map(|doc| doc.text().text().to_owned()).as_deref(),
        Some("edited one two")
    );
    let undone =
        state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).is_some_and(lmdv_edit::EditorSession::undo);
    assert!(undone);
    state.editor_changed();
    assert_eq!(
        state.tab(first).and_then(Tab::document).map(|doc| doc.text().text().to_owned()).as_deref(),
        Some("one two"),
        "undo in the tab's own history reaches the document (E-40)"
    );
}

fn render(state: WindowState, size: (f32, f32)) -> Vec<ElementInfo> {
    let mut runtime = rune::Runtime::new(state, view, size, rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.elements()
}

fn render_twice(state: WindowState, size: (f32, f32)) -> Vec<ElementInfo> {
    let mut runtime = rune::Runtime::new(state, view, size, rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    runtime.elements()
}

fn has_key(elements: &[ElementInfo], key: &str) -> bool {
    elements.iter().any(|element| element.key.as_deref() == Some(key))
}

#[test]
fn typing_parses_the_preview_after_the_debounce_and_keeps_the_last_good_model_meanwhile() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Preview", "# First\n");
    enter_editor(&mut state);
    let before = state.active_tab().and_then(|tab| tab.view.reader_revision).expect("first model");
    type_text(&mut state, "New ");
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert_eq!(crate::editor::PREVIEW_DEBOUNCE, 0.2, "E-05's 200 ms");
    assert_eq!(
        state.active_tab().and_then(|tab| tab.view.reader_revision),
        Some(before),
        "the debounce holds the old model"
    );
    assert!(state.active_tab().is_some_and(|tab| tab.view.preview_pending));
    state.poll(0.19, &mut tx);
    assert_eq!(
        state.active_tab().and_then(|tab| tab.view.reader_revision),
        Some(before),
        "no parse during the 200 ms debounce"
    );

    state.poll(0.21, &mut tx);
    poll_until(&mut state, "the preview parse", |state| {
        state.active_tab().and_then(|tab| tab.view.reader_revision) != Some(before)
    });
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("new model");
    assert!(model.source.starts_with("New "), "the preview parsed the edited source");
}

#[test]
fn an_invalid_mermaid_block_is_a_local_fallback_and_never_a_parse_error() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Diagram", "# First\n");
    enter_editor(&mut state);
    let before = state.active_tab().and_then(|tab| tab.view.reader_revision).expect("first model");
    type_text(&mut state, "```mermaid\nnot a diagram ->\n```\n\n");

    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    state.poll(0.21, &mut tx);
    poll_until(&mut state, "the diagram parse", |state| {
        state.active_tab().and_then(|tab| tab.view.reader_revision) != Some(before)
    });
    let tab = state.active_tab().expect("tab");
    let model = tab.view.reader.as_ref().expect("preview model");
    assert!(tab.view.reader_error.is_none(), "an invalid diagram is an M-14 block fallback, not a failed parse");
    assert!(
        model.blocks.iter().any(|block| matches!(block, lmdv_md::Block::Mermaid { .. })),
        "the fence is still a diagram block whose source the renderer can fall back to"
    );
    assert!(state.ui.notice.is_none(), "no preview-error notice for a block that renders its own fallback");
}

#[test]
fn a_failed_preview_parse_keeps_the_last_good_model_and_shows_a_dismissible_notice() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Preview error", "# First\n");
    enter_editor(&mut state);
    let good = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("last good model");
    type_text(&mut state, "New ");

    let mut tx = Tx::default();
    state.poll(0.0, &mut tx); // arms the 200 ms debounce, so no parse runs yet
    // What `adopt_parsed(Err(..))` leaves: the last good model is untouched and
    // the revision's failure is recorded for the preview.
    if let Some(tab) = state.active_tab_mut() {
        tab.view.reader_error = Some("the Markdown parser failed: boom".to_owned());
    }
    state.poll(0.0, &mut tx);
    let tab = state.active_tab().expect("tab");
    assert_eq!(
        tab.view.reader.as_ref().map(|model| model.revision),
        Some(good.revision),
        "the preview keeps the last good model"
    );
    let notice = state.ui.notice.as_ref().expect("E-05's notice");
    assert!(notice.text.contains("Preview error"), "{}", notice.text);
    assert!(notice.text.contains("boom"), "the parser's reason is shown: {}", notice.text);
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));

    state.dismiss_notice();
    assert!(state.ui.notice.is_none(), "the notice is dismissible");
    state.poll(0.0, &mut tx);
    assert!(state.ui.notice.is_none(), "one failure is announced once, even after dismissal");
}

#[test]
fn the_dirty_dot_and_title_clear_after_save() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("dot.md");
    std::fs::write(&path, "text\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "x");
    assert!(state.window_title().contains('•'));
    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    wait_for_save(&mut state);
    assert!(!state.window_title().contains('•'));
    assert_eq!(state.window_title(), "dot.md — lmdv");
}

#[test]
fn entering_edit_mode_puts_the_caret_at_the_section_being_read() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Sections", "# Top\n\nText\n\n## Second\n\nMore\n");
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("model");
    let line = model.headings[1].line;
    state.reading.viewport_anchor.set(Some(crate::reader::ViewportAnchor {
        heading: Some(1),
        offset: 40.0,
        top: false,
    }));
    enter_editor(&mut state);
    let expected =
        state.active_tab().and_then(Tab::document).map(|doc| doc.text().line_start(line)).expect("line start");
    let caret = state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|editor| editor.selection().start);
    assert_eq!(caret, Some(expected), "E-03's basic caret placement");
}

#[test]
fn the_one_second_draft_timer_writes_while_dirty() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("timer-draft.md");
    std::fs::write(&path, "disk\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "draft ");

    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert!(state.active_tab().is_some_and(|tab| tab.view.draft_pending));
    state.poll(1.1, &mut tx);
    assert!(!state.active_tab().is_some_and(|tab| tab.view.draft_pending), "the draft timer fired");
    flush_store(&state);
    let key = path.to_string_lossy().into_owned();
    let draft = lmdv_io::store::load_draft(&test_support::dirs(&temp), &key).expect("the draft exists");
    assert_eq!(draft.text, "draft disk\n");
    assert_eq!(draft.base, "disk\n", "the base is the file text edit mode started from");
}

#[test]
fn an_own_save_echo_is_suppressed_for_the_watcher() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("echo.md");
    std::fs::write(&path, "base\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "saved ");
    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    wait_for_save(&mut state);

    std::fs::write(&path, "external\n").unwrap();
    let _ = state.apply_file_event(FileEvent::Changed { path: path.clone() });
    assert!(state.prompt().is_none(), "the own-write window swallowed the echo");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.text().text(), "saved base\n", "the buffer was not reloaded");
}

#[test]
fn save_all_from_the_window_prompt_saves_then_closes() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("all.md");
    std::fs::write(&path, "base\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "all ");
    state.request_close_window();
    assert!(matches!(state.prompt(), Some(Prompt::CloseWindow)));

    let mut tx = Tx::default();
    state.prompt_primary(&mut tx);
    poll_until(&mut state, "Save all to close", |state| state.prompt().is_none() && !state.has_unsaved_documents());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "all base\n");
    assert!(!state.ui.close_requested, "poll issued the close it had been asked for");
}

#[test]
fn keeping_the_disk_copy_after_a_close_conflict_reopens_the_window_for_routing() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("close-conflict.md");
    std::fs::write(&path, "base\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "mine ");

    std::fs::write(&path, "theirs\n").unwrap();
    state.request_close_window();
    assert!(matches!(state.prompt(), Some(Prompt::CloseWindow)));
    let mut tx = Tx::default();
    state.prompt_primary(&mut tx);
    poll_until(&mut state, "the close conflict", |state| matches!(state.prompt(), Some(Prompt::SaveConflict { .. })));
    assert!(state.shared().windows().get(state.id()).is_some_and(|entry| entry.closing));

    state.prompt_secondary(&mut tx);
    assert!(state.prompt().is_none());
    assert!(!state.editor.close_on_clean);
    assert!(!state.shared().windows().get(state.id()).is_some_and(|entry| entry.closing), "T-25 restores routing");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n");
}

#[test]
fn save_all_waits_for_a_pasted_save_as_before_closing() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("mixed.md");
    std::fs::write(&file, "base\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &file);
    enter_editor(&mut state);
    type_text(&mut state, "dirty ");
    let pasted = state.open_synthetic("Pasted", "text");
    let pasted_doc = state.tab(pasted).and_then(Tab::document).cloned().expect("pasted");

    state.request_close_window();
    assert!(matches!(state.prompt(), Some(Prompt::CloseWindow)));
    let question = state.close_window_question();
    assert!(question.contains("mixed.md"), "{question}");
    assert!(question.contains("Pasted"), "{question}");
    let mut tx = Tx::default();
    state.prompt_primary(&mut tx);

    // The file's save lands while the pasted text is still choosing a path:
    // the window must not close before that save completes.
    poll_until(&mut state, "the file save", |state| {
        state
            .tabs()
            .iter()
            .filter_map(Tab::document)
            .find(|doc| doc.key.borrow().path() == Some(file.as_path()))
            .is_some_and(|doc| !doc.is_dirty())
    });
    assert!(!state.ui.close_requested, "the pasted Save As is still pending");
    assert!(state.editor.close_pending.contains(&pasted_doc.id));

    let target = temp.path().join("pasted.md");
    state.start_save(pasted_doc.id, target.clone(), None, true, crate::editor::AfterSave::CloseWindow);
    poll_until(&mut state, "the pasted save and close", |state| {
        !state.editor.close_on_clean && state.editor.close_pending.is_empty()
    });
    assert!(!state.ui.close_requested, "poll issued the close it was waiting for");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "text");
}

#[test]
fn edit_mode_hides_the_reading_panes_and_restores_them() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("Panes", "# Panes\n\ntext\n");
    state.left.set_open(true);
    state.right.set_open(true);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = false;
    }
    enter_editor(&mut state);
    assert!(!state.left.is_open() && !state.right.is_open(), "E-02 hides the reading panes");
    let mut tx = Tx::default();
    state.toggle_editor_mode(&mut tx);
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Viewer));
    assert!(state.left.is_open() && state.right.is_open(), "reading restores them");

    // Switching to another tab while editing restores the reading panes for
    // that tab, and coming back hides them again.
    enter_editor(&mut state);
    let second = state.open_synthetic("Other", "# Other\n");
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(second));
    assert!(state.left.is_open() && state.right.is_open(), "a viewer tab restores the panes");
    assert!(state.right.is_open(), "the viewer's own outline choice applies");
    assert!(state.activate_tab(first));
    assert!(!state.left.is_open() && !state.right.is_open(), "the editor hides them again");
}

#[test]
fn leaving_edit_mode_can_save_first_and_returns_to_the_viewer() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("leave-save.md");
    std::fs::write(&path, "base\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "saved ");

    let mut tx = Tx::default();
    state.toggle_editor_mode(&mut tx);
    assert!(matches!(state.prompt(), Some(Prompt::DiscardEdit { .. })));
    state.prompt_primary(&mut tx);
    poll_until(&mut state, "the save-and-leave", |state| {
        state.active_tab().map(|tab| tab.view.mode) == Some(Mode::Viewer)
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved base\n");
    assert!(!state.active_tab().is_some_and(|tab| tab.view.dirty));
}

#[test]
fn leaving_edit_mode_discards_on_request_and_cancel_keeps_editing() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("leave-discard.md");
    std::fs::write(&path, "base\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "draft ");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    let key = crate::editor::draft_key(&doc).expect("a file draft key");

    let mut tx = Tx::default();
    state.toggle_editor_mode(&mut tx);
    assert!(matches!(state.prompt(), Some(Prompt::DiscardEdit { .. })));

    // Cancel changes nothing: the mode, the buffer and its crash copy stay.
    state.prompt_cancel();
    assert!(state.prompt().is_none());
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));
    assert_eq!(
        state.active_tab().and_then(Tab::document).map(|doc| doc.text().text().to_owned()).as_deref(),
        Some("draft base\n")
    );
    assert!(state.active_tab().is_some_and(|tab| tab.view.dirty));
    flush_store(&state);
    assert!(lmdv_io::store::load_draft(&test_support::dirs(&temp), &key).is_some(), "Cancel keeps the draft");

    // Discard reloads the saved text, leaves edit mode and removes the draft.
    state.toggle_editor_mode(&mut tx);
    assert!(matches!(state.prompt(), Some(Prompt::DiscardEdit { .. })));
    state.prompt_discard_edit(&mut tx);
    assert!(state.prompt().is_none());
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Viewer));
    let discarded = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(discarded.text().text(), "base\n", "the saved version is back in the buffer");
    assert!(!discarded.is_dirty() && !state.active_tab().is_some_and(|tab| tab.view.dirty));
    flush_store(&state);
    assert!(lmdv_io::store::load_draft(&test_support::dirs(&temp), &key).is_none(), "the draft was removed");

    // The viewer's preview is the saved version, not the discarded text.
    poll_until(&mut state, "the discarded preview", |state| {
        state.active_tab().is_some_and(|tab| tab.view.reader_revision == tab.document().map(|doc| doc.revision()))
    });
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("preview model");
    assert_eq!(&*model.source, "base\n");
}

#[test]
fn the_saved_check_expires_after_its_seconds() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("flash.md");
    std::fs::write(&path, "base\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "x");
    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    wait_for_save(&mut state);
    assert!(state.saved_flash());
    state.poll(0.1, &mut tx);
    assert!(state.saved_flash());
    state.poll(1.6, &mut tx);
    assert!(!state.saved_flash(), "the saved check faded");
}

#[test]
fn pasted_documents_draft_under_a_pasted_key() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Pasted note", "# Pasted\n");
    enter_editor(&mut state);
    type_text(&mut state, "typed ");
    state.flush_active_draft();
    flush_store(&state);
    let drafts = lmdv_io::store::load_drafts(&test_support::dirs(&temp));
    assert!(
        drafts.iter().any(|(key, draft)| key.starts_with("pasted:") && draft.text == "typed # Pasted\n"),
        "the pasted draft is keyed pasted:<id>: {drafts:?}"
    );
}

#[test]
fn change_range_is_a_minimal_character_safe_delta() {
    use crate::editor::change_range;
    assert_eq!(change_range("abc", "abc"), (3, 3, 3));
    assert_eq!(change_range("abc", "abXc"), (2, 2, 3));
    assert_eq!(change_range("abc", "ac"), (1, 2, 1));
    assert_eq!(change_range("", "x"), (0, 0, 1));
    assert_eq!(change_range("héllo", "hallo"), (1, 3, 2));
    let (start, old_end, new_end) = change_range("a👨‍👩‍👧b", "aXb");
    assert!(start == 1 && old_end == 19 && new_end == 2, "{start} {old_end} {new_end}");
}

#[test]
fn save_as_renames_an_untitled_document_into_a_file() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Budget", "# Budget\n");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    let path = temp.path().join("Budget.md");
    state.start_save(doc.id, path.clone(), None, true, crate::editor::AfterSave::Dismiss);
    wait_for_save(&mut state);

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Budget\n");
    assert_eq!(*doc.key.borrow(), lmdv_core::DocumentKey::File(path.clone()));
    assert!(state.shared().docs().find_path(&path).is_some());
    assert_eq!(state.active_tab().map(|tab| tab.full_title()).as_deref(), Some("Budget.md"));
    assert_eq!(state.active_tab().map(|tab| tab.title()).as_deref(), Some("Budget"));
}

#[test]
fn an_untitled_document_asks_for_a_path_on_ctrl_s() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Pasted note", "text\n");
    let mut tx = Tx::default();
    state.request_save_active(&mut tx);
    assert!(state.editor.after_save.is_empty(), "no job started without a path");
    assert_eq!(state.active_tab().and_then(Tab::document).map(|doc| doc.key.borrow().path().is_some()), Some(false));
}

// ------------------------------------------------- E-11/E-12 Save as (OI-071)

/// Waits for a Save as to finish: the path probe, then the write.
fn wait_for_save_as(state: &mut WindowState) {
    poll_until(state, "the save-as probe and its save", |state| {
        state.editor.save_as.pending.is_empty() && state.editor.after_save.is_empty() && state.worker().is_empty()
    });
}

#[test]
fn the_save_as_name_is_file_safe_and_ends_in_md() {
    use crate::editor::suggested_file_name;
    assert_eq!(suggested_file_name("Budget"), "Budget.md");
    assert_eq!(suggested_file_name("notes.md"), "notes.md");
    assert_eq!(suggested_file_name("A/B: C?"), "A-B- C-.md", "path separators and reserved characters become dashes");
    assert_eq!(suggested_file_name("  ...  "), "Untitled.md");
    assert_eq!(suggested_file_name(""), "Untitled.md");
}

#[test]
fn save_as_starts_in_the_pane_root_and_falls_back_to_documents() {
    use crate::editor::save_as_folder_for;
    let note = Path::new("/notes/note.md");
    let pane = Path::new("/pane");
    let documents = Path::new("/home/user/Documents");
    assert_eq!(
        save_as_folder_for(Some(note), Some(pane), Some(documents)),
        Some(PathBuf::from("/notes")),
        "a file saves beside itself"
    );
    assert_eq!(
        save_as_folder_for(None, Some(pane), Some(documents)),
        Some(PathBuf::from("/pane")),
        "pasted text starts in the pane root, the paste home"
    );
    assert_eq!(
        save_as_folder_for(None, None, Some(documents)),
        Some(PathBuf::from("/home/user/Documents")),
        "with no pane root it falls back to Documents"
    );
    assert_eq!(save_as_folder_for(None, None, None), None);

    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.set_browser_root(temp.path().to_path_buf());
    state.open_synthetic("Budget", "# Budget\n");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(state.save_as_folder(&doc), Some(temp.path().to_path_buf()), "the pasted tab uses the pane's folder");

    // A second, independent state: the first one persisted its pane root.
    let other = TempDir::new().unwrap();
    let mut plain = window(&other);
    plain.open_synthetic("Budget", "# Budget\n");
    let doc = plain.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(
        plain.save_as_folder(&doc),
        lmdv_io::documents_dir(),
        "without a pane root the pasted tab starts at the platform Documents folder"
    );
}

#[test]
fn saving_pasted_text_makes_the_tab_that_file_and_moves_its_places() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Budget", "# Budget\n\nTotal: 3\n");
    let doc = state.tab(tab).and_then(Tab::document).cloned().expect("document");
    let pasted_key = {
        let key = doc.key.borrow();
        let lmdv_core::DocumentKey::Untitled(paste) = &*key else { panic!("pasted text") };
        format!("pasted:{}", paste.0)
    };
    // A place the tab left before it was saved, keyed as pasted text.
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.nav.record(lmdv_core::Place::new(pasted_key.clone(), ScrollAnchor::default()));
    }

    let chosen = temp.path().join("Budget.md");
    state.save_as_chosen(doc.id, chosen.clone(), crate::editor::AfterSave::Dismiss);
    wait_for_save_as(&mut state);

    let written = std::fs::canonicalize(&chosen).unwrap();
    let written_key = written.to_string_lossy().into_owned();
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "# Budget\n\nTotal: 3\n");
    assert_eq!(*doc.key.borrow(), lmdv_core::DocumentKey::File(written.clone()), "the tab is that file now");
    assert_eq!(doc.saved.borrow().path.as_deref(), Some(written.as_path()));
    assert!(!doc.is_dirty(), "the document matches what was written");
    assert_eq!(state.shared().docs().find_path(&written).map(|found| found.id), Some(doc.id));
    let tab = state.tab(tab).expect("the tab stayed");
    assert_eq!(tab.full_title(), "Budget.md");
    assert!(state.shared().recents().iter().any(|recent| recent.path == written), "the new file is in recents");
    assert_eq!(
        tab.view.nav.back.first().map(|place| place.path.as_str()),
        Some(written_key.as_str()),
        "the Back place follows the file"
    );
    assert!(!state.window_title().contains('•'), "the dirty dot is gone");
    flush_store(&state);
    let drafts = lmdv_io::store::load_drafts(&test_support::dirs(&temp));
    assert!(
        !drafts.iter().any(|(key, _)| key.starts_with("pasted:") || key == &written_key),
        "the pasted and file drafts are gone: {drafts:?}"
    );
}

#[test]
fn save_as_refuses_a_file_open_with_unsaved_changes() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("taken.md");
    std::fs::write(&path, "theirs\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "dirty ");
    let other = state.open_synthetic("Copy", "mine\n");
    let pasted = state.tab(other).and_then(Tab::document).cloned().expect("document");

    state.save_as_chosen(pasted.id, path.clone(), crate::editor::AfterSave::Dismiss);
    wait_for_save_as(&mut state);

    let notice = state.ui.notice.as_ref().map(|notice| notice.text.clone()).unwrap_or_default();
    assert!(notice.contains("taken.md"), "the warning names the file: {notice}");
    assert!(notice.contains("unsaved changes"), "the warning says why: {notice}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n", "nothing was written");
    assert!(matches!(*pasted.key.borrow(), lmdv_core::DocumentKey::Untitled(_)), "the pasted document stays unsaved");
    assert_eq!(state.tabs.iter().filter(|tab| tab.document_id().is_some()).count(), 2, "both documents are still open");
}

#[test]
fn a_refused_save_as_cancels_the_window_close_it_was_part_of() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("taken.md");
    std::fs::write(&path, "theirs\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    type_text(&mut state, "dirty ");
    state.open_synthetic("Copy", "mine\n");
    let pasted = state.active_tab().and_then(Tab::document).cloned().expect("document");

    // As the close plan leaves it: this window is saving every dirty document.
    state.editor.close_on_clean = true;
    state.editor.close_pending.insert(pasted.id);
    state.save_as_chosen(pasted.id, path.clone(), crate::editor::AfterSave::CloseWindow);
    wait_for_save_as(&mut state);

    assert!(!state.editor.close_on_clean, "the refused Save as cancels the close");
    assert!(state.editor.close_pending.is_empty(), "and forgets its pending documents");
    assert!(!state.ui.close_requested, "the window stays open");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n", "nothing was written");
}

#[test]
fn save_as_merges_with_a_clean_file_open_in_this_window() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("merged.md");
    std::fs::write(&path, "old text\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    let existing = state.active_tab().and_then(Tab::document).map(|doc| doc.id).expect("document");
    let tab = state.open_synthetic("Budget", "# Budget\n");
    let pasted = state.tab(tab).and_then(Tab::document).cloned().expect("document");

    state.save_as_chosen(pasted.id, path.clone(), crate::editor::AfterSave::Dismiss);
    wait_for_save_as(&mut state);

    let written = std::fs::canonicalize(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "# Budget\n", "the pasted text took the file over");
    assert_eq!(*pasted.key.borrow(), lmdv_core::DocumentKey::File(written.clone()));
    assert_eq!(
        state.tabs.iter().filter(|tab| tab.document_id().is_some()).count(),
        1,
        "one document for the file remains"
    );
    assert!(state.tab(tab).is_some(), "the pasted tab is the survivor");
    assert!(state.shared().docs().get(existing).is_none(), "the released duplicate left the registry");
    assert!(state.closed_tabs().is_empty(), "a merged view is not a reopen place");
    assert_eq!(state.shared().watch_refcount(&written), 1, "the file keeps one watch, not two");
}

#[cfg(unix)]
#[test]
fn save_as_finds_a_duplicate_through_a_symlink() {
    let temp = TempDir::new().unwrap();
    let real = temp.path().join("real.md");
    std::fs::write(&real, "old text\n").unwrap();
    let alias = temp.path().join("alias.md");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &real);
    let existing = state.active_tab().and_then(Tab::document).map(|doc| doc.id).expect("document");
    state.open_synthetic("Budget", "# Budget\n");
    let pasted = state.active_tab().and_then(Tab::document).cloned().expect("document");

    state.save_as_chosen(pasted.id, alias.clone(), crate::editor::AfterSave::Dismiss);
    wait_for_save_as(&mut state);

    assert_eq!(std::fs::read_to_string(&real).unwrap(), "# Budget\n", "the link points at the file written");
    assert!(std::fs::symlink_metadata(&alias).is_ok_and(|meta| meta.file_type().is_symlink()), "the alias stays");
    assert!(state.shared().docs().get(existing).is_none(), "the identity found the duplicate");
    assert_eq!(state.tabs.iter().filter(|tab| tab.document_id().is_some()).count(), 1);
}

#[test]
fn save_as_releases_a_clean_file_open_in_another_window() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("shared.md");
    std::fs::write(&path, "theirs\n").unwrap();
    let (shared, mut first, mut second) = two_windows(&temp);
    open_loaded(&mut first, &path);
    let existing = first.active_document_id().expect("the first window shows the file");
    poll_once(&mut second);
    second.open_synthetic("Copy", "mine\n");
    let pasted = second.active_tab().and_then(Tab::document).cloned().expect("document");

    second.save_as_chosen(pasted.id, path.clone(), crate::editor::AfterSave::Dismiss);
    wait_for_save_as(&mut second);

    let written = std::fs::canonicalize(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "mine\n", "the save took the file over");
    assert_eq!(*pasted.key.borrow(), lmdv_core::DocumentKey::File(written.clone()));
    assert!(second.ui.notice.as_ref().is_none_or(|notice| !notice.text.contains("another window")));

    // E-12: the other window is asked to close its clean view; it leaves on
    // its next poll, and the file keeps one document.
    poll_once(&mut first);
    assert_eq!(first.tabs().len(), 1);
    assert!(first.active_tab().is_some_and(Tab::is_start), "the released view closed");
    assert!(shared.docs().get(existing).is_none(), "the released document left the registry");
    assert_eq!(shared.docs().len(), 1, "one document for the file");
    assert_eq!(shared.docs().find_path(&written).map(|doc| doc.id), Some(pasted.id));
}

#[cfg(unix)]
#[test]
fn the_other_windows_editor_follows_an_adopted_disk_read() {
    let temp = TempDir::new().unwrap();
    let real = temp.path().join("real.md");
    std::fs::write(&real, "old\n").unwrap();
    let alias = temp.path().join("alias.md");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);
    open_loaded(&mut first, &real);
    let doc = first.active_tab().and_then(Tab::document).cloned().expect("document");
    enter_editor(&mut first);
    let second_tab = second.attach_document(doc.clone());
    poll_once(&mut second);

    // Another editor replaces the file while the first window holds a clean
    // edit session; the alias load is the fresher read (OI-172).
    let staged = temp.path().join("staged.md");
    std::fs::write(&staged, "new text\n").unwrap();
    std::fs::rename(&staged, &real).unwrap();
    second.open_path(alias);
    poll_until(&mut second, "the second window's merge", |state| state.active_document_id() == Some(doc.id));

    // The merge happened in the second window; the first learns from the
    // broadcast and its editor field follows the adopted text.
    poll_until(&mut first, "the first window's refresh", |state| {
        state.tabs.iter().any(|tab| tab.view.editor.as_ref().is_some_and(|editor| editor.text() == "new text\n"))
    });
    let editor_tab = first.active_tab().expect("the editor tab");
    assert_eq!(editor_tab.document_id(), Some(doc.id));
    assert!(!editor_tab.view.dirty, "the adopted text is the saved base");
    assert_eq!(doc.text().text(), "new text\n");
    assert_eq!(second.active_document_id(), Some(doc.id));
    assert!(second.tab(second_tab).is_some(), "the second window keeps its view");
}

#[test]
fn save_as_moves_the_other_windows_places_and_titles() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("shared.md");
    std::fs::write(&path, "text\n").unwrap();
    let (_shared, mut first, mut second) = two_windows(&temp);
    open_loaded(&mut first, &path);
    let doc = first.active_tab().and_then(Tab::document).cloned().expect("document");
    let second_tab = second.attach_document(doc.clone());
    let old_key = doc.key.borrow().path().map(|path| path.to_string_lossy().into_owned()).expect("a file key");
    if let Some(tab) = second.tabs.iter_mut().find(|tab| tab.id == second_tab) {
        tab.view.nav.record(lmdv_core::Place::new(old_key.clone(), ScrollAnchor::default()));
    }
    poll_once(&mut first);
    poll_once(&mut second);

    let chosen = temp.path().join("renamed.md");
    first.save_as_chosen(doc.id, chosen.clone(), crate::editor::AfterSave::Dismiss);
    wait_for_save_as(&mut first);
    let written = std::fs::canonicalize(&chosen).unwrap();
    let written_key = written.to_string_lossy().into_owned();
    assert_eq!(*doc.key.borrow(), lmdv_core::DocumentKey::File(written.clone()));

    // OI-173: the saving window already renamed its own places; the other
    // window follows the broadcast on its next poll.
    poll_once(&mut second);
    let tab = second.tab(second_tab).expect("the second view stays");
    assert_eq!(
        tab.view.nav.back.first().map(|place| place.path.as_str()),
        Some(written_key.as_str()),
        "the Back place follows the save-as rename"
    );
    assert_eq!(tab.full_title(), "renamed.md", "the strip title follows too");
    assert_eq!(tab.tooltip(), written.to_string_lossy(), "and the tooltip names the new file");
}

#[test]
fn save_as_warns_when_the_other_window_cannot_be_asked() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("unreachable.md");
    std::fs::write(&path, "theirs\n").unwrap();
    let (shared, mut first, mut second) = two_windows(&temp);
    open_loaded(&mut first, &path);
    poll_once(&mut second);
    // The window is no longer registered (it is closing), but its tab still
    // holds the document: no release can be delivered, so the warning stays.
    shared.unregister_window(first.id());
    second.open_synthetic("Copy", "mine\n");
    let pasted = second.active_tab().and_then(Tab::document).cloned().expect("document");

    second.save_as_chosen(pasted.id, path.clone(), crate::editor::AfterSave::Dismiss);
    wait_for_save_as(&mut second);

    let notice = second.ui.notice.as_ref().map(|notice| notice.text.clone()).unwrap_or_default();
    assert!(notice.contains("unreachable.md"), "the warning names the file: {notice}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n", "nothing was written");
    assert!(matches!(*pasted.key.borrow(), lmdv_core::DocumentKey::Untitled(_)));
    assert_eq!(first.tabs().iter().filter(|tab| tab.document_id().is_some()).count(), 1, "the other window keeps it");
}

#[test]
fn a_foreign_edit_lease_is_a_notice_not_a_takeover() {
    let temp = TempDir::new().unwrap();
    let shared = test_support::shared(&temp);
    let mut first = WindowState::new(shared.clone());
    let path: PathBuf = temp.path().join("lease.md");
    std::fs::write(&path, "text\n").unwrap();
    open_loaded(&mut first, &path);
    enter_editor(&mut first);

    let mut second = WindowState::new(shared);
    open_loaded(&mut second, &path);
    let mut tx = Tx::default();
    assert!(!second.enter_editor(None, &mut tx));
    assert!(matches!(second.prompt(), Some(Prompt::LeaseHeld { .. })));
    assert_eq!(second.active_tab().map(|tab| tab.view.mode), Some(Mode::Viewer));

    // The first window still owns the lease.
    let doc = first.active_tab().and_then(Tab::document).cloned().expect("document");
    assert_eq!(doc.lease_holder().map(|lease| lease.window), Some(first.id()));
}

// ------------------------------------------------- E-07 scroll sync (P3-04c2)

use crate::editor::scroll_sync::{self, EditorTop, Placement, PreviewBlock, PreviewGeometry, PreviewTable};
use crate::reader::ScrollSnapshot;

/// A document long enough for both source panes to scroll, under the
/// single-field threshold (`Auto` keeps one field).
fn scrolling_markdown(paragraphs: usize) -> String {
    let mut text = String::from("# A long note\n\n");
    for index in 0..paragraphs {
        text.push_str(&format!(
            "Paragraph {index} carries enough prose to fill a line or two in the preview column.\n\n"
        ));
    }
    text
}

fn preview_block(index: usize, key: &str, top: f32, height: f32, next_top: f32) -> PreviewBlock {
    PreviewBlock { index, key: key.to_owned(), top, height, next_top, heading: None, table: None }
}

fn geometry(blocks: Vec<PreviewBlock>, viewport: f32, max: f32) -> PreviewGeometry {
    PreviewGeometry { blocks, content_bottom: 1200.0, viewport, max }
}

fn runtime(state: WindowState) -> rune::Runtime<WindowState> {
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime
}

fn article_offset(runtime: &rune::Runtime<WindowState>) -> f32 {
    runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(reader::ARTICLE_KEY))
        .map_or(0.0, |article| article.scrolled[1])
}

#[test]
fn a_scrolled_field_maps_between_two_preview_blocks() {
    let doc = lmdv_md::parse("# A\n\nfirst paragraph\n\n## B\n\nsecond paragraph\n");
    let line_height = crate::edit_ui::EditorMetrics::for_reader(16).line_height;
    let top = EditorTop { measured: true, start: 16.0, ..EditorTop::default() };
    let offset = 16.0 + line_height * 1.75;
    let line = scroll_sync::top_line(&top, offset, (0, doc.line_count()), doc.line_count(), line_height);
    assert!((line - 1.75).abs() < 1e-4, "the offset is 1.75 lines down, got {line}");

    let blocks = vec![preview_block(0, "b0", 40.0, 60.0, 140.0), preview_block(1, "b1", 140.0, 100.0, 300.0)];
    let geometry = geometry(blocks, 700.0, 2000.0);
    let placement = scroll_sync::preview_placement(&doc, line, false, &geometry).expect("a block holds the line");
    let span = &doc.block_spans[0];
    // The anchor spans to the next block's first line, as the original's
    // `anchorSpan` does, so the blank line between blocks interpolates.
    let next = doc.block_spans[1].line;
    let t = ((line - span.line as f32) / (next - span.line) as f32).clamp(0.0, 1.0);
    // The viewport top is `y_line - SYNC_MARGIN`; `y` is where the block's top
    // sits relative to it, so a line inside the block puts the top above.
    let expected_y = 16.0 - t * 100.0;
    assert_eq!(placement.key, "b0", "the block that holds line 1.75");
    assert!((placement.y - expected_y).abs() < 1e-3, "y {} expected {expected_y}", placement.y);
    assert!(placement.y < 0.0, "the block's top is scrolled above the viewport");
}

#[test]
fn a_block_row_maps_its_measured_height_onto_its_source_lines() {
    let top = EditorTop { measured: true, block: Some(3), block_top: 64.0, block_height: 80.0, ..EditorTop::default() };
    let line_height = crate::edit_ui::EditorMetrics::for_reader(16).line_height;
    let line = scroll_sync::top_line(&top, 64.0 + 40.0, (10, 14), 30, line_height);
    assert!((line - 12.0).abs() < 1e-4, "halfway down a four-line block, got {line}");
    let line = scroll_sync::top_line(&top, 64.0 - 20.0, (10, 14), 30, line_height);
    assert!((line - 10.0).abs() < 1e-4, "above the row clamps to its first line");
    let unmeasured = EditorTop { block: Some(3), ..EditorTop::default() };
    assert_eq!(scroll_sync::top_line(&unmeasured, 900.0, (10, 14), 30, line_height), 10.0);
}

#[test]
fn the_preview_reveal_keeps_a_bottom_block_in_view() {
    let doc = lmdv_md::parse("# A\n\nfirst\n\n## B\n\nsecond\n");
    let caret = doc.block_spans[1].line as f32 + 0.5;
    let blocks = vec![preview_block(0, "b0", 300.0, 40.0, 340.0), preview_block(1, "b1", 900.0, 100.0, 1000.0)];
    let geometry = geometry(blocks, 700.0, 2000.0);
    let base = Placement { key: "b0".to_owned(), y: 0.0, target: 300.0, index: Some(0), fallback: false };
    let placement = scroll_sync::reveal_placement(&doc, caret, &base, &geometry).expect("the caret's block");
    assert_eq!(placement.key, "b1");
    // The whole 100 pt block fits; its bottom lands above the viewport's
    // bottom margin (700 - 48), so the target is 1000 - 700 + 48.
    assert!((placement.target - 348.0).abs() < 0.01, "target {}", placement.target);
    assert!((placement.y - (900.0 - 348.0)).abs() < 0.01, "the block's top is 552 pt below the target");
}

#[test]
fn a_table_header_is_covered_when_the_top_line_is_inside_a_table() {
    let doc = lmdv_md::parse("| A | B |\n| --- | --- |\n| one | two |\n| three | four |\n");
    let mut block = preview_block(0, "b0", 40.0, 200.0, 300.0);
    block.table = Some(PreviewTable { top: 40.0, bottom: 240.0, head: 30.0 });
    let geometry = geometry(vec![block], 700.0, 2000.0);
    let line = 2.0; // a row line
    let placement = scroll_sync::preview_placement(&doc, line, false, &geometry).expect("the table block");
    let span = &doc.block_spans[0];
    let next = doc.block_spans.get(1).map_or(span.end_line, |next| next.line);
    let t = ((line - span.line as f32) / (next - span.line) as f32).clamp(0.0, 1.0);
    let y_line = 40.0 + t * 260.0;
    let cover = (y_line - 40.0).min(30.0);
    assert!(cover > 0.0, "the line is under the header");
    let expected = 40.0 - (y_line - 16.0 - cover);
    assert!((placement.y - expected).abs() < 1e-3, "y {} expected {expected}", placement.y);
}

#[test]
fn the_scroll_target_clamps_to_the_article_ends() {
    let doc = lmdv_md::parse("# A\n\nfirst\n\n## B\n\nsecond\n");
    let blocks = vec![preview_block(0, "b0", 40.0, 60.0, 140.0), preview_block(1, "b1", 140.0, 100.0, 300.0)];
    let geometry = geometry(blocks, 700.0, 100.0);
    let line = doc.block_spans[1].line as f32;
    let at_end = scroll_sync::preview_placement(&doc, line, false, &geometry).expect("the second block");
    assert_eq!(at_end.target, 100.0, "the target stops at the article's end");
    let top = scroll_sync::preview_placement(&doc, line, true, &geometry).expect("the second block");
    assert_eq!(top.target, 0.0, "an editor at its top shows the preview's top");
    let line = scroll_sync::top_line(
        &EditorTop { measured: true, start: 16.0, ..EditorTop::default() },
        -50.0,
        (0, 8),
        8,
        22.4,
    );
    assert_eq!(line, 0.0, "a negative offset clamps to line 0");
}

#[test]
fn a_changed_source_offset_requests_a_preview_scroll() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Scroll", &scrolling_markdown(30));
    enter_editor(&mut state);
    state.editor.sync_suppressed_until = None;
    state.editor.last_offset = Some(0.0);
    state.editor.last_caret = Some(0);
    state.editor.editor_top.set(EditorTop { measured: true, start: 16.0, ..EditorTop::default() });
    let doc = state.active_reader().expect("parsed");
    let blocks = (0..3)
        .filter_map(|index| {
            let key = doc.block_key(index)?;
            Some(preview_block(index, key, 40.0 + index as f32 * 200.0, 120.0, 240.0 + index as f32 * 200.0))
        })
        .collect();
    *state.editor.preview.borrow_mut() = geometry(blocks, 700.0, 4000.0);
    let line_height = state.editor_metrics().line_height;
    state.editor.scroll.set(ScrollSnapshot { offset: 16.0 + line_height * 6.0, max: 4000.0, viewport: 700.0 });

    let mut tx = Tx::default();
    state.poll_scroll_sync(1.0, &mut tx);
    let sync = state.editor.last_sync.clone().expect("a mapping");
    assert!(!sync.reveal, "a source scroll is not a caret reveal");
    assert!(sync.line > 5.0, "six lines down, got {}", sync.line);
    assert!(sync.key.starts_with('b'), "a preview block key: {}", sync.key);
    assert!(sync.y < 0.0, "the block's top is scrolled above the viewport: {}", sync.y);
}

#[test]
fn a_caret_move_requests_a_reveal() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Scroll", &scrolling_markdown(30));
    enter_editor(&mut state);
    state.editor.sync_suppressed_until = None;
    state.editor.last_offset = Some(0.0);
    state.editor.last_caret = Some(0);
    state.editor.editor_top.set(EditorTop { measured: true, start: 16.0, ..EditorTop::default() });
    let doc = state.active_reader().expect("parsed");
    let blocks = (0..8)
        .filter_map(|index| {
            let key = doc.block_key(index)?;
            Some(preview_block(index, key, 40.0 + index as f32 * 200.0, 120.0, 240.0 + index as f32 * 200.0))
        })
        .collect();
    *state.editor.preview.borrow_mut() = geometry(blocks, 700.0, 4000.0);
    let caret_byte = state.active_tab().and_then(Tab::document).expect("document").text().line_start(20);
    state.active_editor_edit().set_caret(caret_byte);

    let mut tx = Tx::default();
    state.poll_scroll_sync(1.0, &mut tx);
    let sync = state.editor.last_sync.clone().expect("a mapping");
    assert!(sync.reveal, "the caret moved");
    assert_eq!(sync.line, 20.0);
}

#[test]
fn a_parse_completion_reveals_the_caret_block() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Scroll", &scrolling_markdown(30));
    enter_editor(&mut state);
    state.editor.sync_suppressed_until = None;
    state.editor.editor_top.set(EditorTop { measured: true, start: 16.0, ..EditorTop::default() });
    let doc = state.active_reader().expect("parsed");
    let blocks = (0..4)
        .filter_map(|index| {
            let key = doc.block_key(index)?;
            Some(preview_block(index, key, 40.0 + index as f32 * 200.0, 120.0, 240.0 + index as f32 * 200.0))
        })
        .collect();
    *state.editor.preview.borrow_mut() = geometry(blocks, 700.0, 4000.0);
    let mut tx = Tx::default();
    state.sync_preview_to_caret(&mut tx);
    let sync = state.editor.last_sync.clone().expect("a mapping");
    assert!(sync.reveal);
    assert!(sync.key.starts_with('b'));
}

#[test]
fn block_mode_maps_the_visible_row_to_its_source_line() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_editor_block(&mut state);
    state.editor.sync_suppressed_until = None;
    let span = {
        let tab = state.active_tab().expect("tab");
        let editor = tab.view.editor.as_ref().expect("editor");
        let doc = tab.document().expect("document");
        let block = editor.block_editor().and_then(|blocks| blocks.block(2)).expect("a third block");
        (doc.text().line_of(block.range.start), doc.text().line_of(block.range.end))
    };
    assert!(span.1 > span.0, "the third block spans source lines");
    state.editor.editor_top.set(EditorTop {
        measured: true,
        block: Some(2),
        block_top: 16.0,
        block_height: 80.0,
        ..EditorTop::default()
    });
    state.editor.last_offset = Some(0.0);
    state.editor.last_caret = state.editor_caret_line();
    state.editor.scroll.set(ScrollSnapshot { offset: 16.0 + 40.0, max: 80_000.0, viewport: 700.0 });
    let doc = state.active_reader().expect("parsed");
    let blocks = (0..8)
        .filter_map(|index| {
            let key = doc.block_key(index)?;
            Some(preview_block(index, key, 40.0 + index as f32 * 200.0, 120.0, 240.0 + index as f32 * 200.0))
        })
        .collect();
    *state.editor.preview.borrow_mut() = geometry(blocks, 700.0, 80_000.0);
    let mut tx = Tx::default();
    state.poll_scroll_sync(1.0, &mut tx);
    let sync = state.editor.last_sync.clone().expect("a mapping");
    let expected = span.0 as f32 + (span.1 - span.0) as f32 * 0.5;
    assert!((sync.line - expected).abs() < 1.0, "half a row down block 2: line {} expected {expected}", sync.line);
    assert!(!sync.reveal);
}

#[test]
fn a_source_wheel_scrolls_the_preview() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Scroll", &scrolling_markdown(80));
    enter_editor(&mut state);
    state.editor.sync_suppressed_until = None;
    state.editor.last_offset = Some(0.0);
    state.editor.last_caret = Some(0);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    assert_eq!(article_offset(&runtime), 0.0, "the preview starts at the top");

    // A 320 pt scroll is about 14 source lines at E-37's 22.4 pt line height,
    // far enough that the preview's anchor ends above the viewport.
    runtime.wheel(200.0, 420.0, -320.0);
    for _ in 0..5 {
        runtime.frame(1.0 / 60.0);
    }
    let offset = runtime.state.editor.scroll.get().offset;
    assert!(offset > 200.0, "the source scrolled: {offset}");
    let sync = runtime.state.editor.last_sync.clone().expect("the mapping");
    assert!(!sync.reveal, "a source scroll, not a reveal");
    assert!(sync.line > 0.0, "{sync:?}");
    assert!(sync.y < 0.0, "the anchor's top is scrolled above the preview's top: {}", sync.y);
    assert!(article_offset(&runtime) > 0.0, "the preview followed: {}", article_offset(&runtime));
}

#[test]
fn a_block_mode_source_wheel_scrolls_the_preview_from_the_visible_row() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_editor_block(&mut state);
    state.editor.sync_suppressed_until = None;
    state.editor.last_offset = Some(0.0);
    state.editor.last_caret = state.editor_caret_line();
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    assert_eq!(article_offset(&runtime), 0.0, "the preview starts at the top");

    runtime.wheel(220.0, 500.0, -640.0);
    for _ in 0..8 {
        runtime.frame(1.0 / 60.0);
    }
    let top = runtime.state.editor.editor_top.get();
    let block = top.block.expect("a measured visible row");
    assert!(runtime.state.editor.visible_blocks.as_ref().is_some_and(|range| range.start == block));
    let (start, end) = {
        let tab = runtime.state.active_tab().expect("tab");
        let editor = tab.view.editor.as_ref().expect("editor");
        let doc = tab.document().expect("document");
        let span = editor.block_editor().and_then(|blocks| blocks.block(block)).expect("the row's block");
        (doc.text().line_of(span.range.start), doc.text().line_of(span.range.end))
    };
    let sync = runtime.state.editor.last_sync.clone().expect("the block mapping");
    assert!(!sync.reveal);
    assert!(
        sync.line >= start as f32 && sync.line <= end as f32,
        "line {} is inside the visible block's span {start}..{end}",
        sync.line
    );
    assert!(sync.line > 0.0, "the source scrolled past the first block");
    assert!(article_offset(&runtime) > 0.0, "the preview followed: {}", article_offset(&runtime));
}

#[test]
fn moving_the_caret_to_the_bottom_reveals_its_block() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Scroll", &scrolling_markdown(80));
    enter_editor(&mut state);
    state.editor.sync_suppressed_until = None;
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    assert_eq!(article_offset(&runtime), 0.0);

    let (byte, expected) = {
        let tab = runtime.state.active_tab().expect("tab");
        let doc = tab.document().expect("document");
        (doc.text().line_start(100), doc.text().line_of(doc.text().line_start(100)))
    };
    runtime.update(|state, _| state.active_editor_edit().set_caret(byte));
    for _ in 0..6 {
        runtime.frame(1.0 / 60.0);
    }
    let sync = runtime.state.editor.last_sync.clone().expect("the caret reveal");
    assert!(sync.reveal, "a caret move reveals");
    assert_eq!(sync.line, expected as f32);
    assert!(article_offset(&runtime) > 0.0, "the preview followed the caret: {}", article_offset(&runtime));
    let caret_block = runtime
        .state
        .active_reader()
        .and_then(|doc| doc.block_index_at(byte))
        .and_then(|index| runtime.state.active_reader().and_then(|doc| doc.block_key(index).map(str::to_owned)));
    assert_eq!(Some(sync.key), caret_block, "the reveal targets the caret's block");
}

#[test]
fn manual_preview_scrolling_never_moves_the_editor() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Scroll", &scrolling_markdown(80));
    enter_editor(&mut state);
    state.editor.sync_suppressed_until = None;
    state.editor.last_offset = Some(0.0);
    state.editor.last_caret = Some(0);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    let editor_before = runtime.state.editor.scroll.get();
    assert_eq!(editor_before.offset, 0.0);

    runtime.wheel(800.0, 420.0, -240.0);
    for _ in 0..5 {
        runtime.frame(1.0 / 60.0);
    }
    assert!(article_offset(&runtime) > 0.0, "the preview scrolled: {}", article_offset(&runtime));
    assert_eq!(runtime.state.editor.scroll.get().offset, 0.0, "the editor stayed put");
    assert!(runtime.state.editor.last_sync.is_none(), "no sync was requested");

    // Scrolling the source again moves the preview again.
    runtime.wheel(200.0, 420.0, -160.0);
    for _ in 0..5 {
        runtime.frame(1.0 / 60.0);
    }
    assert!(runtime.state.editor.scroll.get().offset > 100.0, "the source scrolled");
    assert!(runtime.state.editor.last_sync.is_some(), "the preview is following again");
}

#[test]
fn entering_edit_mode_does_not_jump_the_preview_during_entry_settling() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Scroll", &scrolling_markdown(80));
    state.set_pending_line(50);
    let mut tx = Tx::default();
    state.apply_line_target(1.0, &mut tx);
    enter_editor(&mut state);
    // The 200 ms entry window swallows the field's first scroll report.
    state.editor.last_offset = Some(700.0);
    state.editor.scroll.set(ScrollSnapshot { offset: 700.0, max: 4000.0, viewport: 700.0 });
    let mut tx = Tx::default();
    state.poll_scroll_sync(1.0, &mut tx);
    assert!(state.editor.last_sync.is_none(), "the preview did not move on entry settling");
}

/// The focused source field's caret height is its line height: Rune reports
/// what it measured, so this proves the field re-laid out at the new metrics
/// (E-37).
fn focused_caret_height(runtime: &rune::Runtime<WindowState>) -> f32 {
    let field = runtime.elements().into_iter().find(|element| element.focused).expect("a focused source field");
    field.caret.expect("the focused field has a caret")[3]
}

#[test]
fn the_single_source_field_follows_the_reading_size() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Sizes", "# Sizes\n\nOne line of source.\n");
    enter_editor(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let metrics = crate::edit_ui::EditorMetrics::for_reader(16);
    assert!(
        (focused_caret_height(&runtime) - metrics.line_height).abs() < 0.5,
        "the default field is 14 pt on a 22.4 pt line: {}",
        focused_caret_height(&runtime)
    );

    // Changing the Aa size while editing re-measures the same field.
    runtime.update(|state, _| state.set_reader_size(22));
    runtime.frame(1.0 / 60.0);
    let metrics = crate::edit_ui::EditorMetrics::for_reader(22);
    assert!(
        (focused_caret_height(&runtime) - metrics.line_height).abs() < 0.5,
        "19.25 pt on a 30.8 pt line: {}",
        focused_caret_height(&runtime)
    );
    assert_eq!(metrics.size, 19.25);
}

#[test]
fn block_mode_fields_follow_the_reading_size() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_editor_block(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let metrics = crate::edit_ui::EditorMetrics::for_reader(16);
    assert!(
        (focused_caret_height(&runtime) - metrics.line_height).abs() < 0.5,
        "a block field starts at 14 pt on a 22.4 pt line: {}",
        focused_caret_height(&runtime)
    );

    runtime.update(|state, _| state.set_reader_size(22));
    runtime.frame(1.0 / 60.0);
    let metrics = crate::edit_ui::EditorMetrics::for_reader(22);
    assert!(
        (focused_caret_height(&runtime) - metrics.line_height).abs() < 0.5,
        "the block field re-measured at 19.25 pt on a 30.8 pt line: {}",
        focused_caret_height(&runtime)
    );
}

// ---- spellcheck (ADR-008, E-SPELL, E-32, P-11) ----------------------------

/// The fixture dictionary pair under `tests/fixtures/spell`.
fn spell_fixture() -> Availability {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/spell/system");
    Availability {
        files: DictionaryFiles {
            language: "en_US".to_owned(),
            aff: root.join("en_US.aff"),
            dic: root.join("en_US.dic"),
        },
        fallback: false,
    }
}

/// Seeds the window's checker with the fixture, as a finished discovery and
/// load would. The check itself still runs on a worker.
fn seed_spell(state: &WindowState) -> Availability {
    let availability = spell_fixture();
    let dictionary = Arc::new(Spell::open(&availability.files).expect("the fixture dictionary parses"));
    state.shared().seed_spell(Some(availability.clone()), Some(dictionary));
    availability
}

/// Ranges for `(start, end)` word pairs, in text order.
fn spell_ranges(pairs: &[(usize, usize)]) -> Vec<std::ops::Range<usize>> {
    pairs.iter().map(|&(start, end)| start..end).collect()
}

/// Polls with an advancing clock until the tab has exactly `expected` ranges.
fn wait_for_spell(state: &mut WindowState, tab: lmdv_core::TabId, expected: &[std::ops::Range<usize>]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut tx = Tx::default();
    while state.editor.spell.ranges(tab) != Some(expected) {
        assert!(Instant::now() < deadline, "the spelling check never reported: {:?}", state.editor.spell.ranges(tab));
        state.poll(1.0, &mut tx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The syntax spans of the active single field, with spelling underlines.
fn painted_spans(state: &WindowState) -> Vec<rune::Span> {
    let tab = state.active_tab().expect("the tab");
    let editor = tab.view.editor.as_ref().expect("the editor");
    state.editor_spans(tab, &editor.edit, &SyntaxTheme::neutral(true), None, rune::Color::WHITE)
}

#[test]
fn spelling_waits_for_the_300ms_debounce_and_paints_the_editor() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("spell.md");
    std::fs::write(&path, "hello wrld\n").unwrap();
    let mut state = window(&temp);
    seed_spell(&state);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    let tab = state.active_tab().map(|tab| tab.id).expect("the tab");

    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert!(!state.editor.spell.is_checking(tab), "nothing runs inside the debounce");
    assert!(state.editor.spell.ranges(tab).is_none());
    state.poll(0.29, &mut tx);
    assert!(!state.editor.spell.is_checking(tab), "290 ms is still inside it");
    state.poll(0.31, &mut tx);
    assert!(state.editor.spell.is_checking(tab), "300 ms starts one check");

    wait_for_spell(&mut state, tab, &spell_ranges(&[(6, 10)]));
    assert_eq!(state.editor.spell.ranges(tab), Some(spell_ranges(&[(6, 10)]).as_slice()), "only `wrld` is misspelled");
    let painted = painted_spans(&state);
    assert!(
        painted.iter().any(|span| span.underline.is_some() && (span.start, span.end) == (6, 10)),
        "the word carries an underline: {painted:?}"
    );

    // An edit bumps the revision: the old ranges are never painted over it.
    type_text(&mut state, "New ");
    let painted = painted_spans(&state);
    assert!(
        painted.iter().all(|span| span.underline.is_none()),
        "the answer for the previous revision is dropped: {painted:?}"
    );
}

#[test]
fn block_mode_checks_and_paints_only_the_visible_rows() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("blocks.md");
    std::fs::write(&path, "hello wrld\n\nhello wrld\n").unwrap();
    let mut state = window(&temp);
    seed_spell(&state);
    state.set_large_file_mode(LargeFileMode::Block);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    let tab = state.active_tab().map(|tab| tab.id).expect("the tab");
    assert!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|e| e.block_mode_active()));

    // The first block is the visible one.
    state.editor.visible_blocks = Some(0..1);
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    state.poll(0.31, &mut tx);
    wait_for_spell(&mut state, tab, &spell_ranges(&[(6, 10)]));
    assert_eq!(
        state.editor.spell.ranges(tab),
        Some(spell_ranges(&[(6, 10)]).as_slice()),
        "only the visible block was checked"
    );

    let underlined = |state: &WindowState, index: usize| {
        let block = state
            .active_tab()
            .and_then(|tab| tab.view.editor.as_ref())
            .and_then(|editor| editor.block_editor())
            .and_then(|blocks| blocks.block(index))
            .expect("the block");
        let spans =
            state.editor_block_spans(&block.edit, &SyntaxTheme::neutral(true), block.range.clone(), rune::Color::WHITE);
        spans.iter().filter(|span| span.underline.is_some()).map(|span| (span.start, span.end)).collect::<Vec<_>>()
    };
    assert_eq!(underlined(&state, 0), vec![(6, 10)], "block-local coordinates");
    assert!(underlined(&state, 1).is_empty());

    // Scrolling checks the newly visible rows without a keystroke.
    state.editor.visible_blocks = Some(1..2);
    state.poll(1.0, &mut tx);
    wait_for_spell(&mut state, tab, &spell_ranges(&[(18, 22)]));
    assert_eq!(state.editor.spell.ranges(tab), Some(spell_ranges(&[(18, 22)]).as_slice()));
    assert!(underlined(&state, 0).is_empty(), "the first block is no longer covered");
    assert_eq!(underlined(&state, 1), vec![(6, 10)]);
}

#[test]
fn without_a_dictionary_the_editor_never_checks_or_paints() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("plain.md");
    std::fs::write(&path, "hello wrld\n").unwrap();
    let mut state = window(&temp);
    state.shared().seed_spell(None, None);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    let tab = state.active_tab().map(|tab| tab.id).expect("the tab");

    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    state.poll(1.0, &mut tx);
    assert!(!state.editor.spell.is_checking(tab), "no engine, no job");
    assert!(state.editor.spell.ranges(tab).is_none());
    assert!(painted_spans(&state).iter().all(|span| span.underline.is_none()));
}

#[test]
fn turning_spellcheck_off_stops_the_checker_and_the_underlines() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("spell.md");
    std::fs::write(&path, "hello wrld\n").unwrap();
    let mut state = window(&temp);
    seed_spell(&state);
    open_loaded(&mut state, &path);
    enter_editor(&mut state);
    let tab = state.active_tab().map(|tab| tab.id).expect("the tab");
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    state.poll(0.31, &mut tx);
    wait_for_spell(&mut state, tab, &spell_ranges(&[(6, 10)]));

    state.set_spellcheck(false);
    assert!(painted_spans(&state).iter().all(|span| span.underline.is_none()), "off hides the squiggles at once");
    state.poll(2.0, &mut tx);
    assert!(!state.editor.spell.is_checking(tab), "off stops new checks");

    state.set_spellcheck(true);
    state.poll(2.0, &mut tx);
    state.poll(2.31, &mut tx);
    assert!(state.editor.spell.is_checking(tab), "on checks again without waiting for a keystroke");
}

#[test]
fn the_app_discovers_a_dictionary_beside_its_data_and_checks_with_it() {
    let temp = TempDir::new().unwrap();
    let dirs = test_support::dirs(&temp);
    // The launch's UI locale, so a system dictionary for another language can
    // never win over the fixture in the app's own folder.
    std::fs::create_dir_all(&dirs.config).unwrap();
    std::fs::write(dirs.prefs_path(), br#"{"locale": "en"}"#).unwrap();
    let dictionaries = dirs.data.join("dictionaries");
    std::fs::create_dir_all(&dictionaries).unwrap();
    let fixture = spell_fixture().files;
    for path in [&fixture.aff, &fixture.dic] {
        std::fs::copy(path, dictionaries.join(path.file_name().expect("a file name"))).unwrap();
    }

    let path = temp.path().join("discovered.md");
    std::fs::write(&path, "hello wrld\n").unwrap();
    let mut state = WindowState::new(AppShared::start(dirs));
    open_loaded(&mut state, &path);

    // Discovery runs from the first poll, on a worker (ADR-008).
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut now = 0.0;
    let mut tx = Tx::default();
    while state.spell_availability().is_none() {
        assert!(Instant::now() < deadline, "the app never found the dictionary beside its data");
        state.poll(now, &mut tx);
        std::thread::sleep(Duration::from_millis(5));
    }
    let availability = state.spell_availability().expect("the fixture dictionary");
    assert_eq!(availability.language(), "en_US");

    enter_editor(&mut state);
    let tab = state.active_tab().map(|tab| tab.id).expect("the tab");
    while state.editor.spell.ranges(tab).is_none_or(|ranges| ranges.is_empty()) {
        assert!(Instant::now() < deadline, "the app never loaded and checked with the dictionary");
        now += 0.1;
        state.poll(now, &mut tx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(state.editor.spell.ranges(tab), Some(spell_ranges(&[(6, 10)]).as_slice()));
}

#[test]
fn a_watch_overflow_is_one_notice_and_the_documents_reload() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("overflow.md");
    std::fs::write(&path, "one\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);

    assert!(state.apply_file_event(FileEvent::Overflow), "the overflow redraws");
    let notice = state.ui.notice.as_ref().map(|notice| notice.text.clone()).unwrap_or_default();
    assert!(notice.contains("Some file changes were missed"), "OI-178 surfaces it: {notice}");

    // The re-probe events that follow still reload on the normal path.
    std::fs::write(&path, "two\n").unwrap();
    assert!(state.apply_file_event(FileEvent::Changed { path: path.clone() }));
    poll_until(&mut state, "the re-probe reload", |state| {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| doc.text().text() == "two\n")
    });
}
