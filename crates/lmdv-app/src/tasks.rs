//! Live task checkboxes in the reading view (M-07, E-08).
//!
//! The renderer draws every task as a checkbox and hands the real control to
//! the window's [`TaskControlWire`](lmdv_render::TaskControlWire); this module
//! makes that control live, locates the exact `[ ]`/`[x]` bytes in the source
//! (from the wire's source range, OI-092), flips one byte in the shared
//! [`Document`](lmdv_core::Document), and writes the file through a `lmdv-io`
//! save job. It is the one deliberate exception to "the file only changes on
//! Ctrl+S" (E-08).
//!
//! The success notice carries an Undo action; the conflict prompt offers the
//! same **Overwrite** / **Keep theirs** choice as an editor save, but restores
//! the buffer before asking so a failed tick never leaves an unsaved viewer
//! write behind.
//!
//! Only viewer tabs get the wire: the editor's preview and source stay
//! read-only for task clicks, so a tick can never race the editor's buffer.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use lmdv_core::{DocStatus, Document, DocumentId, DocumentKey, FileMeta, JobId, hash_text};
use lmdv_io::SaveOutcome;
use lmdv_io::WriteError;
use lmdv_io::store::StorePatch;
use lmdv_md::{Block, Inline, MdDoc, OBJECT_CHAR};
use lmdv_render::{TaskControlWire, TaskRef};
use rune::{El, Key, KeyEvent, Tx};

use crate::editor::{Prompt, draft_key};
use crate::shared::unix_millis;
use crate::window::{Mode, NoticeAction, NoticeKind, WindowState, title_for_path};

/// How long a task notice keeps its Undo action, in virtual clock seconds. It
/// is longer than a plain status notice because it is the only way back.
pub const TASK_UNDO_SECONDS: f64 = 8.0;

/// One task marker in a top-level block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The item's index inside its own list (the renderer's `TaskRef::item`).
    pub item: usize,
    /// Whether the source marker is checked.
    pub checked: bool,
    /// The item's own text, computed like the renderer's accessible label.
    pub label: String,
    /// The three bytes of `[ ]`/`[x]` in the LF source.
    pub marker: Range<usize>,
}

/// One undoable viewer task write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskUndo {
    /// The document the write belongs to.
    pub doc: DocumentId,
    /// The path that was written.
    pub path: PathBuf,
    /// The marker's source bytes.
    pub range: Range<usize>,
    /// The middle byte before the toggle (` `, `x` or `X`).
    pub before: u8,
    /// The middle byte the write produced.
    pub after: u8,
}

/// The conflict prompt's pending edit: the bytes to write if the user chooses
/// **Overwrite**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEdit {
    /// The document the edit belongs to.
    pub doc: DocumentId,
    /// The file that changed on disk.
    pub path: PathBuf,
    /// The marker's source bytes.
    pub range: Range<usize>,
    /// The middle byte currently in the restored buffer.
    pub before: u8,
    /// The middle byte **Overwrite** writes.
    pub after: u8,
    /// Whether the fresh write should offer Undo again.
    pub offer_undo: bool,
}

/// One save job's context.
#[derive(Debug)]
struct TaskSave {
    doc: DocumentId,
    path: PathBuf,
    range: Range<usize>,
    before: u8,
    after: u8,
    /// BLAKE3 of the exact LF text handed to the save job.
    hash: [u8; 32],
    /// A forward tick offers Undo; an Undo write does not.
    offer_undo: bool,
}

/// Task state owned by one window: the job contexts and their stale guards.
#[derive(Debug, Default)]
pub struct TaskState {
    saves: HashMap<JobId, TaskSave>,
    generations: crate::jobs::GenerationTracker,
}

impl TaskState {
    /// Whether a task save is in flight for `doc`.
    pub fn is_saving(&self, doc: DocumentId) -> bool {
        self.saves.values().any(|save| save.doc == doc)
    }

    /// The number of in-flight task saves (tests and diagnostics).
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.saves.len()
    }

    /// Whether `job` is a task save this window started.
    pub fn is_job(&self, job: JobId) -> bool {
        self.saves.contains_key(&job)
    }
}

