//! P4-02b: cross-window tab transfer (T-21/T-22), its lease/subscriber/watch
//! handoff, the tab and window menus, and the opener seam.
//!
//! Every test drives two `WindowState`s on one `AppShared` exactly as the
//! runtime windows do, draining the same-thread mailboxes in turn.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use lmdv_core::{Document, DocumentId, DocumentKey, EditLease, ScrollAnchor, SourceBuffer, TabId};
use rune::{PointerPress, Tx};
use tempfile::TempDir;

use crate::editor::Prompt;
use crate::shared::AppShared;
use crate::test_support;
use crate::transfer::TransferMsg;
use crate::window::{Mode, Tab, WindowState};
use crate::windows::{OpenedWindow, WindowOpenRequest, window_key};

fn shared(temp: &TempDir) -> Rc<AppShared> {
    let shared = test_support::shared(temp);
    // Parses run inline and draft checks stay out of the way, as in every
    // other headless app test.
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
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
}

/// The round trip of one transfer: the target adopts and answers, then the
/// source consumes the answer. The source is polled last because the target's
/// `Adopted` reply lands in its inbox during the target's poll.
fn poll_both(source: &mut WindowState, target: &mut WindowState) {
    poll(target);
    poll(source);
}

fn edit_session(document: &Document) -> lmdv_edit::EditorSession {
    let (text, eol, revision) = {
        let buffer = document.text();
        (buffer.text().to_owned(), buffer.eol(), buffer.revision())
    };
    let mut session = lmdv_edit::EditorSession::new(&text, eol);
    session.set_doc_revision(revision);
    session.set_base_revision(revision);
    session
}

/// Puts one tab in edit mode and takes its lease, without the UI path.
fn enter_editor(state: &mut WindowState, tab: TabId) {
    let document = state.tab(tab).and_then(Tab::document).cloned().expect("document");
    document.acquire_edit(state.id(), tab).expect("the lease");
    let base = document.text().text().to_owned();
    let session = edit_session(&document);
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.mode = Mode::Editor;
        tab.view.editor = Some(session);
        tab.view.edit_base = Some(base);
        tab.view.dirty = document.is_dirty();
    }
}

fn tab_of(state: &WindowState, document: DocumentId) -> TabId {
    state
        .tabs()
        .iter()
        .find(|tab| tab.document_id() == Some(document))
        .map(|tab| tab.id)
        .expect("the window shows the document")
}

fn enabled(state: &WindowState, id: &str) -> bool {
    crate::commands::lookup_command(id).is_some_and(|command| (command.enabled)(state))
}

#[test]
fn an_insertion_slot_wins_over_the_active_start_page() {
    use crate::transfer::adopt_index;

    // The real crash: a one-tab target showing its start page, with a hover
    // slot past the last tab. It must append, never index 1 of a length-1 vec.
    assert_eq!(adopt_index(None, Some(1), 0, 1, true), (1, false), "a hover slot past the last tab appends");
    assert_eq!(adopt_index(Some(0), None, 0, 1, true), (0, false), "an explicit slot inserts");
    assert_eq!(adopt_index(None, None, 0, 1, true), (0, true), "a menu move replaces the front start page");
    assert_eq!(adopt_index(None, None, 2, 4, false), (3, false), "otherwise after the active tab");
    assert_eq!(adopt_index(Some(9), None, 2, 4, false), (4, false), "a slot past the end clamps to an append");
}

#[test]
fn a_drop_slot_past_the_last_tab_appends() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("append.md");
    let document = ready_file(&shared, &path, "# Append\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    source.sync_registry();
    target.sync_registry();
    assert!(target.active_tab().is_some_and(Tab::is_start));

    source.move_tab_to_window_at(source_tab, target.id(), Some(1)).expect("queued");
    poll(&mut target);
    assert_eq!(target.tabs().len(), 2);
    assert!(target.tabs()[0].is_start());
    assert_eq!(target.tabs()[1].document_id(), Some(document.id));
}

