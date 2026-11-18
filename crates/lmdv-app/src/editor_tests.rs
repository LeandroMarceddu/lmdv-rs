//! P3 integration: edit mode, document sync, saving, EOL, conflicts, drafts,
//! the watcher and the close guards.
//!
//! These tests drive `WindowState` directly (no window), with real files in a
//! temporary directory. Jobs run on Rune's task pool exactly as they do in the
//! window; the helpers poll until the completion lands.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use lmdv_core::{DocStatus, Draft, Eol, ScrollAnchor};
use lmdv_io::watch::FileEvent;
use rune::{ElementInfo, Tx};
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
fn a_draft_equal_to_the_file_is_dropped_silently() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("same.md");
    std::fs::write(&path, "same\n").unwrap();
    let mut state = window(&temp);
    open_loaded(&mut state, &path);
    let doc = state.active_document_id().expect("document");
    let key = path.to_string_lossy().into_owned();

    assert!(!state.handle_draft_loaded(doc, key.clone(), Some(Draft::new("same\n", "older\n", 1))));
    assert!(state.prompt().is_none(), "an identical draft is not offered");
    flush_store(&state);
    assert!(lmdv_io::store::load_draft(&test_support::dirs(&temp), &key).is_none(), "and it is removed");
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
    let mut tx = Tx::default();
    state.request_reload(&mut tx);
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
    assert_eq!(
        state.active_tab().and_then(|tab| tab.view.reader_revision),
        Some(before),
        "the debounce holds the old model"
    );
    assert!(state.active_tab().is_some_and(|tab| tab.view.preview_pending));
    assert_eq!(
        state.active_tab().and_then(|tab| tab.view.reader_revision),
        Some(before),
        "no parse during the debounce"
    );

    state.poll(0.2, &mut tx);
    poll_until(&mut state, "the preview parse", |state| {
        state.active_tab().and_then(|tab| tab.view.reader_revision) != Some(before)
    });
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("new model");
    assert!(model.source.starts_with("New "), "the preview parsed the edited source");
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