/// The one task-control wire a window builds, shared by every article frame.
///
/// The renderer hands the real checkbox, so the box itself takes the pointer
/// press and keeps its role, checked state and label; Space (Rune's focused
/// press) and Enter (the app's key handler) activate it too. No overlay is
/// added, so the element tree holds one checkbox per item (OI-091).
pub fn task_control_wire() -> TaskControlWire {
    Rc::new(|control: El, task: &TaskRef| {
        let pressed = task.clone();
        let keyed = task.clone();
        control
            .key(task_hit_key(task))
            .on_press_tx(move |state: &mut WindowState, _tx: &mut Tx| state.toggle_task(&pressed))
            .on_key(move |state: &mut WindowState, event: &KeyEvent, _tx: &mut Tx| {
                let activate = event.key == Key::Enter || matches!(&event.key, Key::Text(text) if text == " ");
                if activate {
                    state.toggle_task(&keyed);
                }
                activate
            })
    })
}

/// The stable key of a task's control.
///
/// The label's hash keeps nested lists unique when two lists in one block both
/// have an item at the same index (the renderer's `TaskRef::item` is list-local
/// and the wire never sees the list path).
pub fn task_hit_key(task: &TaskRef) -> String {
    format!("task-hit-{}-{}-{:08x}", task.block, task.item, label_hash(&task.label))
}

fn label_hash(label: &str) -> u32 {
    // FNV-1a: deterministic across processes and frames, unlike a RandomState.
    let mut hash = 0x811c_9dc5_u32;
    for byte in label.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Locates the source marker a [`TaskRef`] names, unambiguously.
///
/// The renderer fills [`TaskRef::source`] from the inline source map, so a
/// nested list can repeat `item` without ambiguity (OI-092). Only a ref
/// without a source range falls back to matching the list index, label and
/// checked state; two candidates that agree on all of them are reported as
/// ambiguous (`None`) and nothing is written.
pub fn locate_marker(doc: &MdDoc, task: &TaskRef) -> Option<Range<usize>> {
    if marker_middle(doc.source.as_ref(), &task.source).is_some() {
        return Some(task.source.clone());
    }
    let candidates = candidates_for_block(doc, &task.block);
    pick(&candidates, task)
}

/// Every task item in the top-level block keyed `block`, in document order.
pub fn candidates_for_block(doc: &MdDoc, block: &str) -> Vec<Candidate> {
    let Some(index) = doc.block_spans.iter().position(|span| span.key == block) else { return Vec::new() };
    let mut candidates = Vec::new();
    match &doc.blocks[index] {
        Block::List { .. } | Block::Quote { .. } => collect_tasks(&doc.blocks[index], &mut candidates),
        Block::FootnoteDefs => {
            for footnote in &doc.footnotes {
                for body in &footnote.blocks {
                    collect_tasks(body, &mut candidates);
                }
            }
        }
        _ => {}
    }
    candidates
}

fn pick(candidates: &[Candidate], task: &TaskRef) -> Option<Range<usize>> {
    let exact: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate.item == task.item && candidate.label == task.label && candidate.checked == task.checked
        })
        .collect();
    if let [one] = exact.as_slice() {
        return Some(one.marker.clone());
    }
    // A quick second click before the reparse lands sees a stale `checked`; the
    // label and list index still identify the one item.
    let same_label: Vec<&Candidate> =
        candidates.iter().filter(|candidate| candidate.item == task.item && candidate.label == task.label).collect();
    match same_label.as_slice() {
        [one] => Some(one.marker.clone()),
        _ => None,
    }
}

/// Collects every task item under `block`, nested lists and quotes included.
fn collect_tasks(block: &Block, out: &mut Vec<Candidate>) {
    match block {
        Block::List { items, .. } => {
            for (item, blocks) in items.iter().enumerate() {
                if let Some(candidate) = candidate(item, blocks) {
                    out.push(candidate);
                }
                for child in blocks {
                    collect_tasks(child, out);
                }
            }
        }
        Block::Quote { blocks, .. } => {
            for child in blocks {
                collect_tasks(child, out);
            }
        }
        _ => {}
    }
}

/// One list item's task marker, when its first block is a task paragraph.
fn candidate(item: usize, blocks: &[Block]) -> Option<Candidate> {
    let Some(Block::Para(inline)) = blocks.first() else { return None };
    let (checked, marker) = marker_in(inline)?;
    Some(Candidate { item, checked, label: item_label(blocks), marker })
}

/// The first task checkbox object of an inline run and its source marker.
fn marker_in(inline: &Inline) -> Option<(bool, Range<usize>)> {
    let (checked, rendered) = inline.objects.iter().find_map(|object| match object.kind {
        lmdv_md::InlineObjectKind::TaskCheckbox { checked } => Some((checked, object.range.start)),
        _ => None,
    })?;
    let start = inline.source_map.source_of(rendered)?;
    let marker = start..start.checked_add(3)?;
    Some((checked, marker))
}