#[test]
fn a_clean_tab_moves_with_its_view_state_and_the_watch() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("clean.md");
    let document = ready_file(&shared, &path, "# Clean\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    source.open_synthetic("Other", "# Other\n");
    source.activate_tab(source_tab);
    source.outline_filter.replace_text("methods");
    let anchor = ScrollAnchor { id: "intro".to_owned(), offset: 18.0 };
    {
        let tab = source.tabs.iter_mut().find(|tab| tab.id == source_tab).expect("tab");
        tab.view.anchor = anchor.clone();
    }
    source.sync_registry();
    target.sync_registry();
    assert_eq!(shared.watch_refcount(&path), 1);

    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    assert_eq!(source.transfer.pending_len(), 1);
    assert!(source.tab(source_tab).is_none(), "the source detached the tab");
    assert_eq!(source.tabs().len(), 1, "the other tab remains");
    assert!(source.active_document_id().is_some_and(|id| id != document.id), "the other tab comes forward");
    assert_eq!(shared.watch_refcount(&path), 0, "the source stopped following");
    assert_eq!(document.subscribers.borrow().len(), 1, "the subscriber entry stays until the answer");
    assert_eq!(document.lease_holder(), None);

    poll(&mut target);
    let target_tab = tab_of(&target, document.id);
    assert_eq!(target.tabs().len(), 1, "the adopted tab replaced the start page");
    assert_eq!(target.active_document_id(), Some(document.id));
    assert_eq!(target.tab(target_tab).map(|tab| tab.view.anchor.clone()), Some(anchor));
    assert_eq!(target.tab(target_tab).map(|tab| tab.view.toc_filter.clone()), Some("methods".to_owned()));
    assert_eq!(shared.watch_refcount(&path), 1, "the watch token moved with the tab");
    assert_eq!(document.subscribers.borrow().len(), 2, "the source removes itself only on Adopted");

    poll(&mut source);
    assert_eq!(source.transfer.pending_len(), 0);
    assert_eq!(document.subscribers.borrow().len(), 1, "the source is no longer a view");
    assert!(document.subscribers.borrow().contains(&target.id()));
    assert_eq!(shared.watch_refcount(&path), 1);
    assert_eq!(shared.windows().focus_order().first(), Some(&target.id()), "the target is focused");
    {
        let windows = shared.windows();
        let published = windows.get(target.id()).expect("target entry");
        assert_eq!(published.active_document, Some(document.id));
        assert_eq!(published.documents.len(), 1);
    }
}

#[test]
fn moving_the_last_tab_leaves_a_start_page() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("only.md");
    let document = ready_file(&shared, &path, "# Only\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    source.sync_registry();

    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    assert_eq!(source.tabs().len(), 1);
    assert!(source.active_tab().is_some_and(Tab::is_start), "a start page takes its place");
    assert_eq!(source.window_title(), "lmdv");
    poll_both(&mut source, &mut target);
    assert_eq!(source.tabs().len(), 1);
    assert!(source.active_tab().is_some_and(Tab::is_start));
    assert_eq!(target.active_document_id(), Some(document.id));
    assert_eq!(shared.windows().get(target.id()).and_then(|entry| entry.active_document), Some(document.id));
}

#[test]
fn a_dirty_last_view_moves_with_its_text_and_no_prompt() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("dirty.md");
    let document = ready_file(&shared, &path, "base\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    document.apply(0..0, "edited ");
    source.sync_tab_dirty(&document);
    source.sync_registry();
    assert!(document.is_dirty());

    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    assert!(source.prompt().is_none(), "a move is not a close");
    poll_both(&mut source, &mut target);
    assert!(source.prompt().is_none());
    assert!(target.prompt().is_none());
    assert_eq!(source.tabs().len(), 1);
    assert!(source.active_tab().is_some_and(Tab::is_start));
    let target_tab = tab_of(&target, document.id);
    assert!(target.tab(target_tab).is_some_and(|tab| tab.view.dirty), "the dirty dot travels");
    assert_eq!(document.text().text(), "edited base\n");
    assert_eq!(document.subscribers.borrow().len(), 1);
}

#[test]
fn a_clean_duplicate_merges_into_the_existing_tab() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("shared.md");
    let document = ready_file(&shared, &path, "# Shared\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    let target_tab = target.attach_document(Rc::clone(&document));
    source.sync_registry();
    target.sync_registry();
    enter_editor(&mut source, source_tab);
    assert_eq!(document.lease_holder(), Some(EditLease { window: source.id(), tab: source_tab }));

    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    poll(&mut target);
    assert_eq!(target.tabs().len(), 1, "one tab per document per window");
    assert_eq!(target.active_tab().map(|tab| tab.id), Some(target_tab), "the existing tab is activated");
    assert_eq!(document.lease_holder(), None, "a clean editor session ends with the merged tab");
    assert_eq!(document.subscribers.borrow().len(), 2, "the source still owns its view until the reply");

    poll(&mut source);
    assert_eq!(source.tabs().len(), 1, "the source kept its start page");
    assert!(source.active_tab().is_some_and(Tab::is_start));
    assert_eq!(source.transfer.pending_len(), 0);
    assert_eq!(document.subscribers.borrow().len(), 1);
    assert!(document.subscribers.borrow().contains(&target.id()));
    assert_eq!(document.text().text(), "# Shared\n");
    assert_eq!(shared.windows().focus_order().first(), Some(&target.id()));
}

#[test]
fn a_dirty_duplicate_is_refused_and_returned_with_text_and_lease_intact() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("refused.md");
    let document = ready_file(&shared, &path, "base\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    target.attach_document(Rc::clone(&document));
    source.sync_registry();
    target.sync_registry();
    enter_editor(&mut source, source_tab);
    document.apply(0..0, "edited ");
    source.sync_tab_dirty(&document);
    target.sync_tab_dirty(&document);
    let lease = EditLease { window: source.id(), tab: source_tab };
    assert_eq!(document.lease_holder(), Some(lease));

    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    poll(&mut target);
    assert_eq!(target.tabs().len(), 1, "the target added no tab");
    assert!(
        target.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("already open")),
        "the target names the refused document"
    );

    poll(&mut source);
    assert_eq!(source.tabs().len(), 1, "the tab came back");
    let tab = source.tab(source_tab).expect("the same tab id");
    assert_eq!(tab.view.mode, Mode::Editor, "the editor session survived");
    assert!(tab.view.editor.is_some());
    assert_eq!(document.lease_holder(), Some(lease), "the source kept the lease");
    assert_eq!(document.text().text(), "edited base\n");
    assert!(document.is_dirty());
    assert_eq!(document.subscribers.borrow().len(), 2);
    assert_eq!(shared.watch_refcount(&path), 2, "both views follow again");
    assert_eq!(source.transfer.pending_len(), 0);
    assert!(source.prompt().is_none());
}

#[test]
fn an_editing_tab_transfers_the_lease_and_the_source_becomes_a_viewer() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("editing.md");
    let document = ready_file(&shared, &path, "# Editing\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    enter_editor(&mut source, source_tab);
    assert_eq!(document.lease_holder().map(|lease| lease.window), Some(source.id()));

    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    poll_both(&mut source, &mut target);
    let target_tab = tab_of(&target, document.id);
    assert_eq!(target.active_tab().map(|tab| tab.id), Some(target_tab));
    assert_eq!(target.tab(target_tab).map(|tab| tab.view.mode), Some(Mode::Editor));
    assert!(target.tab(target_tab).is_some_and(|tab| tab.view.editor.is_some()));
    assert_eq!(
        document.lease_holder(),
        Some(EditLease { window: target.id(), tab: target_tab }),
        "the lease moved with the tab"
    );
    assert!(source.tabs().iter().all(|tab| tab.view.mode == Mode::Viewer), "the source is a viewer again");
    assert!(source.active_tab().is_some_and(Tab::is_start));
    assert_eq!(document.subscribers.borrow().len(), 1);
}

#[test]
fn moving_the_last_tab_keeps_the_watch_count_and_restores_it_when_returned() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("watched.md");
    let document = ready_file(&shared, &path, "one\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    assert_eq!(shared.watch_refcount(&path), 1);

    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    assert_eq!(shared.watch_refcount(&path), 0, "no view follows while the tab is in flight");
    poll(&mut target);
    assert_eq!(shared.watch_refcount(&path), 1, "the target's view follows");
    poll(&mut source);
    assert_eq!(shared.watch_refcount(&path), 1);
}