/// The item's own text, exactly as `lmdv-render` builds the accessible label:
/// formulas and other objects are excluded, sub-lists are not the item's own
/// text, and the result is capped at 120 characters.
fn item_label(blocks: &[Block]) -> String {
    let mut text = String::new();
    for block in blocks {
        match block {
            Block::Para(inline) | Block::Heading { inline, .. } => {
                text.extend(inline.text.chars().filter(|ch| *ch != OBJECT_CHAR));
                text.push(' ');
            }
            Block::Code { text: code, .. } => {
                text.push_str(code);
                text.push(' ');
            }
            _ => {}
        }
    }
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.chars().count() > 120 {
        let cut: String = out.chars().take(119).collect();
        format!("{}…", cut.trim_end())
    } else {
        out
    }
}

/// The marker's middle byte at `range`, when the three bytes really are a task
/// marker in `text`.
fn marker_middle(text: &str, range: &Range<usize>) -> Option<u8> {
    let bytes = text.as_bytes();
    let marker = bytes.get(range.clone())?;
    if marker.len() != 3 || marker[0] != b'[' || marker[2] != b']' || !matches!(marker[1], b' ' | b'x' | b'X') {
        return None;
    }
    Some(marker[1])
}

/// Flips one marker byte in the document; `false` when the marker no longer
/// matches the bytes the caller expected.
fn set_middle(document: &Document, range: &Range<usize>, expected: u8, to: u8) -> bool {
    let current = {
        let buffer = document.text();
        marker_middle(buffer.text(), range)
    };
    if current != Some(expected) {
        return false;
    }
    let byte = range.start + 1;
    document.apply(byte..byte + 1, if to == b' ' { " " } else { "x" });
    true
}

impl WindowState {
    /// The shared task-control wire for this window's articles.
    pub(crate) fn task_control_wire(&self) -> TaskControlWire {
        self.cached_task_control_wire().clone()
    }

    /// Handles a pointer press or Space/Enter activation on a task checkbox in
    /// the reader (M-07).
    pub(crate) fn toggle_task(&mut self, task: &TaskRef) {
        let Some(tab) = self.active_tab() else { return };
        // Defence in depth: the editor never passes the wire, and the check is
        // repeated here so a stale frame cannot write from edit mode.
        if tab.view.mode != Mode::Viewer {
            return;
        }
        let Some(document) = tab.document().cloned() else { return };
        if !matches!(*document.status.borrow(), DocStatus::Ready) {
            return;
        }
        let Some(model) = tab.view.reader.clone() else {
            self.notice(lmdv_i18n::t!("task.notReady"));
            return;
        };
        // A reload replaces the buffer and clears the revision; a tick against
        // the old offsets could land in the wrong line.
        if tab.view.reader_revision.is_none() {
            self.notice(lmdv_i18n::t!("task.notReady"));
            return;
        }
        if self.tasks.is_saving(document.id) {
            self.notice(lmdv_i18n::t!("task.saving"));
            return;
        }
        let Some(marker) = locate_marker(&model, task) else {
            self.notice(lmdv_i18n::t!("task.ambiguous"));
            return;
        };
        let (middle, expected, path) = {
            let buffer = document.text();
            let Some(middle) = marker_middle(buffer.text(), &marker) else {
                drop(buffer);
                self.notice(lmdv_i18n::t!("task.changedOnDisk"));
                return;
            };
            (middle, document.saved.borrow().meta, document.key.borrow().path().map(Path::to_path_buf))
        };
        let dirty = document.is_dirty();
        let leased = document.lease_holder().is_some();
        let after = if middle == b' ' { b'x' } else { b' ' };

        if !set_middle(&document, &marker, middle, after) {
            self.notice(lmdv_i18n::t!("task.changedWhileClicked"));
            return;
        }
        self.sync_tab_dirty(&document);

        // A document with unsaved edits (another window's editor) keeps the
        // tick in the buffer and lets that save write it; a pasted document has
        // no file at all. Neither may start a viewer write.
        if dirty || leased || path.is_none() {
            self.show_task_notice(lmdv_i18n::t!("task.bufferChanged"), None, NoticeKind::Info);
            return;
        }
        let path = path.expect("the path was checked above");
        self.start_task_save(&document, path, marker, middle, after, Some(expected), true);
    }

    /// Starts a task save and remembers the exact text it handed to the job.
    #[allow(clippy::too_many_arguments)]
    fn start_task_save(
        &mut self,
        document: &Rc<Document>,
        path: PathBuf,
        range: Range<usize>,
        before: u8,
        after: u8,
        expected: Option<FileMeta>,
        offer_undo: bool,
    ) {
        let (text, eol) = {
            let buffer = document.text();
            (buffer.text().to_owned(), buffer.eol())
        };
        let hash = hash_text(&text);
        let generation = self.tasks.generations.begin(document.id);
        let job = self.shared.jobs().save(
            self.worker().clone(),
            document.id,
            generation,
            path.clone(),
            text,
            eol,
            expected,
            false,
        );
        self.tasks.saves.insert(job, TaskSave { doc: document.id, path, range, before, after, hash, offer_undo });
        self.sync_tab_dirty(document);
    }

    /// Runs the action a notice offers and dismisses it. A second tick replaces
    /// the notice, so only the latest toggle can be undone.
    pub(crate) fn run_notice_action(&mut self) {
        let Some(action) = self.ui.notice.as_ref().and_then(|notice| notice.action.clone()) else { return };
        self.dismiss_notice();
        match action {
            NoticeAction::TaskUndo(undo) => self.undo_task(undo),
            NoticeAction::ShowInFolder(path) => {
                self.shared.open_external(self.worker().clone(), path);
            }
            NoticeAction::OpenFile(path) => {
                // L-07: the desktop's own handler opens a whitelisted
                // document, media file or archive. A script never reaches here.
                self.shared.open_external(self.worker().clone(), path);
            }
            // L-09: poll has the frame's `Tx` that a native dialog needs.
            NoticeAction::SaveAs(doc) => self.ui.save_as_requested = Some(doc),
            NoticeAction::UsePrintDialog => self.print_document(),
        }
    }

    /// Reverses the toggle the notice's Undo action names.
    pub(crate) fn undo_task(&mut self, undo: TaskUndo) {
        let Some(document) = self.shared.docs().get(undo.doc) else { return };
        if document.key.borrow().path() != Some(undo.path.as_path()) {
            self.notice(lmdv_i18n::t!("msg.undoFailed", "err" => lmdv_i18n::t!("task.undoMoved")));
            return;
        }
        if self.tasks.is_saving(document.id) {
            self.notice(lmdv_i18n::t!("task.saving"));
            return;
        }
        let was_dirty = document.is_dirty();
        let leased = document.lease_holder().is_some();
        let current = {
            let buffer = document.text();
            marker_middle(buffer.text(), &undo.range)
        };
        if current != Some(undo.after) {
            self.notice(lmdv_i18n::t!("msg.undoFailed", "err" => lmdv_i18n::t!("task.undoChanged")));
            return;
        }
        if !set_middle(&document, &undo.range, undo.after, undo.before) {
            self.notice(lmdv_i18n::t!("msg.undoFailed", "err" => lmdv_i18n::t!("task.undoChanged")));
            return;
        }
        self.sync_tab_dirty(&document);
        if was_dirty || leased {
            self.show_task_notice(lmdv_i18n::t!("task.bufferUndone"), None, NoticeKind::Info);
            return;
        }
        let expected = document.saved.borrow().meta;
        self.start_task_save(&document, undo.path, undo.range, undo.after, undo.before, Some(expected), false);
    }

    /// Handles a task save completion. `None` means the job was not a task save.
    pub(crate) fn handle_task_saved(
        &mut self,
        job: JobId,
        doc: DocumentId,
        generation: u64,
        result: Result<SaveOutcome, WriteError>,
    ) -> Option<bool> {
        if !self.tasks.saves.contains_key(&job) {
            return None;
        }
        let Some(save) = self.tasks.saves.remove(&job) else { return Some(false) };
        if save.doc != doc || !self.tasks.generations.finish(save.doc, generation) {
            return Some(false);
        }
        let Some(document) = self.shared.docs().get(save.doc) else { return Some(false) };
        match result {
            Ok(outcome) => Some(self.task_saved(save, &document, outcome)),
            Err(error) => Some(self.task_save_failed(save, &document, error)),
        }
    }