#[test]
fn a_closing_target_returns_the_tab_to_its_place() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let first = source.open_synthetic("First", "# First\n");
    let second = source.open_synthetic("Second", "# Second\n");
    source.activate_tab(second);
    let document = source.tab(second).and_then(Tab::document).cloned().expect("doc");
    source.sync_registry();
    target.sync_registry();
    shared.set_window_closing(target.id(), true);
    assert!(source.window_targets().is_empty(), "a closing window is not offered");

    source.move_tab_to_window(second, target.id()).expect("queued");
    assert_eq!(source.tabs().len(), 1);
    assert_eq!(source.active_tab().map(|tab| tab.id), Some(first));
    poll(&mut target);
    assert_eq!(target.tabs().len(), 1, "the closing target took nothing");
    assert!(target.active_tab().is_some_and(Tab::is_start));

    poll(&mut source);
    assert_eq!(source.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>(), vec![first, second]);
    assert_eq!(source.active_tab().map(|tab| tab.id), Some(first), "the neighbour stays in front");
    assert_eq!(document.subscribers.borrow().len(), 1, "the source never lost its subscriber");
    assert_eq!(source.transfer.pending_len(), 0);
}

#[test]
fn a_returned_tab_lands_at_the_clamped_origin_index() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let first = source.open_synthetic("First", "# First\n");
    let second = source.open_synthetic("Second", "# Second\n");
    let third = source.open_synthetic("Third", "# Third\n");
    source.activate_tab(third);
    source.sync_registry();
    shared.set_window_closing(target.id(), true);

    source.move_tab_to_window(third, target.id()).expect("queued");
    assert_eq!(source.tabs().len(), 2);
    source.close_tab(second);
    assert_eq!(source.tabs().len(), 1, "the strip shrank while the tab was away");
    poll(&mut target);
    poll(&mut source);
    assert_eq!(source.tabs().len(), 2);
    assert_eq!(source.tabs()[1].id, third, "min(origin_index, len)");
    assert_eq!(source.active_tab().map(|tab| tab.id), Some(first));
}

#[test]
fn a_drop_slot_inserts_at_the_requested_index() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("dropped.md");
    let document = ready_file(&shared, &path, "# Dropped\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    let a = target.open_synthetic("A", "# A\n");
    let b = target.open_synthetic("B", "# B\n");
    target.activate_tab(b);
    target.sync_registry();

    source.move_tab_to_window_at(source_tab, target.id(), Some(0)).expect("queued");
    poll(&mut target);
    assert_eq!(target.tabs().len(), 3);
    assert_eq!(target.tabs()[0].document_id(), Some(document.id));
    assert_eq!(target.tabs()[1].id, a);
    assert_eq!(target.tabs()[2].id, b);
    assert_eq!(target.active_tab().and_then(Tab::document_id), Some(document.id));
}

#[test]
fn a_background_tab_move_keeps_the_front_tab() {
    let temp = TempDir::new().unwrap();
    let (_shared, mut source, mut target) = two_windows(&temp);
    let a = source.open_synthetic("A", "# A\n");
    let b = source.open_synthetic("B", "# B\n");
    let c = source.open_synthetic("C", "# C\n");
    source.sync_registry();
    let moved = source.tab(a).and_then(Tab::document).cloned().expect("doc");

    source.move_tab_to_window(a, target.id()).expect("queued");
    assert_eq!(source.active_tab().map(|tab| tab.id), Some(c), "the front tab did not move");
    poll_both(&mut source, &mut target);
    assert_eq!(source.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>(), vec![b, c]);
    assert_eq!(target.active_document_id(), Some(moved.id));
    assert_eq!(moved.subscribers.borrow().len(), 1);
}