    fn task_saved(&mut self, save: TaskSave, document: &Rc<Document>, outcome: SaveOutcome) -> bool {
        let old_path = document.key.borrow().path().map(Path::to_path_buf);
        let new_path = outcome.path.clone();
        if old_path.as_deref() != Some(new_path.as_path()) {
            self.shared.docs_mut().rekey(document.id, DocumentKey::File(new_path.clone()));
            *document.title.borrow_mut() = title_for_path(&new_path);
            self.shared.watch_path(&new_path);
            if let Some(old) = &old_path {
                self.shared.unwatch_path(old);
                self.shared.rename_position(old, &new_path);
                self.shared.rename_recent(old, &new_path);
                self.rename_history_paths(old, &new_path);
            }
        }
        document.set_saved(outcome.meta, save.hash, new_path.clone());
        *document.status.borrow_mut() = DocStatus::Ready;
        self.shared.docs_mut().insert_ready(document.clone());
        document.ignore_writes_until.set(Some(Instant::now() + crate::editor::OWN_WRITE_SUPPRESS));
        document.last_local_write.set(Some(outcome.meta));
        self.shared.record_recent(new_path.clone(), unix_millis());
        self.shared.mark_draft_checked(document.id);
        if let Some(key) = draft_key(document) {
            self.shared.queue_store(StorePatch::RemoveDraft(key));
        }
        let dirty = document.is_dirty();
        for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(document.id)) {
            tab.refresh_title();
            tab.view.dirty = dirty;
            tab.view.disk_changed = false;
            if !dirty {
                tab.view.draft_pending = false;
                tab.view.draft_due = None;
                tab.view.draft_written = false;
            }
        }
        self.sync_editor_titles();
        if save.offer_undo {
            let undo = TaskUndo {
                doc: document.id,
                path: new_path,
                range: save.range,
                before: save.before,
                after: save.after,
            };
            let checked = save.after != b' ';
            let text = if checked { lmdv_i18n::t!("msg.taskChecked") } else { lmdv_i18n::t!("msg.taskUnchecked") };
            self.show_task_notice(text, Some(undo), NoticeKind::Success);
        } else {
            self.dismiss_notice();
        }
        // The buffer moved, so the next poll reparses the article and draws the
        // new checked state (`drive_parse` keys on the revision).
        true
    }

    fn task_save_failed(&mut self, save: TaskSave, document: &Rc<Document>, error: WriteError) -> bool {
        // A failed write never leaves the viewer's optimistic tick behind.
        set_middle(document, &save.range, save.after, save.before);
        self.sync_tab_dirty(document);
        match error {
            WriteError::Conflict => {
                let edit = TaskEdit {
                    doc: document.id,
                    path: save.path,
                    range: save.range,
                    before: save.before,
                    after: save.after,
                    offer_undo: save.offer_undo,
                };
                self.dismiss_notice();
                self.editor.prompt = Some(Prompt::TaskConflict { edit });
            }
            WriteError::Deleted(_) => {
                document.mark_gone();
                self.notice(lmdv_i18n::t!("msg.taskUpdateFailed", "err" => lmdv_i18n::t!("task.fileDeleted")));
            }
            error => {
                self.notice(lmdv_i18n::t!("msg.taskUpdateFailed", "err" => error));
            }
        }
        true
    }

    /// **Overwrite** in the task conflict prompt: re-apply the edit and write
    /// without the metadata guard.
    pub(crate) fn resolve_task_conflict(&mut self, edit: TaskEdit) {
        let Some(document) = self.shared.docs().get(edit.doc) else { return };
        if document.key.borrow().path() != Some(edit.path.as_path()) {
            self.notice(lmdv_i18n::t!("task.moved"));
            return;
        }
        if self.tasks.is_saving(document.id) {
            self.notice(lmdv_i18n::t!("task.saving"));
            return;
        }
        if !set_middle(&document, &edit.range, edit.before, edit.after) {
            self.notice(lmdv_i18n::t!("task.changedAgain"));
            return;
        }
        self.sync_tab_dirty(&document);
        self.start_task_save(&document, edit.path, edit.range, edit.before, edit.after, None, edit.offer_undo);
    }

    /// **Keep theirs** in the task conflict prompt: load the disk version.
    pub(crate) fn keep_task_theirs(&mut self, edit: TaskEdit) {
        let Some(document) = self.shared.docs().get(edit.doc) else { return };
        if document.key.borrow().path() != Some(edit.path.as_path()) {
            return;
        }
        self.force_reload(edit.doc, edit.path);
        self.notice(lmdv_i18n::t!("task.keptTheirs"));
    }

    fn show_task_notice(&mut self, text: impl Into<String>, undo: Option<TaskUndo>, kind: NoticeKind) {
        let action = undo.map(NoticeAction::TaskUndo);
        self.show_notice(text.into(), kind, action, TASK_UNDO_SECONDS);
    }
}