#[test]
fn a_vanished_target_leaves_the_tab_where_it_was() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, target) = two_windows(&temp);
    let path = temp.path().join("vanished.md");
    let document = ready_file(&shared, &path, "text\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    let target_id = target.id();
    drop(target);
    source.sync_registry();

    let error = source.move_tab_to_window(source_tab, target_id).expect_err("the target is gone");
    assert!(error.contains("disappeared"), "{error}");
    assert_eq!(source.tabs().len(), 1);
    assert_eq!(source.active_tab().map(|tab| tab.id), Some(source_tab));
    assert_eq!(source.transfer.pending_len(), 0);
    assert_eq!(shared.watch_refcount(&path), 1);
    assert_eq!(document.subscribers.borrow().len(), 1);
}

#[test]
fn an_in_flight_target_that_vanishes_is_reaped() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, target) = two_windows(&temp);
    let path = temp.path().join("reaped.md");
    let document = ready_file(&shared, &path, "text\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    source.sync_registry();
    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    assert_eq!(source.transfer.pending_len(), 1);

    // The target leaves the registry without answering (the original's
    // 15 s timeout is not needed in-process; this is its cleanup).
    shared.unregister_window(target.id());
    poll(&mut source);
    assert_eq!(source.transfer.pending_len(), 0);
    assert!(document.subscribers.borrow().is_empty(), "the lost view's subscriber is gone");
    assert_eq!(shared.watch_refcount(&path), 0, "the tab's watch went with it");
    drop(target);
}

#[test]
fn the_window_chooser_lists_other_open_non_closing_windows() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let tab = source.open_synthetic("Source", "# Source\n");
    target.open_synthetic("Second notes", "# Second\n");
    target.sync_registry();
    source.sync_registry();

    let targets = source.window_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, target.id());
    assert_eq!(targets[0].title, "Second notes — lmdv");

    source.show_tab_menu(tab, PointerPress { x: 12.0, y: 24.0 });
    assert_eq!(source.tab_menu().map(|menu| menu.chooser), Some(false));
    source.show_tab_window_menu(tab, None);
    assert_eq!(source.tab_menu().map(|menu| menu.chooser), Some(true));
    source.close_tab_menu();
    assert!(source.tab_menu().is_none());

    shared.set_window_closing(target.id(), true);
    assert!(source.window_targets().is_empty(), "a closing window is skipped");
}

#[test]
fn the_transfer_commands_enable_only_when_a_move_is_possible() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, target) = two_windows(&temp);
    assert!(!enabled(&source, "tab.move-new-window"), "a start page cannot move");
    assert!(!enabled(&source, "tab.move-to-window"));

    let tab = source.open_synthetic("Source", "# Source\n");
    source.sync_registry();
    assert!(enabled(&source, "tab.move-new-window"));
    assert!(enabled(&source, "tab.move-to-window"), "the second window is a target");

    shared.set_window_closing(target.id(), true);
    assert!(!enabled(&source, "tab.move-to-window"), "the only target is closing");
    shared.set_window_closing(target.id(), false);
    assert!(enabled(&source, "tab.move-to-window"));

    source.editor.prompt = Some(Prompt::CloseWindow);
    assert!(!enabled(&source, "tab.move-new-window"), "a dialog keeps the tab in place");
    source.editor.prompt = None;

    let document = source.tab(tab).and_then(Tab::document).cloned().expect("doc");
    source.editor.save_texts.insert(document.id, "saving".to_owned());
    assert!(!enabled(&source, "tab.move-new-window"), "a save in flight keeps the tab in place");
    source.editor.save_texts.clear();

    let loading =
        shared.docs_mut().insert_loading(DocumentKey::File(temp.path().join("loading.md")), "loading.md".to_owned());
    source.attach_document(loading);
    assert!(!enabled(&source, "tab.move-new-window"), "a loading tab stays until its load lands");
}

#[test]
fn a_single_window_has_no_move_target_but_can_tear_out() {
    let temp = TempDir::new().unwrap();
    let shared = shared(&temp);
    let mut alone = WindowState::new(Rc::clone(&shared));
    assert!(!enabled(&alone, "tab.move-new-window"), "the start page cannot move");
    alone.open_synthetic("Alone", "# Alone\n");
    assert!(alone.window_targets().is_empty(), "the current window is never a target");
    assert!(!enabled(&alone, "tab.move-to-window"));
    assert!(enabled(&alone, "tab.move-new-window"), "a new window is always a target");
}

#[test]
fn hover_opens_and_clears_the_incoming_slot() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let token = shared.allocate_transfer();
    assert!(shared.post_window(
        target.id(),
        crate::shared::UiMsg::Transfer(TransferMsg::Hover { token, target: target.id(), point: Some([12.0, 34.0]) })
    ));
    poll(&mut target);
    assert_eq!(target.transfer.hover().map(|hover| hover.point), Some([12.0, 34.0]));

    assert!(shared.post_window(
        target.id(),
        crate::shared::UiMsg::Transfer(TransferMsg::Hover { token, target: target.id(), point: None })
    ));
    poll(&mut target);
    assert!(target.transfer.hover().is_none());

    // An adoption clears a stale hover from the same drag.
    let path = temp.path().join("hover.md");
    let document = ready_file(&shared, &path, "# Hover\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    source.sync_registry();
    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    poll(&mut target);
    assert!(target.transfer.hover().is_none());
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

#[test]
fn move_to_new_window_uses_the_opener_seam() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, _target) = two_windows(&temp);
    OPENED.with(|opened| opened.borrow_mut().clear());
    OPEN_TARGET.with(|target| *target.borrow_mut() = None);
    shared.set_window_opener(target_opener);
    let tab = source.open_synthetic("Torn", "# Torn\n");
    let document = source.tab(tab).and_then(Tab::document).cloned().expect("doc");

    source.move_tab_to_new_window(tab);
    let mut target = OPEN_TARGET.with(|target| target.borrow_mut().take()).expect("the opener built a window");
    assert_eq!(OPENED.with(|opened| opened.borrow().len()), 1);
    assert!(source.tab(tab).is_none(), "the source detached the tab");
    assert!(source.active_tab().is_some_and(Tab::is_start));

    poll(&mut target);
    poll(&mut source);
    assert_eq!(target.tabs().len(), 1, "the new window replaced its start page");
    assert_eq!(target.active_document_id(), Some(document.id));
    assert_eq!(document.subscribers.borrow().len(), 1);
    assert!(document.subscribers.borrow().contains(&target.id()));
    assert_eq!(shared.windows().len(), 3, "two test windows plus the opened one");
}

#[test]
fn dropping_the_source_while_pending_lets_the_target_adopt() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("orphaned.md");
    let document = ready_file(&shared, &path, "# Orphan\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    source.sync_registry();
    source.move_tab_to_window(source_tab, target.id()).expect("queued");

    // The source closes before the target's poll: its subscriber entry and
    // lease go, while the tab in flight keeps the document registered.
    drop(source);
    assert!(shared.docs().get(document.id).is_some(), "the tab in flight keeps the document registered");
    assert!(document.subscribers.borrow().is_empty(), "the source's subscriber entry is gone");
    poll(&mut target);
    assert_eq!(target.active_document_id(), Some(document.id));
    assert_eq!(document.subscribers.borrow().len(), 1);
    assert!(document.subscribers.borrow().contains(&target.id()));
    assert_eq!(document.lease_holder(), None);
}

#[test]
fn a_sent_tab_keeps_its_subscriber_until_the_reply() {
    let temp = TempDir::new().unwrap();
    let (shared, mut source, mut target) = two_windows(&temp);
    let path = temp.path().join("pending.md");
    let document = ready_file(&shared, &path, "# Pending\n");
    let source_tab = source.attach_document(Rc::clone(&document));
    source.sync_registry();

    source.move_tab_to_window(source_tab, target.id()).expect("queued");
    assert_eq!(source.transfer.pending_len(), 1);
    assert_eq!(document.subscribers.borrow().len(), 1, "the source keeps its subscriber entry");
    let token = source.transfer.pending_token().expect("one in-flight tab");
    assert_eq!(source.transfer.pending_target(token), Some(target.id()));
    poll_both(&mut source, &mut target);
    assert_eq!(source.transfer.pending_len(), 0);
    assert_eq!(document.subscribers.borrow().len(), 1);
    assert!(document.subscribers.borrow().contains(&target.id()));
}
