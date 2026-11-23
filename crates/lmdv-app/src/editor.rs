//! Edit mode: the editor session, live preview, saving, conflicts, drafts, the
//! file watcher's decisions and the close guards (E-01–E-19, E-39, E-40).
//!
//! Everything here is UI state and job orchestration. Reading and writing the
//! disk happen in `crate::jobs`; this module only decides what to ask for and
//! what a completion means. The canonical text stays in the shared
//! [`Document`](lmdv_core::Document); the [`EditorSession`] is the editor's view
//! of it and supplies selection, undo, line ending and draft text.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lmdv_core::{DocStatus, Document, DocumentId, DocumentKey, Draft, EditLease, FileMeta, TabId, hash_text};
use lmdv_edit::EditorSession;
use lmdv_edit::spell::{Availability, compose_underlines};
use lmdv_io::store::StorePatch;
use lmdv_io::{ReadError, WriteError};
use rune::{FileDialogOptions, Key, KeyEvent, Span, TextEdit, Tx};
use rune_kit::syntax::SyntaxTheme;

use crate::commands::MARKDOWN_EXTENSIONS;
use crate::jobs::{JobMsg, SpellMailbox, SpellMsg, WorkerMailbox};
use crate::reader::ScrollSnapshot;
use crate::shared::{SpellDiscovery, UiMsg};
use crate::window::{Mode, Tab, WindowState, title_for_path};
use crate::windows::{CloseDoc, close_title};

pub(crate) mod scroll_sync;

use scroll_sync::{ENTRY_SUPPRESS, EditorTop, PreviewGeometry, PreviewSync, VisualCache};

/// How long after the last keystroke the preview parses (E-05).
pub const PREVIEW_DEBOUNCE: f64 = 0.2;
/// How long after the last keystroke the crash draft is written (E-16).
pub const DRAFT_DEBOUNCE: f64 = 1.0;
/// How long after the last keystroke the source is spellchecked (E-SPELL).
pub const SPELL_DEBOUNCE: f64 = 0.30;
/// Before the source view reports its visible rows, this many blocks are
/// checked in block mode (the first screenful, capped).
const SPELL_BLOCK_FALLBACK: usize = 40;
/// How long the saved check stays visible (E-08).
pub const SAVED_FLASH_SECONDS: f64 = 1.4;
/// Watcher events within this window after our own save are ignored.
pub const OWN_WRITE_SUPPRESS: Duration = Duration::from_millis(1500);
/// After Escape leaves the source field, Tab within this window leaves the
/// editor instead of indenting (E-33).
pub const ESCAPE_TAB_WINDOW: Duration = Duration::from_secs(2);
/// Below this shell width only the source is shown (E-02).
pub const NARROW_SPLIT_WIDTH: f32 = 760.0;
/// The source pane's share of the split at the start.
pub const SPLIT_DEFAULT: f32 = 0.5;
/// The divider the drag can move the split inside.
pub const SPLIT_MIN: f32 = 0.2;
pub const SPLIT_MAX: f32 = 0.8;

/// What to do once a save completes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AfterSave {
    /// Just save.
    #[default]
    Dismiss,
    /// Leave edit mode and return to the reading view.
    LeaveEditor,
    /// Close this tab.
    CloseTab(TabId),
    /// Close the window when every dirty document is saved.
    CloseWindow,
    /// Save every dirty document in every window, then quit (aggregate quit).
    Quit,
}

/// A decision the user must make. Dialogs are rendered from this value.
#[derive(Clone, Debug, PartialEq)]
pub enum Prompt {
    /// `Ctrl+E` with unsaved edits (E-13).
    DiscardEdit { doc: DocumentId },
    /// Closing a dirty tab (E-08/T-05).
    CloseTab { tab: TabId, doc: DocumentId },
    /// Closing the window with dirty documents.
    CloseWindow,
    /// The file changed on disk while the buffer is dirty (E-10).
    SaveConflict { doc: DocumentId, after: AfterSave },
    /// A viewer task tick's file changed on disk before its save landed.
    TaskConflict { edit: crate::tasks::TaskEdit },
    /// The file was deleted while open (E-10).
    FileDeleted { doc: DocumentId, after: AfterSave },
    /// The watcher saw a change on disk for a dirty buffer.
    DiskChanged { doc: DocumentId },
    /// A crash draft differs from the file (E-17).
    Draft { doc: DocumentId, key: String, draft: Draft },
    /// Another window holds the edit lease (E-01, T-SHARED).
    LeaseHeld { holder: EditLease },
    /// Quit with unsaved documents in one or more windows (P4-02a).
    Quit,
}

impl Prompt {
    /// The dialog's title.
    pub fn title(&self) -> &'static str {
        match self {
            Self::DiscardEdit { .. } => lmdv_i18n::t!("msg.discardTitle"),
            Self::CloseTab { .. } => lmdv_i18n::t!("prompt.closeTabTitle"),
            Self::CloseWindow => lmdv_i18n::t!("prompt.closeWindowTitle"),
            Self::SaveConflict { .. } => lmdv_i18n::t!("msg.conflictTitle"),
            Self::TaskConflict { .. } => lmdv_i18n::t!("msg.conflictTitle"),
            Self::FileDeleted { .. } => lmdv_i18n::t!("prompt.deletedTitle"),
            Self::DiskChanged { .. } => lmdv_i18n::t!("msg.conflictTitle"),
            Self::Draft { .. } => lmdv_i18n::t!("prompt.draftTitle"),
            Self::LeaseHeld { .. } => lmdv_i18n::t!("prompt.leaseTitle"),
            Self::Quit => lmdv_i18n::t!("prompt.quitTitle"),
        }
    }
}

/// The aggregate quit's documents and the one-at-a-time save cursor.
///
/// The initiating window names every document in one dialog, then saves them
/// in turn. Each id appears once, so a document shown in two windows is saved
/// once. A failed or abandoned save clears the guard and keeps the app open.
#[derive(Debug, Default)]
pub struct QuitGuard {
    /// Every document the question is about, deduplicated by id.
    pub docs: Vec<DocumentId>,
    /// The dialog names, in the same order as `docs`.
    pub names: Vec<String>,
    /// Documents still waiting for their save.
    pub queue: VecDeque<DocumentId>,
    /// Saves are running; the quit follows when the queue empties.
    pub saving: bool,
}

impl QuitGuard {
    /// Forgets the plan and stops treating the next save as part of a quit.
    fn clear(&mut self) {
        self.docs.clear();
        self.names.clear();
        self.queue.clear();
        self.saving = false;
    }
}

/// A Save as whose chosen path is being checked before anything is written.
///
/// The probe answers with the file's metadata, so E-12's duplicate is found by
/// identity — a symlink, a hard link or a different spelling of the same file
/// is the same document.
#[derive(Clone, Debug)]
pub struct PendingSaveAs {
    /// The path the dialog returned.
    pub path: PathBuf,
    /// What to do once the write lands.
    pub after: AfterSave,
}

/// Save as' path probes, one at a time per document.
///
/// The answer is a metadata job on a worker: a handler never stats a file.
/// It answers into this own mailbox because the window's job dispatcher drops
/// [`JobMsg::Probed`] today — that route is reserved for the watcher's reload.
#[derive(Default)]
pub struct SaveAsState {
    /// Chosen paths waiting for their probe, by document.
    pub pending: HashMap<DocumentId, PendingSaveAs>,
    /// One probe generation per document; a newer Save as supersedes an older.
    pub probes: crate::jobs::GenerationTracker,
    /// Where [`crate::jobs::JobHub::probe`] answers, built on first use.
    mailbox: Option<Arc<WorkerMailbox>>,
}

/// The last spelling result for one tab, in whole-source byte coordinates.
#[derive(Clone, Debug)]
pub struct SpellCheck {
    /// The editor revision the ranges belong to; an edit invalidates them.
    pub revision: u64,
    /// The source range that was checked.
    pub window: Range<usize>,
    /// The whole buffer was checked (the single field), so every range applies.
    pub whole: bool,
    /// The misspelled words, in text order.
    pub ranges: Vec<Range<usize>>,
}

/// Per-window spellcheck state: the worker mailbox, the debounce timer and the
/// last answer per tab. The dictionary itself lives in [`crate::shared`] once.
#[derive(Default)]
pub struct SpellState {
    mailbox: Option<Arc<SpellMailbox>>,
    checks: HashMap<TabId, SpellCheck>,
    /// The debounce due time.
    due: Option<f64>,
    /// The check on the worker, so its answer can be matched.
    in_flight: Option<lmdv_core::JobId>,
    in_flight_tab: Option<TabId>,
    /// The (tab, revision) the debounce was last armed for.
    seen: Option<(TabId, u64)>,
}

impl SpellState {
    fn check_for(&self, tab: TabId) -> Option<&SpellCheck> {
        self.checks.get(&tab)
    }

    /// Forgets the ranges and the timer, for a new or left editor session.
    pub(crate) fn reset(&mut self) {
        self.checks.clear();
        self.due = None;
        self.seen = None;
    }

    /// Whether a check is running for `tab`.
    #[cfg(test)]
    pub(crate) fn is_checking(&self, tab: TabId) -> bool {
        self.in_flight.is_some() && self.in_flight_tab == Some(tab)
    }

    /// The ranges the last check left for `tab`, for tests.
    #[cfg(test)]
    pub(crate) fn ranges(&self, tab: TabId) -> Option<&[Range<usize>]> {
        self.check_for(tab).map(|check| check.ranges.as_slice())
    }
}

/// What a reload does with the text the worker reads back (R-21/R-22).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadIntent {
    /// Replace the buffer and reparse (an explicit reload, a conflict reload).
    Replace,
    /// Compare first: an unchanged read only updates the saved baseline, so
    /// an mtime-only `touch` changes nothing on screen.
    Compare,
    /// E-15's same-file reopen: an unchanged read updates the baseline, a
    /// different read reloads a clean view with its anchor restored and is only
    /// announced for a dirty buffer, which is never read over.
    ReopenCheck,
}

/// Editor-side window state. `scratch` keeps the `on_edit` accessor total when
/// no tab is in edit mode.
pub struct EditorState {
    pub scratch: TextEdit,
    /// Source pane share of the split (`0.2..=0.8`).
    pub split: f32,
    /// The live preview is on (hidden automatically below 760 pt).
    pub preview_visible: bool,
    /// Below 760 pt the user asked for the preview on its own.
    pub preview_narrow: bool,
    /// The open decision dialog.
    pub prompt: Option<Prompt>,
    /// Work to finish after a save, keyed by document.
    pub after_save: HashMap<DocumentId, AfterSave>,
    /// The exact text handed to the save job, so a save that lands after more
    /// typing records the right base hash (E-08).
    pub save_texts: HashMap<DocumentId, String>,
    /// A save generation per document, so a stale completion is ignored.
    pub saves: crate::jobs::GenerationTracker,
    /// Save as paths waiting for their collision probe (E-12).
    pub save_as: SaveAsState,
    /// Every dirty document must save before the window closes.
    pub close_on_clean: bool,
    /// Documents whose CloseWindow save, or Save As dialog, is still pending.
    pub close_pending: BTreeSet<DocumentId>,
    /// The aggregate quit's documents and save cursor (P4-02a).
    pub quit: QuitGuard,
    /// The last watcher sequence this window consumed.
    pub watch_seq: u64,
    /// The saved check's expiry on the virtual clock.
    pub saved_flash_until: Option<f64>,
    /// The expiry wake has been requested.
    pub saved_flash_armed: bool,
    /// Reading panes to restore when no tab is editing: the editing tab and
    /// (browser, outline, outline_hidden). Edit mode hides them (E-02).
    pub saved_panes: Option<(TabId, bool, bool, bool)>,
    /// The go-to-line prompt (E-31), window-level and transient.
    pub goto_line: Option<crate::edit_ui::GotoLine>,
    /// The format bar's open Insert/Tools menu (E-22), window-level and
    /// transient: a tab switch or leaving edit mode closes it.
    pub format_menu: Option<crate::edit_ui::FormatMenu>,
    /// The journal kind the next block-field change belongs to (typing,
    /// deleting or a discrete paste), set by the field's key handler.
    pub block_kind: lmdv_edit::BlockEditKind,
    /// When Escape last left the source field, for the Esc-then-Tab leave
    /// gesture (E-33). Cleared once the Tab is handled or leaves the window.
    pub last_escape_at: Option<Instant>,
    /// The source viewport's latest report, shared with its `on_scroll_offset`
    /// (E-07).
    pub scroll: Rc<Cell<ScrollSnapshot>>,
    /// Where the source's first visible line sits, measured by the view.
    pub editor_top: Rc<Cell<EditorTop>>,
    /// The visible source field's exact visual lines (E-07), measured by the
    /// view with the field's own font, spans and wrap width.
    pub visual: RefCell<VisualCache>,
    /// The preview blocks around the editor's top and caret, measured by the
    /// reader view.
    pub preview: RefCell<PreviewGeometry>,
    /// The virtual rows the block-mode source is showing.
    pub visible_blocks: Option<Range<usize>>,
    /// The last source offset `poll` synced from.
    pub last_offset: Option<f32>,
    /// The line the caret was on at the last sync.
    pub last_caret: Option<usize>,
    /// The last editor→preview mapping, for tests and evidence.
    pub last_sync: Option<PreviewSync>,
    /// A parse completion asks for a caret reveal.
    pub reveal_pending: bool,
    /// Editor scrolls until this clock time keep revealing the caret block
    /// (the original's 300 ms `revealUntil`).
    pub reveal_until: f64,
    /// Scroll reports are ignored until this instant after entering edit mode.
    pub sync_suppressed_until: Option<Instant>,
    /// The Linux spellchecker's debounce, worker and last ranges (ADR-008).
    pub spell: SpellState,
    /// The failed preview parse already announced for a tab (E-05), so one
    /// failure shows one notice and a good parse re-arms it.
    pub preview_error: Option<(TabId, String)>,
    /// Watcher reloads whose read is compared with the buffer before it
    /// replaces anything (R-21). A quiet reload whose text is unchanged only
    /// updates the saved baseline.
    pub compare_reloads: BTreeSet<DocumentId>,
    /// E-15's reopen checks whose read is compared before it can touch a
    /// buffer. Disjoint from `compare_reloads`.
    pub reopen_checks: BTreeSet<DocumentId>,
    /// E-15(c): the tab whose source viewport must reveal the line a `--line`
    /// or a search hit named, once that tab is in front.
    pub line_reveal: Option<TabId>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            scratch: TextEdit::default(),
            split: SPLIT_DEFAULT,
            preview_visible: true,
            preview_narrow: false,
            prompt: None,
            after_save: HashMap::new(),
            save_texts: HashMap::new(),
            saves: crate::jobs::GenerationTracker::default(),
            save_as: SaveAsState::default(),
            close_on_clean: false,
            close_pending: BTreeSet::new(),
            quit: QuitGuard::default(),
            watch_seq: 0,
            saved_flash_until: None,
            saved_flash_armed: false,
            saved_panes: None,
            goto_line: None,
            format_menu: None,
            block_kind: lmdv_edit::BlockEditKind::Typing,
            last_escape_at: None,
            scroll: Rc::new(Cell::new(ScrollSnapshot::default())),
            editor_top: Rc::new(Cell::new(EditorTop::default())),
            visual: RefCell::new(VisualCache::default()),
            preview: RefCell::new(PreviewGeometry::default()),
            visible_blocks: None,
            last_offset: None,
            last_caret: None,
            last_sync: None,
            reveal_pending: false,
            reveal_until: 0.0,
            sync_suppressed_until: None,
            spell: SpellState::default(),
            preview_error: None,
            compare_reloads: BTreeSet::new(),
            reopen_checks: BTreeSet::new(),
            line_reveal: None,
        }
    }
}

impl WindowState {
    /// Whether the active document can enter edit mode.
    pub fn editor_available(&self) -> bool {
        self.active_tab().is_some_and(|tab| tab.document().is_some_and(|doc| is_ready(doc)))
    }

    /// The split's drag state, for the divider.
    pub fn editor_split(&self) -> f32 {
        self.editor.split.clamp(SPLIT_MIN, SPLIT_MAX)
    }

    /// Sets the split after a divider drag.
    pub fn set_editor_split(&mut self, split: f32) {
        if split.is_finite() {
            self.editor.split = split.clamp(SPLIT_MIN, SPLIT_MAX);
        }
    }

    /// Shows or hides the live preview (E-02's narrow-window toggle).
    pub fn toggle_preview(&mut self) {
        self.editor.preview_visible = !self.editor.preview_visible;
    }

    /// Below 760 pt: show the preview in place of the source (E-02).
    pub fn show_narrow_preview(&mut self) {
        self.editor.preview_narrow = true;
        self.editor.preview_visible = true;
    }

    /// Below 760 pt: return to the source.
    pub fn hide_narrow_preview(&mut self) {
        self.editor.preview_narrow = false;
    }

    /// The `on_edit` accessor: the active editor's field, or a harmless scratch.
    ///
    /// In block mode this is the active block's field; the caret at the top is
    /// the block the session focused on entry or last moved to.
    pub(crate) fn active_editor_edit(&mut self) -> &mut TextEdit {
        let active = self.active;
        let tabs = &mut self.tabs;
        if let Some(tab) = tabs.get_mut(active)
            && let Some(editor) = tab.view.editor.as_mut()
        {
            if editor.block_mode_active() {
                let index = editor.active_block().unwrap_or(0);
                editor.set_active_block(Some(index));
                if let Some(field) = editor.block_editor_mut().and_then(|blocks| blocks.block_mut(index)) {
                    return field;
                }
            } else {
                return &mut editor.edit;
            }
        }
        &mut self.editor.scratch
    }

    /// `Ctrl+E`: edit, or return to reading (E-01/E-13).
    pub(crate) fn toggle_editor_mode(&mut self, tx: &mut Tx) {
        if self.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor) {
            self.request_leave_editor(tx);
        } else {
            self.enter_editor(None, tx);
        }
    }

    /// Enters edit mode for the active document, optionally from draft text.
    pub(crate) fn enter_editor(&mut self, text_override: Option<String>, tx: &mut Tx) -> bool {
        self.editor.last_escape_at = None;
        self.ui.block_menu = None;
        self.section_line.reset();
        // Editing is not reading: the j/k target, the `g g` window and the
        // pinned heading do not belong to the source field (R-06–R-09).
        self.reading_keys.reset();
        self.reading.landing = None;
        self.reading.landing_flash = None;
        let Some(tab) = self.active_tab() else { return false };
        let Some(doc) = tab.document().cloned() else {
            self.notice(lmdv_i18n::t!("msg.openDocFirst"));
            return false;
        };
        if tab.view.editor.is_some() {
            self.focus_editor_field(tx);
            return true;
        }
        let from_draft = text_override.is_some();
        // OI-073: a crash draft of a deleted or unreadable file is the only
        // copy of its text. Restore it even though the file itself could not
        // be read; the document becomes a normal dirty buffer and the first
        // save offers Save again.
        let recovering = from_draft && matches!(*doc.status.borrow(), DocStatus::Error(_) | DocStatus::Gone);
        if !is_ready(&doc) && !recovering {
            self.notice(lmdv_i18n::t!("msg.docLoading"));
            return false;
        }
        let tab_id = tab.id;
        self.reset_scroll_sync();
        if let Err(error) = doc.acquire_edit(self.id(), tab_id) {
            self.editor.prompt = Some(Prompt::LeaseHeld { holder: error.holder });
            return false;
        }
        let base = doc.snapshot().to_string();
        let eol = doc.text().eol();
        let text = text_override.unwrap_or_else(|| base.clone());
        let previous_find =
            self.active_tab().map(|tab| (tab.view.find_bar.query.text.clone(), tab.view.find_bar.replace.text.clone()));
        let mut session = EditorSession::new(&text, eol);
        // A new session has its own revisions; stale ranges must not paint.
        self.editor.spell.reset();
        if let Some((query, replace)) = previous_find {
            session.find.query = query;
            session.find.replace = replace;
        }
        session.set_doc_revision(doc.revision());
        session.set_base_revision(doc.revision());
        if !from_draft {
            session.edit.set_caret(self.reading_caret(&doc).unwrap_or(0));
        }
        // The layout is chosen once, here: `Block` always, `Auto` above the
        // measured threshold. It never flips while this session is open.
        crate::block_ui::apply_large_file_mode(self.shared().prefs().editor.large_file_mode, &mut session);
        crate::block_ui::prime_editor_focus(&mut session);
        let block_focus = session.block_mode_active().then(|| session.active_block()).flatten();
        let revision = if text != base { doc.replace_all(&text, eol) } else { doc.revision() };
        if recovering {
            // The draft now is the document: the reader and the editor draw it
            // as a normal dirty buffer, and the failed load is explained by the
            // dirty marker and the next save's Save again.
            *doc.status.borrow_mut() = DocStatus::Ready;
        }
        session.set_doc_revision(revision);
        let dirty = doc.is_dirty();
        if let Some(tab) = self.active_tab_mut() {
            tab.view.mode = Mode::Editor;
            tab.view.reader_find.close();
            tab.view.editor = Some(session);
            tab.view.edit_base = Some(base);
            tab.view.dirty = dirty;
            tab.view.preview_pending = false;
            tab.view.preview_due = None;
            tab.view.draft_pending = dirty;
            tab.view.draft_due = None;
            tab.view.disk_changed = false;
        }
        self.editor.prompt = None;
        self.reading.pending = None;
        self.sync_editor_panes();
        self.sync_editor_titles();
        match block_focus {
            Some(index) => self.focus_block(index, tx),
            None => tx.focus("editor-source", false),
        }
        // The field's first scroll settling must not move the preview off the
        // reading position (E-03/E-07).
        self.suppress_scroll_sync();
        true
    }

    /// Ignores source scroll settling for the entry window (E-07), used after
    /// entering edit mode and when an editing tab comes back.
    pub(crate) fn suppress_scroll_sync(&mut self) {
        self.editor.sync_suppressed_until = Some(Instant::now() + ENTRY_SUPPRESS);
    }

    /// Clears the editor→preview sync bookkeeping for a new edit session or a
    /// tab switch (the scroll cell belongs to the window, not the tab).
    pub(crate) fn reset_scroll_sync(&mut self) {
        self.editor.scroll.set(ScrollSnapshot::default());
        self.editor.editor_top.set(EditorTop::default());
        self.editor.visual.borrow_mut().clear();
        *self.editor.preview.borrow_mut() = PreviewGeometry::default();
        self.editor.visible_blocks = None;
        self.editor.last_offset = None;
        self.editor.last_caret = None;
        self.editor.last_sync = None;
        self.editor.reveal_pending = false;
        self.editor.reveal_until = 0.0;
        self.editor.sync_suppressed_until = None;
    }

    /// The caret a first edit should start at: the source line of the heading
    /// above the reader viewport (E-03's basic form).
    fn reading_caret(&self, doc: &Document) -> Option<usize> {
        let heading = self.reading.viewport_anchor.get()?.heading?;
        let line = self.active_tab()?.view.reader.as_ref()?.headings.get(heading)?.line;
        let buffer = doc.text();
        let byte = buffer.line_start(line);
        Some(byte.min(buffer.text().len()))
    }

    /// Leaves edit mode, flushing the draft and releasing the lease.
    pub(crate) fn leave_editor(&mut self, tx: &mut Tx) {
        self.editor.last_escape_at = None;
        self.ui.block_menu = None;
        self.flush_active_draft();
        let Some(doc) = self.active_tab().and_then(Tab::document).cloned() else { return };
        let tab_id = self.active_tab().map(|tab| tab.id);
        if let Some(tab_id) = tab_id {
            doc.release_edit(EditLease { window: self.id(), tab: tab_id });
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.view.mode = Mode::Viewer;
            tab.view.editor = None;
            tab.view.edit_base = None;
            tab.view.preview_pending = false;
            tab.view.preview_due = None;
            tab.view.dirty = doc.is_dirty();
            tab.view.find_bar.open = false;
            tab.view.slash_menu = crate::edit_ui::SlashMenu::default();
        }
        self.editor.goto_line = None;
        self.editor.format_menu = None;
        self.editor.prompt = None;
        self.editor.spell.reset();
        self.reset_scroll_sync();
        self.sync_editor_panes();
        self.sync_editor_titles();
        // The reading view is back: the line may show once the page settles, and
        // the reading time is counted from the model the edit produced.
        self.section_line.reset();
        self.reading_keys.reset();
        self.reading.landing = None;
        self.reading.landing_flash = None;
        self.refresh_reading_stats();
        tx.wake_at(self.ui.last_now + crate::section::SECTION_SETTLE_SECONDS + 0.02);
        tx.blur();
    }

    /// Hides the reading panes while the active tab edits, and restores them
    /// when it returns to reading (E-02).
    pub(crate) fn sync_editor_panes(&mut self) {
        let editing = self.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor);
        if editing {
            if self.editor.saved_panes.is_none()
                && let Some(tab) = self.active_tab()
            {
                self.editor.saved_panes =
                    Some((tab.id, self.left.is_open(), self.right.is_open(), tab.view.outline_hidden));
            }
            self.left.set_open(false);
            self.right.set_open(false);
            if let Some(tab) = self.active_tab_mut() {
                tab.view.outline_hidden = true;
            }
        } else if let Some((tab_id, left, right, outline_hidden)) = self.editor.saved_panes.take() {
            self.left.set_open(left);
            self.right.set_open(right);
            // Only the tab that was editing gets its own outline choice back;
            // another tab keeps the preference it already had.
            if let Some(tab) = self.active_tab_mut().filter(|tab| tab.id == tab_id) {
                tab.view.outline_hidden = outline_hidden;
            }
            self.sync_outline();
        }
    }

    /// `Ctrl+E` while dirty: ask before discarding (E-13).
    fn request_leave_editor(&mut self, tx: &mut Tx) {
        let dirty = self.active_tab().is_some_and(|tab| tab.view.dirty);
        if !dirty {
            self.leave_editor(tx);
            return;
        }
        if let Some(doc) = self.active_document_id() {
            self.flush_active_draft();
            self.editor.prompt = Some(Prompt::DiscardEdit { doc });
        }
    }

    /// The E-13 prompt's **Discard** action: reload the saved text into the
    /// preview, remove the draft and leave edit mode.
    pub(crate) fn prompt_discard_edit(&mut self, tx: &mut Tx) {
        self.editor.prompt = None;
        self.discard_editor(tx);
    }

    /// Discards the editor buffer and returns to the saved text.
    fn discard_editor(&mut self, tx: &mut Tx) {
        let Some(doc) = self.active_tab().and_then(Tab::document).cloned() else { return };
        let base = self.active_tab().and_then(|tab| tab.view.edit_base.clone());
        if let Some(base) = base {
            let eol = doc.saved.borrow().eol;
            doc.replace_all(&base, eol);
        }
        if let Some(key) = draft_key(&doc) {
            self.shared.queue_store(StorePatch::RemoveDraft(key));
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.view.dirty = doc.is_dirty();
            tab.view.draft_written = false;
        }
        self.leave_editor(tx);
    }

    /// Applies the bound field's text to the shared document after an edit
    /// (`on_input_change`; a common prefix/suffix delta). In block mode the
    /// edited field is committed into the block journal first, so preview,
    /// undo, drafts and the dirty check all see the same source.
    pub(crate) fn editor_changed(&mut self) {
        let kind = std::mem::replace(&mut self.editor.block_kind, lmdv_edit::BlockEditKind::Typing);
        let Some(doc) = self.active_tab().and_then(Tab::document).cloned() else { return };
        let Some(tab) = self.active_tab_mut() else { return };
        let Some(editor) = tab.view.editor.as_mut() else { return };
        let committed = editor.block_mode_active()
            && editor.active_block().is_some_and(|index| editor.commit_block_as(index, kind));
        if !committed {
            editor.note_edit();
        }
        let text = editor.text().to_owned();
        let old = doc.text().text().to_owned();
        if text != old {
            let (start, old_end, new_end) = change_range(&old, &text);
            doc.apply(start..old_end, &text[start..new_end]);
            editor.set_doc_revision(doc.revision());
        }
        let dirty = tab.view.edit_base.as_deref() != Some(text.as_str());
        tab.view.dirty = dirty;
        tab.view.preview_pending = true;
        tab.view.preview_due = None;
        tab.view.draft_pending = true;
        tab.view.draft_due = None;
        tab.view.disk_changed = false;
        self.refresh_find_hits();
        self.refresh_slash_menu();
    }

    /// `Return` in the source field: a paragraph break, not a submit (6.2).
    pub(crate) fn editor_insert_newline(&mut self) {
        let Some(tab) = self.active_tab_mut() else { return };
        let Some(editor) = tab.view.editor.as_mut() else { return };
        editor.insert("\n");
        self.editor_changed();
    }

    /// Editor-local chords win over the editor (6.2).
    pub(crate) fn editor_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        // Any key other than Tab cancels an armed Escape (E-33).
        if event.key != Key::Tab && event.key != Key::Escape {
            self.editor.last_escape_at = None;
        }
        if self.editor_ui_key(event, tx) {
            // An Escape that closed a panel or menu is not the arm gesture.
            if event.key == Key::Escape {
                self.editor.last_escape_at = None;
            }
            return true;
        }
        // E-33: Tab always indents the source, never toggles focus. Block fields
        // handle it with their own index in `block_key`; this is the single
        // field. After Escape, the same Tab leaves the editor instead.
        if crate::indent::plain_tab(event) {
            if self.escape_then_tab(tx) {
                return true;
            }
            let block_mode = self
                .active_tab()
                .and_then(|tab| tab.view.editor.as_ref())
                .is_some_and(|editor| editor.block_mode_active());
            if block_mode {
                // `block_key` already indented it; never let Rune's focus
                // traversal see a source Tab.
                return true;
            }
            return self.editor_tab(event.mods.shift);
        }
        if event.key == Key::Escape {
            let selection = self.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(EditorSession::selection);
            if let Some(selection) = selection.filter(|selection| !selection.is_empty()) {
                // E-33: the first Escape clears a selection; the next one arms
                // the leave gesture, so a panel, menu or selection takes one
                // Escape of its own.
                self.editor.last_escape_at = None;
                if let Some(editor) = self.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()) {
                    editor.set_selection(selection.end..selection.end);
                }
                return true;
            }
            // Keep the caret where it is and let the next Tab leave. Blurring
            // here would let Rune's focus fallback take a control, whose Tab
            // traversal runs before the app's global key router.
            self.editor.last_escape_at = Some(Instant::now());
            return true;
        }
        if event.is_command('e') && !event.mods.shift {
            self.toggle_editor_mode(tx);
            return true;
        }
        if event.is_command('s') {
            if event.mods.shift {
                self.request_save_as_active(tx);
            } else {
                self.request_save_active(tx);
            }
            return true;
        }
        if event.is_command('r') && !event.mods.shift {
            self.request_reload(tx);
            return true;
        }
        if event.is_command('w') {
            if event.mods.shift {
                self.request_close_window();
            } else {
                self.close_active_tab();
            }
            return true;
        }
        if event.key == Key::Function(5) {
            self.request_reload(tx);
            return true;
        }
        false
    }

    /// Tab/Shift+Tab in single-field mode: indent or outdent the selected lines
    /// as one undo step (E-33). The key is always consumed, so a no-op outdent
    /// at column 0 never moves focus.
    pub(crate) fn editor_tab(&mut self, outdent: bool) -> bool {
        let changed = {
            let Some(editor) = self.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()) else { return true };
            if editor.block_mode_active() {
                return true;
            }
            editor.edit.undo_group(|edit| crate::indent::indent_with_tab(edit, outdent))
        };
        if changed {
            self.editor_changed();
        }
        true
    }

    /// Tab with nothing focused: within [`ESCAPE_TAB_WINDOW`] of Escape leaving
    /// the source, hand the keyboard to the toolbar's preview toggle (E-33).
    /// A stale timestamp lets Rune's own focus traversal handle Tab.
    pub(crate) fn escape_then_tab(&mut self, tx: &mut Tx) -> bool {
        let Some(at) = self.editor.last_escape_at.take() else { return false };
        if at.elapsed() > ESCAPE_TAB_WINDOW {
            return false;
        }
        tx.focus(crate::edit_ui::PREVIEW_TOGGLE_KEY, false);
        true
    }

    /// Saves the active document, if it has a file; pasted text gets Save as.
    pub(crate) fn request_save_active(&mut self, tx: &mut Tx) {
        let Some(doc) = self.active_document_id() else { return };
        self.request_save(doc, AfterSave::Dismiss, tx);
    }

    /// Recreates a file deleted while open (E-10's Save again).
    pub(crate) fn request_save_again(&mut self) {
        let Some(doc) = self.active_document_id() else { return };
        let path = self.shared.docs().get(doc).and_then(|document| document.key.borrow().path().map(Path::to_path_buf));
        if let Some(path) = path {
            self.start_save(doc, path, None, true, AfterSave::Dismiss);
        }
    }

    /// `Ctrl+Shift+S`: ask for a new path (E-11's save dialog).
    pub(crate) fn request_save_as_active(&mut self, tx: &mut Tx) {
        let Some(doc) = self.active_document_id() else { return };
        self.request_save_as(doc, AfterSave::Dismiss, tx);
    }

    /// Saves `doc` at its current path, or asks for one (E-08/E-11).
    pub(crate) fn request_save(&mut self, doc: DocumentId, after: AfterSave, tx: &mut Tx) {
        if self.editor.after_save.contains_key(&doc) || self.editor.save_as.pending.contains_key(&doc) {
            self.notice(lmdv_i18n::t!("msg.saveInProgress"));
            return;
        }
        let Some(document) = self.shared.docs().get(doc) else { return };
        let path = document.key.borrow().path().map(Path::to_path_buf);
        match path {
            Some(path) => self.start_save(doc, path, Some(document.saved.borrow().meta), false, after),
            None => self.request_save_as(doc, after, tx),
        }
    }

    /// Asks for a path and saves there (Save as).
    pub(crate) fn request_save_as(&mut self, doc: DocumentId, after: AfterSave, tx: &mut Tx) {
        let Some(document) = self.shared.docs().get(doc) else { return };
        let title = document.title.borrow().clone();
        let directory = self.save_as_folder(&document);
        let options = FileDialogOptions {
            title: lmdv_i18n::t!("tab.saveAs").to_owned(),
            directory,
            file_name: Some(suggested_file_name(&title)),
            ..FileDialogOptions::default()
        }
        .filter("Markdown", &MARKDOWN_EXTENSIONS);
        tx.save_file(options, move |state: &mut WindowState, result, _| match result {
            Ok(Some(path)) => state.save_as_chosen(doc, path, after),
            Ok(None) => {
                // Cancelling the path cancels a pending close or quit as well.
                state.abandon_quit();
                state.abandon_close();
            }
            Err(error) => {
                state.abandon_quit();
                state.abandon_close();
                state.notice(lmdv_i18n::t!("msg.saveDialogFailed", "err" => error));
            }
        });
    }

    /// The folder Save as opens in for `document` (E-11).
    ///
    /// A file saves beside itself. Pasted text has no folder of its own: it
    /// starts in the file browser's folder — the paste home — and in the user's
    /// Documents folder while the pane has no folder yet.
    pub fn save_as_folder(&self, document: &Document) -> Option<PathBuf> {
        let path = document.key.borrow().path().map(Path::to_path_buf);
        save_as_folder_for(path.as_deref(), self.browser.root(), documents_folder().as_deref())
    }

    /// A path came back from the Save as dialog (E-11/E-12).
    ///
    /// Nothing is written until a worker has probed the path: choosing a file
    /// another tab already shows must merge that view or stop, never leave two
    /// documents for one file.
    pub fn save_as_chosen(&mut self, doc: DocumentId, path: PathBuf, after: AfterSave) {
        if self.editor.after_save.contains_key(&doc) || self.editor.save_as.pending.contains_key(&doc) {
            self.notice(lmdv_i18n::t!("msg.saveInProgress"));
            return;
        }
        let Some(document) = self.shared.docs().get(doc) else { return };
        // The document's own file cannot collide with itself: no probe needed.
        if document.key.borrow().path() == Some(path.as_path()) {
            self.start_save(doc, path, None, true, after);
            return;
        }
        let generation = self.editor.save_as.probes.begin(doc);
        self.editor.save_as.pending.insert(doc, PendingSaveAs { path: path.clone(), after });
        let mailbox = self.save_as_mailbox();
        self.shared.jobs().probe(mailbox, doc, path, generation);
    }

    /// The private mailbox the Save as probes answer on.
    fn save_as_mailbox(&mut self) -> Arc<WorkerMailbox> {
        let waker = self.window_waker();
        if let Some(mailbox) = &self.editor.save_as.mailbox {
            // The host installs its waker after the window is built, so a
            // mailbox built before that would keep the quiet one: attach again.
            mailbox.attach(waker);
            return Arc::clone(mailbox);
        }
        let mailbox = Arc::new(WorkerMailbox::new(self.id()));
        mailbox.attach(waker);
        self.editor.save_as.mailbox = Some(Arc::clone(&mailbox));
        mailbox
    }

    /// Consumes the path probes' answers (E-12).
    fn poll_save_as_probes(&mut self) -> bool {
        let Some(mailbox) = self.editor.save_as.mailbox.clone() else { return false };
        let mut changed = false;
        for msg in mailbox.drain() {
            if let JobMsg::Probed { doc, generation, result, .. } = msg {
                changed |= self.finish_save_as_probe(doc, generation, result);
            }
        }
        changed
    }

    /// Decides a Save as once its probe answered: save, merge a clean duplicate
    /// view, or refuse (E-12).
    fn finish_save_as_probe(&mut self, doc: DocumentId, generation: u64, result: Result<FileMeta, ReadError>) -> bool {
        if !self.editor.save_as.probes.finish(doc, generation) {
            return false;
        }
        let Some(pending) = self.editor.save_as.pending.remove(&doc) else { return false };
        // The tab went away while the probe ran: nothing may be written.
        if self.shared.docs().get(doc).is_none() {
            return false;
        }
        let current_path =
            self.shared.docs().get(doc).and_then(|document| document.key.borrow().path().map(Path::to_path_buf));
        if current_path.as_deref() == Some(pending.path.as_path()) {
            self.start_save(doc, pending.path, None, true, pending.after);
            return true;
        }
        let duplicate = {
            let docs = self.shared.docs();
            // The identity covers symlinks, hard links and another spelling;
            // the exact path catches a document that is still loading.
            let by_identity = match &result {
                Ok(meta) => docs.find_identity(meta),
                Err(_) => None,
            };
            by_identity.or_else(|| docs.find_path(&pending.path)).map(|other| other.id).filter(|id| *id != doc)
        };
        let Some(other) = duplicate else {
            self.start_save(doc, pending.path, None, true, pending.after);
            return true;
        };
        let (dirty, elsewhere) = {
            let docs = self.shared.docs();
            docs.get(other).map_or((false, false), |document| (document.is_dirty(), !self.is_last_view(&document)))
        };
        let name = title_for_path(&pending.path);
        if dirty {
            self.notice(lmdv_i18n::t!("msg.saveAsOpenDirty", "name" => name));
            self.abandon_quit();
            self.abandon_close();
            return true;
        }
        if elsewhere {
            // E-12: a clean duplicate in another window is released, so this
            // save can take the file over. Only when no window can be asked
            // (it closed between the probe and now) does the warning remain.
            if self.release_elsewhere(other) {
                self.start_save(doc, pending.path, None, true, pending.after);
                return true;
            }
            self.notice(lmdv_i18n::t!("msg.saveAsOpenElsewhere", "name" => name));
            self.abandon_quit();
            self.abandon_close();
            return true;
        }
        // Clean and only here: release the duplicate's tab, then write; one
        // document for the file remains and it is this one.
        let tabs: Vec<TabId> =
            self.tabs.iter().filter(|tab| tab.document_id() == Some(other)).map(|tab| tab.id).collect();
        for tab in tabs {
            self.release_duplicate_view(tab);
        }
        self.start_save(doc, pending.path, None, true, pending.after);
        true
    }

    /// Closes a clean duplicate tab after a Save as merged its file (E-12) or
    /// a load merged two spellings of one file (OI-005).
    ///
    /// The tab is not a reopen place: the document it showed is still open, as
    /// the document that just took its file over.
    pub(crate) fn release_duplicate_view(&mut self, tab: TabId) {
        let stale_draft = {
            let Some(index) = self.tabs.iter().position(|candidate| candidate.id == tab) else { return };
            let Some(document) = self.tabs[index].document().cloned() else { return };
            (self.tabs[index].view.draft_written && !document.is_dirty() && self.is_last_view(&document))
                .then(|| draft_key(&document))
                .flatten()
        };
        if let Some(key) = stale_draft {
            self.shared.queue_store(StorePatch::RemoveDraft(key));
        }
        if let Some(tab) = self.tabs.iter_mut().find(|candidate| candidate.id == tab) {
            tab.view.dirty = false;
            tab.view.draft_written = false;
            tab.view.draft_pending = false;
        }
        self.close_tab_without_history(tab);
    }

    /// Asks the windows showing `doc` to close their clean views (E-12).
    ///
    /// The release is a same-thread message: the other window closes its views
    /// on its next poll. The write may start now; when it lands, this document
    /// is the file's one view. False when no live window can be asked, which
    /// keeps the warning as the fallback.
    fn release_elsewhere(&self, doc: DocumentId) -> bool {
        let targets = self.shared.windows_showing(doc, self.id());
        let mut delivered = false;
        for id in targets {
            delivered |= self.shared.post_window(id, UiMsg::ReleaseDocument { doc });
        }
        delivered
    }

    /// Closes this window's clean views of `doc` after another window's Save
    /// as takes the file over (E-12).
    ///
    /// A dirty buffer is never released: the other window's save then cannot
    /// be undone, but this window keeps its text and its draft, and the
    /// ordinary disk-change prompt decides what happens next.
    pub(crate) fn release_clean_views(&mut self, doc: DocumentId) -> bool {
        let Some(document) = self.shared.docs().get(doc) else { return false };
        let dirty = document.is_dirty();
        drop(document);
        if dirty {
            return false;
        }
        let tabs: Vec<TabId> =
            self.tabs.iter().filter(|tab| tab.document_id() == Some(doc)).map(|tab| tab.id).collect();
        if tabs.is_empty() {
            return false;
        }
        for tab in tabs {
            self.release_duplicate_view(tab);
        }
        // The released document may have been rekeyed away already; collect it
        // now, while no local `Rc` keeps it alive.
        self.shared.docs_mut().collect_unused();
        true
    }

    /// Starts a save job. `expected` is the last known metadata; `None`
    /// overwrites whatever is on disk (Save as, or Overwrite after a conflict).
    pub(crate) fn start_save(
        &mut self,
        doc: DocumentId,
        path: PathBuf,
        expected: Option<FileMeta>,
        create: bool,
        after: AfterSave,
    ) {
        let Some(document) = self.shared.docs().get(doc) else { return };
        // The editor owns the newest text (including uncommitted block fields).
        // `save_document` applies the line ending itself, so the text handed
        // to it stays LF (E-09): converting here too would double the CRs.
        let (text, eol) = match self.sync_editor_document(doc) {
            Some(editor) => (editor.text().to_owned(), editor.eol()),
            None => {
                let buffer = document.text();
                (buffer.text().to_owned(), buffer.eol())
            }
        };
        if after == AfterSave::Dismiss
            && !create
            && expected.is_some()
            && !document.is_dirty()
            && matches!(*document.status.borrow(), DocStatus::Ready)
            && document.key.borrow().path() == Some(path.as_path())
        {
            self.notice(lmdv_i18n::t!("msg.noChanges"));
            return;
        }
        let generation = self.editor.saves.begin(doc);
        self.editor.save_texts.insert(doc, text.clone());
        self.editor.after_save.insert(doc, after);
        self.shared.jobs().save(self.worker().clone(), doc, generation, path, text, eol, expected, create);
    }

    /// Commits the editor session's text into the document and returns it.
    fn sync_editor_document(&mut self, doc: DocumentId) -> Option<EditorSession> {
        let index = self.tabs.iter().position(|tab| tab.document_id() == Some(doc))?;
        let document = self.shared.docs().get(doc)?;
        let tab = self.tabs.get_mut(index)?;
        let editor = tab.view.editor.as_mut()?;
        editor.flush_blocks();
        let text = editor.text().to_owned();
        let current = document.text().text().to_owned();
        if text != current {
            let (start, old_end, new_end) = change_range(&current, &text);
            document.apply(start..old_end, &text[start..new_end]);
            editor.set_doc_revision(document.revision());
        }
        Some(editor.clone())
    }

    /// Handles a save completion. A viewer task tick owns its job; every other
    /// save is the editor's.
    pub(crate) fn handle_saved(
        &mut self,
        job: lmdv_core::JobId,
        doc: DocumentId,
        generation: u64,
        result: Result<lmdv_io::SaveOutcome, WriteError>,
        tx: &mut Tx,
    ) -> bool {
        if self.tasks.is_job(job) {
            return self.handle_task_saved(job, doc, generation, result).unwrap_or(false);
        }
        if !self.editor.saves.finish(doc, generation) {
            return false;
        }
        let Some(document) = self.shared.docs().get(doc) else {
            self.editor.after_save.remove(&doc);
            self.editor.save_texts.remove(&doc);
            return false;
        };
        match result {
            Ok(outcome) => {
                let old_path = document.key.borrow().path().map(Path::to_path_buf);
                let old_key = draft_key(&document);
                // E-11: pasted text's Back/Forward places are keyed `pasted:<id>`
                // (nav's `place_key`), not by the draft key; read it before the
                // rekey turns the document into a file.
                let old_place = match &*document.key.borrow() {
                    DocumentKey::Untitled(paste) => Some(format!("{}{}", crate::nav::PASTED_PREFIX, paste.0)),
                    DocumentKey::File(_) | DocumentKey::Start => None,
                };
                let new_path = outcome.path.clone();
                // The text actually written, not whatever the buffer holds if
                // the user typed again while the job ran.
                let saved_text = self.editor.save_texts.remove(&doc);
                let hash = saved_text.as_deref().map(hash_text).unwrap_or_else(|| document.hash());
                if old_path.as_deref() != Some(new_path.as_path()) {
                    self.shared.docs_mut().rekey(doc, DocumentKey::File(new_path.clone()));
                    *document.title.borrow_mut() = title_for_path(&new_path);
                    self.shared.watch_path(&new_path);
                    match &old_path {
                        Some(old) => {
                            self.shared.unwatch_path(old);
                            self.shared.rename_position(old, &new_path);
                            self.shared.rename_recent(old, &new_path);
                            self.rename_history_paths(old, &new_path);
                            // OI-173: another window may show the same document
                            // and still name the old spelling in its places.
                            self.broadcast_document_rename(doc, old, &new_path);
                        }
                        None => {
                            // E-11: pasted text's Back/Forward places name the
                            // new file from here on.
                            if let Some(old_place) = old_place.as_deref() {
                                self.rename_history_keys(old_place, &new_path);
                            }
                        }
                    }
                }
                document.set_saved(outcome.meta, hash, new_path.clone());
                *document.status.borrow_mut() = DocStatus::Ready;
                // The atomic replace changed the inode; refresh the registry's
                // path/identity maps so watcher routing finds this document.
                self.shared.docs_mut().insert_ready(document.clone());
                document.ignore_writes_until.set(Some(Instant::now() + OWN_WRITE_SUPPRESS));
                document.last_local_write.set(Some(outcome.meta));
                self.shared.record_recent(new_path.clone(), crate::shared::unix_millis());
                self.shared.mark_draft_checked(doc);
                if let Some(old_key) = old_key {
                    self.shared.queue_store(StorePatch::RemoveDraft(old_key));
                }
                self.shared.queue_store(StorePatch::RemoveDraft(draft_key_for_path(&new_path)));
                let dirty = document.is_dirty();
                for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(doc)) {
                    tab.refresh_title();
                    tab.view.dirty = dirty;
                    tab.view.disk_changed = false;
                    if let Some(text) = &saved_text {
                        tab.view.edit_base = Some(text.clone());
                    }
                    if !dirty {
                        tab.view.draft_pending = false;
                        tab.view.draft_due = None;
                        tab.view.draft_written = false;
                    }
                }
                if let Some(editor_tab) = self.tabs.iter_mut().find(|tab| tab.document_id() == Some(doc))
                    && let Some(editor) = editor_tab.view.editor.as_mut()
                {
                    editor.set_base_revision(document.revision());
                }
                self.sync_editor_titles();
                self.notice(lmdv_i18n::t!("msg.saved"));
                self.editor.saved_flash_until = Some(self.ui.last_now + SAVED_FLASH_SECONDS);
                self.editor.saved_flash_armed = false;
                let after = self.editor.after_save.remove(&doc).unwrap_or_default();
                if dirty {
                    // The user typed while the job ran: keep the window and
                    // the tab, and let the dirty guard ask again.
                    match after {
                        AfterSave::CloseTab(tab) => {
                            self.editor.prompt = Some(Prompt::CloseTab { tab, doc });
                        }
                        AfterSave::LeaveEditor => {
                            self.editor.prompt = Some(Prompt::DiscardEdit { doc });
                        }
                        AfterSave::CloseWindow => {
                            self.abandon_close();
                        }
                        AfterSave::Quit => {
                            self.abandon_quit();
                            self.notice(lmdv_i18n::t!("msg.quitCancelled"));
                        }
                        AfterSave::Dismiss => {}
                    }
                } else {
                    self.finish_after_save(doc, after, tx);
                }
                true
            }
            Err(error) => {
                self.editor.save_texts.remove(&doc);
                let mut after = self.editor.after_save.remove(&doc).unwrap_or_default();
                if after == AfterSave::Quit {
                    // One failed save cancels the whole quit; the conflict or
                    // delete question must not resume a half-finished plan
                    // when the user resolves it.
                    after = AfterSave::Dismiss;
                    self.abandon_quit();
                }
                match error {
                    WriteError::Conflict => {
                        self.editor.prompt = Some(Prompt::SaveConflict { doc, after });
                    }
                    WriteError::Deleted(_) => {
                        self.editor.prompt = Some(Prompt::FileDeleted { doc, after });
                    }
                    error => {
                        self.editor.close_on_clean = false;
                        self.notice(lmdv_i18n::t!("msg.saveFailed", "err" => error));
                        if after == AfterSave::CloseWindow {
                            self.editor.prompt = None;
                            self.abandon_close();
                        }
                    }
                }
                true
            }
        }
    }

    fn finish_after_save(&mut self, doc: DocumentId, after: AfterSave, tx: &mut Tx) {
        match after {
            AfterSave::Dismiss => {}
            AfterSave::LeaveEditor => self.leave_editor_for(doc),
            AfterSave::CloseTab(tab) => {
                self.close_tab_unchecked(tab);
            }
            AfterSave::CloseWindow => {
                self.editor.close_pending.remove(&doc);
                self.close_if_clean();
            }
            AfterSave::Quit => self.save_next_quit_document(tx),
        }
    }

    /// Leaves edit mode for `doc`, active or behind.
    pub(crate) fn leave_editor_for(&mut self, doc: DocumentId) {
        let Some(index) = self.tabs.iter().position(|tab| tab.document_id() == Some(doc)) else { return };
        if index == self.active {
            self.leave_editor(&mut Tx::default());
            return;
        }
        let tab_id = self.tabs[index].id;
        if let Some(document) = self.tabs[index].document().cloned() {
            document.release_edit(EditLease { window: self.id(), tab: tab_id });
            if let Some(tab) = self.tabs.get_mut(index) {
                tab.view.mode = Mode::Viewer;
                tab.view.editor = None;
                tab.view.edit_base = None;
                tab.view.preview_pending = false;
                tab.view.preview_due = None;
                tab.view.dirty = document.is_dirty();
            }
        }
        self.sync_editor_titles();
    }

    fn close_if_clean(&mut self) {
        if self.editor.close_on_clean
            && self.editor.close_pending.is_empty()
            && !self.editor.after_save.values().any(|after| *after == AfterSave::CloseWindow)
            && !self.has_unsaved_documents()
        {
            self.editor.close_on_clean = false;
            self.issue_close();
        }
    }

    /// A close that could not finish leaves the window open: routing may use
    /// it again (T-25).
    fn abandon_close(&mut self) {
        for after in self.editor.after_save.values_mut() {
            if *after == AfterSave::CloseWindow {
                *after = AfterSave::Dismiss;
            }
        }
        self.editor.close_on_clean = false;
        self.editor.close_pending.clear();
        self.shared.set_window_closing(self.id(), false);
    }

    /// A quit that could not finish leaves every window open and routable.
    fn abandon_quit(&mut self) {
        for after in self.editor.after_save.values_mut() {
            if *after == AfterSave::Quit {
                *after = AfterSave::Dismiss;
            }
        }
        self.editor.quit.clear();
        self.editor.close_on_clean = false;
        self.shared.set_window_closing(self.id(), false);
    }

    /// Saves the quit plan's next document, one at a time, then quits.
    ///
    /// Pasted text has no path yet, so it asks for one; the dialog's callback
    /// resumes the plan. A document whose save is already in flight is never
    /// started twice: its completion is redirected to the quit instead.
    fn save_next_quit_document(&mut self, tx: &mut Tx) {
        loop {
            let Some(doc) = self.editor.quit.queue.pop_front() else {
                self.finish_quit();
                return;
            };
            let Some(document) = self.shared.docs().get(doc) else { continue };
            let pasted = matches!(*document.key.borrow(), DocumentKey::Untitled(_));
            if !pasted && !document.is_dirty() {
                continue;
            }
            if let Some(after) = self.editor.after_save.get_mut(&doc) {
                *after = AfterSave::Quit;
                return;
            }
            match document.key.borrow().path().map(Path::to_path_buf) {
                Some(path) => {
                    let expected = Some(document.saved.borrow().meta);
                    self.start_save(doc, path, expected, false, AfterSave::Quit);
                }
                None => self.request_save_as(doc, AfterSave::Quit, tx),
            }
            return;
        }
    }

    /// Ends a quit whose saves all landed, or asks again if a new document
    /// became dirty while the saves ran.
    fn finish_quit(&mut self) {
        let remaining = self.quit_documents();
        if !remaining.is_empty() {
            self.arm_quit_prompt(remaining);
            return;
        }
        self.editor.quit.clear();
        self.shared.set_quitting();
        self.ui.quit_requested = true;
    }

    /// Opens the aggregate quit question for `docs` in this window.
    fn arm_quit_prompt(&mut self, docs: Vec<CloseDoc>) {
        self.editor.quit.clear();
        self.editor.quit.docs = docs.iter().map(|doc| doc.id).collect();
        self.editor.quit.names = docs.iter().map(|doc| doc.title.clone()).collect();
        self.shared.set_window_closing(self.id(), true);
        self.editor.prompt = Some(Prompt::Quit);
        self.sync_registry();
    }

    /// The names the aggregate quit question shows.
    pub fn quit_document_names(&self) -> Vec<String> {
        self.editor.quit.names.clone()
    }

    /// Drops every draft belonging to `docs`, for this window's views.
    ///
    /// `all_views` is true at quit, where every window is closing. A window
    /// close leaves a draft alone while another window still shows the
    /// document: the buffer, lease and draft belong to that view.
    fn discard_documents(&mut self, docs: &[DocumentId], all_views: bool) {
        let window = self.id();
        for doc in docs {
            let Some(document) = self.shared.docs().get(*doc) else { continue };
            let last = self.is_last_view(&document);
            if (all_views || last)
                && let Some(key) = draft_key(&document)
            {
                self.shared.queue_store(StorePatch::RemoveDraft(key));
            }
            for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(*doc)) {
                let tab_id = tab.id;
                document.release_edit(EditLease { window, tab: tab_id });
                tab.view.dirty = false;
                tab.view.draft_written = false;
                tab.view.draft_pending = false;
                tab.view.draft_due = None;
            }
        }
    }

    /// Writes the active tab's draft now (tab switch, close, quit).
    pub(crate) fn flush_active_draft(&mut self) {
        let shared = self.shared.clone();
        let Some(tab) = self.active_tab_mut() else { return };
        flush_tab_draft(shared.as_ref(), tab);
    }

    /// Writes every dirty editor tab's draft (window close, app quit).
    pub(crate) fn flush_all_drafts(&mut self) {
        let shared = self.shared.clone();
        for tab in &mut self.tabs {
            flush_tab_draft(shared.as_ref(), tab);
        }
    }

    /// The dictionary chosen for this launch, for the settings row (E-SPELL).
    pub fn spell_availability(&self) -> Option<Availability> {
        self.shared.spell_availability()
    }

    /// The single field's syntax spans with spelling underlines composed in.
    ///
    /// The syntax spans keep their cache; spelling is painted on top, never
    /// cached with them, so a 300 ms answer never invalidates a highlight.
    pub(crate) fn editor_spans(
        &self,
        tab: &Tab,
        edit: &TextEdit,
        theme: &SyntaxTheme,
        visible: Option<Range<usize>>,
        underline: rune::Color,
    ) -> Vec<Span> {
        let spans = lmdv_edit::syntax::syntax_spans(edit, theme, visible);
        let revision = tab.view.editor.as_ref().map(EditorSession::revision).unwrap_or(0);
        let ranges = self.spell_ranges(tab.id, revision);
        if ranges.is_empty() {
            return spans;
        }
        compose_underlines(spans, ranges, underline)
    }

    /// One block field's syntax spans with spelling underlines composed in.
    ///
    /// `block` is the block's whole-source range; only a check that covered it
    /// contributes, and the painted ranges are block-local.
    pub(crate) fn editor_block_spans(
        &self,
        edit: &TextEdit,
        theme: &SyntaxTheme,
        block: Range<usize>,
        underline: rune::Color,
    ) -> Vec<Span> {
        let spans = lmdv_edit::syntax::syntax_spans(edit, theme, None);
        let Some((tab, revision)) =
            self.active_tab().and_then(|tab| tab.view.editor.as_ref().map(|editor| (tab.id, editor.revision())))
        else {
            return spans;
        };
        let ranges = self.spell_block_ranges(tab, revision, block);
        if ranges.is_empty() {
            return spans;
        }
        compose_underlines(spans, &ranges, underline)
    }

    fn spell_ranges(&self, tab: TabId, revision: u64) -> &[Range<usize>] {
        if !self.shared.prefs().editor.spellcheck {
            return &[];
        }
        self.editor
            .spell
            .check_for(tab)
            .filter(|check| check.whole && check.revision == revision)
            .map_or(&[], |check| check.ranges.as_slice())
    }

    fn spell_block_ranges(&self, tab: TabId, revision: u64, block: Range<usize>) -> Vec<Range<usize>> {
        if !self.shared.prefs().editor.spellcheck {
            return Vec::new();
        }
        let Some(check) = self.editor.spell.check_for(tab) else { return Vec::new() };
        if check.revision != revision || check.window.start > block.start || check.window.end < block.end {
            return Vec::new();
        }
        check
            .ranges
            .iter()
            .filter(|range| range.start >= block.start && range.end <= block.end)
            .map(|range| range.start - block.start..range.end - block.start)
            .collect()
    }

    /// The spellchecker's own worker mailbox, woken like the window's.
    fn spell_mailbox(&mut self) -> Arc<SpellMailbox> {
        let waker = self.window_waker();
        if let Some(mailbox) = &self.editor.spell.mailbox {
            mailbox.attach(waker);
            return Arc::clone(mailbox);
        }
        let mailbox = Arc::new(SpellMailbox::new());
        mailbox.attach(waker);
        self.editor.spell.mailbox = Some(Arc::clone(&mailbox));
        mailbox
    }

    /// Whether spelling can show: the active tab is editing, or the settings
    /// dialog (which owns the toggle) is open.
    ///
    /// Discovery starts on the first poll, but its answer only counts as a
    /// frame change while one of these is true; in a reader-only window the
    /// process-wide scan finishes without asking the shell for another frame.
    fn spell_ui_visible(&self) -> bool {
        self.settings_is_open() || self.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor)
    }

    /// Discovery, loading and every check run on workers (ADR-008); the frame
    /// only arranges the 300 ms debounce and copies the answer's ranges.
    fn poll_spell(&mut self, now: f64, tx: &mut Tx) -> bool {
        let changed = self.drain_spell();
        // One scan per process: the dictionary follows the UI locale chosen at
        // start and a language change only applies to spelling on the next
        // start (E-32).
        if self.shared.spell_discovery() == SpellDiscovery::Unknown
            && let Some(generation) = self.shared.begin_spell_discovery()
        {
            let mailbox = self.spell_mailbox();
            let roots = lmdv_edit::spell::dictionary_roots(Some(&self.shared.dirs().data));
            let tags = lmdv_edit::spell::requested_tags(self.shared.prefs().locale.as_deref());
            self.shared.jobs().discover_spelling(mailbox, generation, roots, tags);
        }
        let enabled = self.shared.prefs().editor.spellcheck;
        if enabled
            && self.shared.spell_dictionary().is_none()
            && let Some(files) = self.shared.spell_availability().map(|found| found.files)
            && let Some(generation) = self.shared.begin_spell_load()
        {
            let mailbox = self.spell_mailbox();
            self.shared.jobs().load_spelling(mailbox, generation, files);
        }
        let Some((tab, revision, window, text, whole)) = self.spell_target() else {
            self.editor.spell.due = None;
            return changed;
        };
        let Some(dictionary) = self.shared.spell_dictionary() else {
            self.editor.spell.due = None;
            return changed;
        };
        if !enabled || window.is_empty() {
            self.editor.spell.due = None;
            return changed;
        }
        let satisfied =
            self.editor.spell.check_for(tab).is_some_and(|check| check.revision == revision && check.window == window);
        if satisfied {
            self.editor.spell.due = None;
            return changed;
        }
        let edited = self.editor.spell.seen != Some((tab, revision));
        if edited {
            // A trailing debounce: the last keystroke pushes the check out.
            self.editor.spell.seen = Some((tab, revision));
            self.editor.spell.due = Some(now + SPELL_DEBOUNCE);
            tx.wake_at(now + SPELL_DEBOUNCE);
        } else if self.editor.spell.due.is_none() {
            // Scrolling in block mode changed the visible rows; check them
            // without waiting for the next keystroke.
            self.editor.spell.due = Some(now);
        }
        if !self.editor.spell.due.is_some_and(|due| now >= due) || self.editor.spell.in_flight.is_some() {
            return changed;
        }
        let mailbox = self.spell_mailbox();
        let job = self.shared.jobs().check_spelling(mailbox, tab, revision, dictionary, window, text, whole);
        self.editor.spell.in_flight = Some(job);
        self.editor.spell.in_flight_tab = Some(tab);
        self.editor.spell.due = None;
        changed
    }

    /// Applies the workers' answers. A stale generation, revision or tab is
    /// dropped; only the current editor's ranges reach the view.
    fn drain_spell(&mut self) -> bool {
        let Some(mailbox) = self.editor.spell.mailbox.clone() else { return false };
        let mut changed = false;
        for message in mailbox.drain() {
            match message {
                SpellMsg::Discovered { generation, found } => {
                    let applied = self.shared.finish_spell_discovery(generation, found);
                    changed |= applied && self.spell_ui_visible();
                }
                SpellMsg::Loaded { generation, result } => {
                    let applied = self.shared.finish_spell_load(generation, result.ok());
                    changed |= applied && self.spell_ui_visible();
                }
                SpellMsg::Checked { job, tab, revision, window, whole, ranges } => {
                    if self.editor.spell.in_flight == Some(job) {
                        self.editor.spell.in_flight = None;
                        self.editor.spell.in_flight_tab = None;
                    }
                    let current = self
                        .active_tab()
                        .and_then(|tab| tab.view.editor.as_ref().map(|editor| (tab.id, editor.revision())));
                    if current == Some((tab, revision)) {
                        self.editor.spell.checks.insert(tab, SpellCheck { revision, window, whole, ranges });
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// The active editor's tab, revision, checked window and text.
    ///
    /// The single field checks the whole buffer. Block mode checks only the
    /// rows the source is showing (its first screenful until the view reports
    /// them), so a 1 MiB document is never walked for one keystroke.
    fn spell_target(&self) -> Option<(TabId, u64, Range<usize>, String, bool)> {
        let tab = self.active_tab()?;
        if tab.view.mode != Mode::Editor {
            return None;
        }
        let editor = tab.view.editor.as_ref()?;
        let text = editor.text();
        let block_mode = editor.block_mode_active();
        let window = if block_mode {
            let blocks = lmdv_edit::block_ranges(text);
            let visible = self.editor.visible_blocks.clone().unwrap_or_else(|| {
                let end = blocks.len().min(SPELL_BLOCK_FALLBACK);
                0..end
            });
            let start = visible.start.min(blocks.len().saturating_sub(1));
            let end = visible.end.max(start + 1).min(blocks.len());
            blocks.get(start)?.start..blocks.get(end.saturating_sub(1))?.end
        } else {
            0..text.len()
        };
        let checked = text.get(window.clone())?.to_owned();
        Some((tab.id, editor.revision(), window, checked, !block_mode))
    }

    /// Fills the preview debounce and draft timers, and consumes watcher events.
    pub(crate) fn poll_editor(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = self.poll_save_as_probes();
        changed |= self.poll_spell(now, tx);
        if let Some(until) = self.editor.saved_flash_until {
            if now >= until {
                self.editor.saved_flash_until = None;
                self.editor.saved_flash_armed = false;
                changed = true;
            } else if !self.editor.saved_flash_armed {
                self.editor.saved_flash_armed = true;
                tx.wake_at(until);
            }
        }
        // Preview and draft debounce for the active editor.
        let (preview_pending, preview_due, draft_pending, draft_due) = match self.active_tab() {
            Some(tab) if tab.view.mode == Mode::Editor => {
                (tab.view.preview_pending, tab.view.preview_due, tab.view.draft_pending, tab.view.draft_due)
            }
            _ => (false, None, false, None),
        };
        // E-05: a failed parse keeps the last good article on screen and asks
        // once for attention through the dismissible notice bar.
        changed |= self.poll_preview_error();
        if preview_pending && preview_due.is_none() {
            if let Some(tab) = self.active_tab_mut() {
                tab.view.preview_due = Some(now + PREVIEW_DEBOUNCE);
            }
            tx.wake_at(now + PREVIEW_DEBOUNCE);
            changed = true;
        }
        if preview_pending && preview_due.is_some_and(|due| now >= due) {
            if let Some(tab) = self.active_tab_mut() {
                tab.view.preview_pending = false;
            }
            changed = true;
        }
        if draft_pending && draft_due.is_none() {
            if let Some(tab) = self.active_tab_mut() {
                tab.view.draft_due = Some(now + DRAFT_DEBOUNCE);
            }
            tx.wake_at(now + DRAFT_DEBOUNCE);
            changed = true;
        }
        if draft_pending && draft_due.is_some_and(|due| now >= due) {
            self.flush_active_draft();
            if let Some(tab) = self.active_tab_mut() {
                tab.view.draft_pending = false;
                tab.view.draft_due = None;
            }
            changed = true;
        }
        changed |= self.poll_file_events(now, tx);
        // E-07: one-way editor→preview sync. It only reacts to a changed
        // source offset, a moved caret or a parse completion; manual preview
        // scrolling never reaches it.
        self.poll_scroll_sync(now, tx);
        changed
    }

    /// E-05: announces a failed preview parse once, keeping the last good
    /// article on screen.
    ///
    /// [`crate::window::TabView::reader_error`] is the failure the parse
    /// completion recorded; a later good parse clears it. Only a tab that
    /// already has a preview shows the notice — a first parse that fails has
    /// nothing to keep, and the viewer's source preview carries the reason.
    fn poll_preview_error(&mut self) -> bool {
        let failure = self.active_tab().and_then(|tab| {
            if tab.view.mode != Mode::Editor {
                return None;
            }
            let reason = tab.view.reader_error.as_ref()?;
            Some((tab.id, reason.clone(), tab.view.reader.is_some()))
        });
        let Some((tab_id, reason, has_preview)) = failure else {
            self.editor.preview_error = None;
            return false;
        };
        if !has_preview || self.editor.preview_error.as_ref().is_some_and(|(id, seen)| *id == tab_id && *seen == reason)
        {
            return false;
        }
        let message = lmdv_i18n::t!("msg.previewError", "err" => reason.clone());
        self.editor.preview_error = Some((tab_id, reason));
        self.notice(message);
        true
    }

    /// Applies watcher events since this window last looked.
    fn poll_file_events(&mut self, _now: f64, _tx: &mut Tx) -> bool {
        let events = self.shared.file_events_since(self.editor.watch_seq);
        let mut changed = false;
        for (seq, event) in events {
            self.editor.watch_seq = self.editor.watch_seq.max(seq);
            changed |= self.apply_file_event(event);
        }
        changed
    }

    /// One watcher decision (also the test seam).
    pub(crate) fn apply_file_event(&mut self, event: lmdv_io::watch::FileEvent) -> bool {
        use lmdv_io::watch::FileEvent;
        match event {
            FileEvent::Changed { path } => self.on_file_changed(&path),
            FileEvent::Renamed { from, to } => self.on_file_renamed(&from, &to),
            FileEvent::Removed { path } => self.on_file_removed(&path),
            // OI-178: window-level, not a document. The watcher already
            // re-probed every followed document, and its Changed/Removed
            // events follow this one, so one notice + a watch reinstall.
            FileEvent::Overflow => {
                self.shared.sync_watch_files();
                self.notice(lmdv_i18n::t!("msg.watchOverflow"));
                true
            }
        }
    }

    fn docs_for_path(&self, path: &Path) -> Vec<DocumentId> {
        let mut docs = Vec::new();
        for tab in &self.tabs {
            if let Some(doc) = tab.document()
                && doc.key.borrow().path() == Some(path)
                && !docs.contains(&doc.id)
            {
                docs.push(doc.id);
            }
        }
        docs
    }

    fn on_file_changed(&mut self, path: &Path) -> bool {
        let docs = self.docs_for_path(path);
        if docs.is_empty() {
            return false;
        }
        let mut changed = false;
        for doc_id in docs {
            let Some(document) = self.shared.docs().get(doc_id) else { continue };
            if document.ignore_writes_until.get().is_some_and(|until| Instant::now() < until) {
                continue;
            }
            let dirty =
                document.is_dirty() || self.tabs.iter().any(|tab| tab.document_id() == Some(doc_id) && tab.view.dirty);
            if dirty {
                for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(doc_id)) {
                    tab.view.disk_changed = true;
                }
                if self.editor.prompt.is_none() {
                    self.flush_all_drafts();
                    self.editor.prompt = Some(Prompt::DiskChanged { doc: doc_id });
                }
                changed = true;
            } else if !self.editor.after_save.contains_key(&doc_id) {
                // R-21: only different text reloads; an mtime-only touch is
                // compared on the read's return and updates the baseline then.
                self.start_reload(doc_id, path.to_path_buf(), ReloadIntent::Compare);
                changed = true;
            }
        }
        changed
    }

    fn on_file_renamed(&mut self, from: &Path, to: &Path) -> bool {
        let docs = self.docs_for_path(from);
        if docs.is_empty() {
            return false;
        }
        for doc_id in docs {
            let Some(document) = self.shared.docs().get(doc_id) else { continue };
            let old_key = draft_key(&document);
            let (meta, hash) = {
                let saved = document.saved.borrow();
                (saved.meta, saved.hash)
            };
            document.set_saved(meta, hash, to.to_path_buf());
            *document.title.borrow_mut() = title_for_path(to);
            self.shared.docs_mut().rekey(doc_id, DocumentKey::File(to.to_path_buf()));
            self.shared.watch_path(to);
            self.shared.unwatch_path(from);
            self.shared.rename_position(from, to);
            self.shared.rename_recent(from, to);
            self.rename_history_paths(from, to);
            if let Some(key) = old_key {
                self.shared.queue_store(StorePatch::RemoveDraft(key));
            }
            for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(doc_id)) {
                tab.refresh_title();
            }
        }
        // R-23: name the file the manager action moved it to. The document,
        // its places and its drafts already followed above; this is the one
        // user-visible sentence.
        self.notice(lmdv_i18n::t!("msg.renamedTo", "name" => title_for_path(to)));
        self.flush_all_drafts();
        self.sync_editor_titles();
        true
    }

    fn on_file_removed(&mut self, path: &Path) -> bool {
        let docs = self.docs_for_path(path);
        if docs.is_empty() {
            return false;
        }
        for doc_id in docs {
            let Some(document) = self.shared.docs().get(doc_id) else { continue };
            if document.ignore_writes_until.get().is_some_and(|until| Instant::now() < until) {
                continue;
            }
            document.mark_gone();
            if self.editor.prompt.is_none() {
                self.editor.prompt = Some(Prompt::FileDeleted { doc: doc_id, after: AfterSave::Dismiss });
            }
        }
        true
    }

    /// Reloads a clean document from disk, keeping the reading anchor.
    pub(crate) fn reload_document(&mut self, doc: DocumentId, path: PathBuf) {
        let document = self.shared.docs().get(doc);
        if let Some(document) = document
            && document.is_dirty()
        {
            if self.editor.prompt.is_none() {
                self.flush_all_drafts();
                self.editor.prompt = Some(Prompt::DiskChanged { doc });
            }
            return;
        }
        self.start_reload(doc, path, ReloadIntent::Replace);
    }

    /// Starts the load and queues the anchor restore; no dirty check.
    fn start_reload(&mut self, doc: DocumentId, path: PathBuf, intent: ReloadIntent) {
        let compare = intent == ReloadIntent::Compare;
        if compare {
            self.editor.compare_reloads.insert(doc);
        } else {
            self.editor.compare_reloads.remove(&doc);
        }
        if intent == ReloadIntent::ReopenCheck {
            self.editor.reopen_checks.insert(doc);
        } else {
            self.editor.reopen_checks.remove(&doc);
        }
        let anchor = self
            .tabs
            .iter()
            .find(|tab| tab.document_id() == Some(doc))
            .map(|tab| tab.view.anchor.clone())
            .unwrap_or_default();
        // A watcher read is only restored once it proves to hold different
        // text; a `touch` must not move the page (R-21). A reopen check does
        // the same, so the anchor is queued by its completion when the text
        // really changed (E-15).
        if !compare && intent != ReloadIntent::ReopenCheck && self.active_document_id() == Some(doc) {
            self.begin_restore(&anchor);
        }
        self.start_load(doc, path);
        // Reloads keep the old article (and the editor field) visible while
        // the new text is read; only a first load shows the loading card.
        if let Some(document) = self.shared.docs().get(doc) {
            *document.status.borrow_mut() = DocStatus::Ready;
        }
    }

    /// E-15: a same-file reopen checks the disk.
    ///
    /// The answer travels the ordinary reload path with [`ReloadIntent::ReopenCheck`]:
    /// identical text only refreshes the saved metadata, different text reloads
    /// a clean view with its reading anchor restored, and a dirty buffer is
    /// never read over — its tabs are marked changed and the existing
    /// file-changed question asks, exactly as the watcher does (R-22).
    pub(crate) fn check_open_document(&mut self, doc: DocumentId) {
        let Some(document) = self.shared.docs().get(doc) else { return };
        // A loading, deleted or failed document has no baseline to compare.
        if !matches!(*document.status.borrow(), DocStatus::Ready) {
            return;
        }
        let Some(path) = document.key.borrow().path().map(Path::to_path_buf) else { return };
        self.start_reload(doc, path, ReloadIntent::ReopenCheck);
    }

    /// Reloads after the user chose Reload for a dirty buffer: the draft is
    /// dropped and the buffer is replaced by the file (E-10/E-15).
    pub(crate) fn force_reload(&mut self, doc: DocumentId, path: PathBuf) {
        if let Some(document) = self.shared.docs().get(doc)
            && let Some(key) = draft_key(&document)
        {
            self.shared.queue_store(StorePatch::RemoveDraft(key));
        }
        for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(doc)) {
            tab.view.draft_written = false;
            tab.view.draft_pending = false;
            tab.view.disk_changed = false;
        }
        self.start_reload(doc, path, ReloadIntent::Replace);
    }

    /// `Ctrl+R`/F5: reload the active file document (R-20).
    ///
    /// In edit mode a dirty buffer is left alone (the watcher's warning, R-22,
    /// is the only path that asks); pasted text has no file and does nothing.
    pub(crate) fn request_reload(&mut self, _tx: &mut Tx) {
        let Some(doc) = self.active_document_id() else { return };
        let Some(document) = self.shared.docs().get(doc) else { return };
        let Some(path) = document.key.borrow().path().map(Path::to_path_buf) else { return };
        if document.is_dirty() || self.tabs.iter().any(|tab| tab.document_id() == Some(doc) && tab.view.dirty) {
            return;
        }
        self.reload_document(doc, path);
    }

    /// Asks the worker whether a draft exists for a file document (E-17).
    pub(crate) fn request_draft_check(&mut self, doc: DocumentId) {
        if self.shared.is_headless() {
            return;
        }
        let Some(document) = self.shared.docs().get(doc) else { return };
        let Some(key) = draft_key(&document) else { return };
        if !self.shared.mark_draft_checked(doc) {
            return;
        }
        self.shared.jobs().load_draft(self.worker().clone(), doc, key, self.shared.dirs().clone());
    }

    /// Handles a draft read: identical drafts vanish, differing ones are offered.
    pub(crate) fn handle_draft_loaded(
        &mut self,
        doc: DocumentId,
        key: String,
        draft: Option<Draft>,
        issue: Option<lmdv_io::store::DraftIssue>,
    ) -> bool {
        // A newer version's or another app's draft stays on disk (OI-169).
        if let Some(issue) = issue {
            self.shared.protect_drafts(std::slice::from_ref(&issue));
        }
        let Some(draft) = draft else { return false };
        let Some(document) = self.shared.docs().get(doc) else { return false };
        if self.tabs.iter().any(|tab| tab.document_id() == Some(doc) && tab.view.editor.is_some()) {
            return false;
        }
        // E-17: another window holding the edit lease owns the buffer; its
        // draft (if any) belongs to that editing session, not to a restore
        // offer here. The shared lease is the cross-window check the original
        // needed a BroadcastChannel for.
        if document.lease_holder().is_some() {
            return false;
        }
        let disk = document.text().text().to_owned();
        if draft.text == disk {
            self.shared.queue_store(StorePatch::RemoveDraft(key));
            return false;
        }
        if self.editor.prompt.is_some() {
            return false;
        }
        self.editor.prompt = Some(Prompt::Draft { doc, key, draft });
        true
    }

    /// Restores a draft into edit mode without touching the file (E-18).
    fn restore_draft(&mut self, doc: DocumentId, key: String, draft: Draft, tx: &mut Tx) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.document_id() == Some(doc)) {
            self.switch_to_index(index);
        }
        if !self.enter_editor(Some(draft.text), tx) {
            self.editor.prompt = None;
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.view.draft_pending = true;
            tab.view.draft_due = None;
            tab.view.draft_written = true;
        }
        // The draft stays as a backup until the next save or a discard.
        let _ = key;
        self.editor.prompt = None;
    }

    /// The tab-close guard: only the last view of a dirty document asks
    /// (E-13/T-05). Another window's view keeps the buffer and its draft.
    pub(crate) fn guard_close_tab(&mut self, id: TabId) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else { return false };
        if self.editor.quit.saving {
            // The aggregate quit is already saving; its tabs stay until it ends.
            return true;
        }
        let Some(document) = self.tabs[index].document().cloned() else { return false };
        if !document.is_dirty() {
            return false;
        }
        // Another window still shows it: this view may close silently.
        if !self.is_last_view(&document) {
            return false;
        }
        self.flush_active_draft();
        self.editor.prompt = Some(Prompt::CloseTab { tab: id, doc: document.id });
        self.sync_registry();
        true
    }

    /// Quits the application after one aggregate question for every window.
    ///
    /// Dirty and pasted documents from all windows are named in this window's
    /// dialog, deduplicated by document, and saved in turn. A question already
    /// open elsewhere is never duplicated: it is brought forward instead.
    pub(crate) fn request_quit(&mut self) {
        if self.editor.prompt.is_some() || self.editor.quit.saving {
            return;
        }
        if let Some(other) = self.shared.window_with_prompt(Some(self.id())) {
            self.shared.focus_window(other);
            self.notice(lmdv_i18n::t!("msg.otherWindowAsking"));
            return;
        }
        let docs = self.quit_documents();
        if docs.is_empty() {
            self.shared.set_quitting();
            self.ui.quit_requested = true;
            return;
        }
        self.flush_all_drafts();
        // T-25: while the quit question is open, launches route elsewhere.
        self.arm_quit_prompt(docs);
    }

    /// Every document a quit must name: the registry's union across windows
    /// plus this window's own tabs, so a tab opened or edited in the current
    /// frame is never missed.
    fn quit_documents(&self) -> Vec<CloseDoc> {
        let mut docs = self.shared.close_documents();
        for tab in &self.tabs {
            let Some(document) = tab.document() else { continue };
            if docs.iter().any(|doc| doc.id == document.id) {
                continue;
            }
            let pasted = matches!(*document.key.borrow(), DocumentKey::Untitled(_));
            let dirty = document.is_dirty();
            if pasted || dirty {
                docs.push(CloseDoc { id: document.id, title: close_title(document), pasted, dirty });
            }
        }
        docs.sort_by_key(|doc| doc.id);
        docs
    }

    /// Issues the close the prompt resolved to.
    fn issue_close(&mut self) {
        self.shared.set_window_closing(self.id(), true);
        self.ui.close_requested = true;
    }

    /// The window-close guard: only documents this window would take with it
    /// are named, in one dialog (T-24).
    pub(crate) fn guard_close_window(&mut self) -> bool {
        if self.editor.prompt.is_some() || self.editor.quit.saving {
            // T-25: one question at a time; a second close changes nothing.
            // A quit already saving is this window's close.
            return true;
        }
        if self.close_documents().is_empty() {
            return false;
        }
        self.flush_all_drafts();
        // T-25: while the close question is open, launches route elsewhere.
        self.shared.set_window_closing(self.id(), true);
        self.editor.prompt = Some(Prompt::CloseWindow);
        self.sync_registry();
        true
    }

    /// Every document this window's close question names, for the dialog.
    pub fn dirty_document_names(&self) -> Vec<String> {
        self.close_documents().into_iter().filter(|doc| doc.dirty).map(|doc| doc.title).collect()
    }

    /// The active save's checked flash, if any.
    pub fn saved_flash(&self) -> bool {
        self.editor.saved_flash_until.is_some()
    }

    /// The open decision, when there is one.
    pub fn prompt(&self) -> Option<&Prompt> {
        self.editor.prompt.as_ref()
    }

    /// The primary dialog action: Save / Overwrite / Reload / Restore / Save
    /// again / Save all.
    pub(crate) fn prompt_primary(&mut self, tx: &mut Tx) {
        let Some(prompt) = self.editor.prompt.clone() else { return };
        match prompt {
            Prompt::DiscardEdit { doc } => {
                self.editor.prompt = None;
                self.request_save(doc, AfterSave::LeaveEditor, tx);
            }
            Prompt::CloseTab { tab, doc } => {
                self.editor.prompt = None;
                self.request_save(doc, AfterSave::CloseTab(tab), tx);
            }
            Prompt::CloseWindow => {
                self.editor.prompt = None;
                self.editor.close_on_clean = true;
                // Pasted text has no file: Save all asks for a path even when
                // it was never edited (T-24).
                let docs: Vec<DocumentId> = self.close_documents().into_iter().map(|doc| doc.id).collect();
                if docs.is_empty() {
                    self.issue_close();
                    return;
                }
                for doc in docs {
                    self.editor.close_pending.insert(doc);
                    if let Some(after) = self.editor.after_save.get_mut(&doc) {
                        // A save already in flight is this document's save.
                        *after = AfterSave::CloseWindow;
                    } else {
                        self.request_save(doc, AfterSave::CloseWindow, tx);
                    }
                }
            }
            Prompt::Quit => {
                self.editor.prompt = None;
                let queue: VecDeque<DocumentId> = self.editor.quit.docs.iter().copied().collect();
                self.editor.quit.queue = queue;
                self.editor.quit.saving = true;
                self.save_next_quit_document(tx);
            }
            Prompt::SaveConflict { doc, after } => {
                self.editor.prompt = None;
                let path = self
                    .shared
                    .docs()
                    .get(doc)
                    .and_then(|document| document.key.borrow().path().map(Path::to_path_buf));
                if let Some(path) = path {
                    self.start_save(doc, path, None, false, after);
                }
            }
            Prompt::TaskConflict { edit } => {
                self.editor.prompt = None;
                self.resolve_task_conflict(edit);
            }
            Prompt::FileDeleted { doc, after } => {
                self.editor.prompt = None;
                let path = self
                    .shared
                    .docs()
                    .get(doc)
                    .and_then(|document| document.key.borrow().path().map(Path::to_path_buf));
                if let Some(path) = path {
                    self.start_save(doc, path, None, true, after);
                }
            }
            Prompt::DiskChanged { doc } => {
                self.editor.prompt = None;
                let path = self
                    .shared
                    .docs()
                    .get(doc)
                    .and_then(|document| document.key.borrow().path().map(Path::to_path_buf));
                if let Some(path) = path {
                    self.force_reload(doc, path);
                }
            }
            Prompt::Draft { doc, key, draft } => self.restore_draft(doc, key, draft, tx),
            Prompt::LeaseHeld { .. } => {
                self.editor.prompt = None;
            }
        }
    }

    /// The secondary action: Don’t save / Keep theirs / Keep mine / Discard.
    pub(crate) fn prompt_secondary(&mut self, _tx: &mut Tx) {
        let Some(prompt) = self.editor.prompt.clone() else { return };
        match prompt {
            Prompt::DiscardEdit { .. } => {
                self.prompt_discard_edit(&mut Tx::default());
            }
            Prompt::CloseTab { tab, .. } => {
                self.editor.prompt = None;
                self.discard_and_close_tab(tab);
            }
            Prompt::CloseWindow => {
                self.editor.prompt = None;
                self.editor.close_on_clean = false;
                self.editor.close_pending.clear();
                let docs: Vec<DocumentId> = self.close_documents().into_iter().map(|doc| doc.id).collect();
                self.discard_documents(&docs, false);
                self.issue_close();
            }
            Prompt::Quit => {
                self.editor.prompt = None;
                let docs: Vec<DocumentId> = std::mem::take(&mut self.editor.quit.docs);
                self.discard_documents(&docs, true);
                self.editor.quit.clear();
                self.shared.set_quitting();
                self.ui.quit_requested = true;
            }
            Prompt::SaveConflict { .. } => {
                self.editor.prompt = None;
                self.abandon_close();
                self.notice(lmdv_i18n::t!("msg.keptDiskDirty"));
            }
            Prompt::TaskConflict { edit } => {
                self.editor.prompt = None;
                self.keep_task_theirs(edit);
            }
            Prompt::FileDeleted { .. } => {
                self.editor.prompt = None;
                self.abandon_close();
                self.notice(lmdv_i18n::t!("msg.keptDeletedBuffer"));
            }
            Prompt::DiskChanged { doc } => {
                self.editor.prompt = None;
                for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(doc)) {
                    tab.view.disk_changed = false;
                }
            }
            Prompt::Draft { key, .. } => {
                self.shared.queue_store(StorePatch::RemoveDraft(key));
                self.editor.prompt = None;
            }
            Prompt::LeaseHeld { .. } => {
                self.editor.prompt = None;
            }
        }
    }

    /// Cancel: nothing changes. Dismissing a draft offer deletes the draft,
    /// as the original does (E-17).
    pub(crate) fn prompt_cancel(&mut self) {
        if let Some(Prompt::Draft { key, .. }) = self.editor.prompt.take() {
            self.shared.queue_store(StorePatch::RemoveDraft(key));
        }
        self.editor.prompt = None;
        // T-23: Cancel stops a close-others/right sequence.
        self.cancel_close_batch();
        self.editor.close_on_clean = false;
        self.editor.close_pending.clear();
        self.editor.quit.clear();
        // A cancelled close prompt makes this window a routing target again.
        self.shared.set_window_closing(self.id(), false);
        self.sync_registry();
    }

    /// Closes a tab without the dirty guard (after Save or Don't save).
    pub(crate) fn close_tab_unchecked(&mut self, tab: TabId) -> bool {
        let closed = self.close_tab(tab) == crate::window::CloseOutcome::Closed;
        if closed {
            self.sync_editor_panes();
        }
        closed
    }

    fn discard_and_close_tab(&mut self, tab: TabId) {
        let doc = self.tabs.iter().find(|candidate| candidate.id == tab).and_then(Tab::document).cloned();
        if let Some(document) = &doc {
            // Another window still shows the document: its buffer, lease and
            // draft stay exactly as they are.
            if self.is_last_view(document)
                && let Some(key) = draft_key(document)
            {
                self.shared.queue_store(StorePatch::RemoveDraft(key));
            }
            document.release_edit(EditLease { window: self.id(), tab });
        }
        if let Some(tab) = self.tabs.iter_mut().find(|candidate| candidate.id == tab) {
            tab.view.dirty = false;
            tab.view.draft_written = false;
            tab.view.draft_pending = false;
        }
        self.close_tab_unchecked(tab);
        let active = self.active_tab().and_then(Tab::document).cloned();
        if let Some(document) = active {
            self.sync_tab_dirty(&document);
        }
    }

    /// Updates every tab's cached dirty flag for `document`.
    pub(crate) fn sync_tab_dirty(&mut self, document: &Document) {
        let dirty = document.is_dirty();
        for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(document.id)) {
            tab.view.dirty = dirty;
        }
    }

    /// Refreshes titles after a mode, buffered-name or dirty change.
    pub(crate) fn sync_editor_titles(&mut self) {
        for tab in &mut self.tabs {
            tab.refresh_title();
        }
    }

    /// Renames this window's Back/Forward places from one key to another.
    ///
    /// File places are keyed by path; pasted text's are keyed `pasted:<id>`, so
    /// a Save as passes that key rather than a path (E-11).
    fn rename_history_keys(&mut self, from: &str, to: &Path) {
        let to = to.to_string_lossy();
        for tab in &mut self.tabs {
            tab.view.nav.rename_path(from, &to);
        }
    }

    /// Tells the other windows that a Save as moved `doc` to a new path
    /// (OI-173), so their Back/Forward places and tab titles follow it.
    ///
    /// Positions and recents live on [`crate::shared::AppShared`] and were
    /// renamed once by the saving window; a watch is process-wide too.
    fn broadcast_document_rename(&self, doc: DocumentId, from: &Path, to: &Path) {
        self.broadcast_window_rename(doc, from, to);
    }

    /// The draft key of a document: its path, or `pasted:<id>` (E-16/E-39).
    pub fn draft_key_of(document: &Document) -> Option<String> {
        draft_key(document)
    }
}

/// Writes or removes one tab's draft according to its state.
///
/// A tab that reads clean never removes a draft while the shared document is
/// still dirty: another window may be showing that edit, and the draft is its
/// crash backup (P4-02a).
fn flush_tab_draft(shared: &crate::shared::AppShared, tab: &mut Tab) {
    let Some(document) = tab.document().cloned() else { return };
    let Some(editor) = tab.view.editor.as_mut() else { return };
    editor.flush_blocks();
    let Some(key) = draft_key(&document) else { return };
    // A quit already knows every document's fate (saved or discarded); a
    // second window's drop must not resurrect a discarded draft (T-24). It may
    // still clean up the draft of a document that is back at its saved text.
    if shared.is_quitting() {
        if tab.view.draft_written && !document.is_dirty() {
            shared.queue_store(StorePatch::RemoveDraft(key));
            tab.view.draft_written = false;
        }
        tab.view.draft_pending = false;
        return;
    }
    let dirty = tab.view.dirty && document.is_dirty();
    if !dirty {
        if tab.view.draft_written && !document.is_dirty() {
            shared.queue_store(StorePatch::RemoveDraft(key));
            tab.view.draft_written = false;
        }
        tab.view.draft_pending = false;
        return;
    }
    let base = tab.view.edit_base.clone().unwrap_or_default();
    let draft = Draft::new(editor.text().to_owned(), base, crate::shared::unix_millis());
    shared.queue_store(StorePatch::Draft { key, draft });
    tab.view.draft_written = true;
    tab.view.draft_pending = false;
}

/// The store key for a document.
pub fn draft_key(document: &Document) -> Option<String> {
    match &*document.key.borrow() {
        DocumentKey::File(path) => Some(draft_key_for_path(path)),
        DocumentKey::Untitled(paste) => Some(format!("pasted:{}", paste.0)),
        DocumentKey::Start => None,
    }
}

fn draft_key_for_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The common prefix/suffix difference between two texts, on char boundaries.
pub(crate) fn change_range(old: &str, new: &str) -> (usize, usize, usize) {
    let mut start = 0;
    let max = old.len().min(new.len());
    while start < max && old.as_bytes()[start] == new.as_bytes()[start] {
        start += 1;
    }
    while start > 0 && (!old.is_char_boundary(start) || !new.is_char_boundary(start)) {
        start -= 1;
    }
    let mut old_end = old.len();
    let mut new_end = new.len();
    while old_end > start && new_end > start && old.as_bytes()[old_end - 1] == new.as_bytes()[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    while (old_end < old.len() && !old.is_char_boundary(old_end))
        || (new_end < new.len() && !new.is_char_boundary(new_end))
    {
        old_end += 1;
        new_end += 1;
    }
    (start, old_end, new_end)
}

/// The folder a Save as starts in (E-11).
///
/// A file saves beside itself. Pasted text has no folder of its own: it starts
/// in the file browser's folder — the paste home — and in the user's Documents
/// folder while the pane has no folder yet.
pub fn save_as_folder_for(
    document_path: Option<&Path>,
    pane_root: Option<&Path>,
    documents: Option<&Path>,
) -> Option<PathBuf> {
    match document_path {
        Some(path) => path.parent().map(Path::to_path_buf),
        None => pane_root.or(documents).map(Path::to_path_buf),
    }
}

/// The Documents folder Save as falls back to for pasted text (E-11).
///
/// `lmdv-io` resolves the XDG roots but exposes no documents folder yet, so the
/// plain home spelling stands in; a `dirs::document_dir()` helper there would
/// follow a localized `user-dirs.dirs` instead (reported as an io follow-up).
fn documents_folder() -> Option<PathBuf> {
    lmdv_io::documents_dir()
}

/// A filesystem-safe default name for Save as (E-11).
pub fn suggested_file_name(title: &str) -> String {
    let trimmed = title.trim();
    let mut name: String = trimmed
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '-',
            ch if ch.is_control() => '-',
            ch => ch,
        })
        .collect();
    name = name.trim_matches(['.', ' ']).to_owned();
    if name.is_empty() {
        name = "Untitled".to_owned();
    }
    if !name.to_ascii_lowercase().ends_with(".md") {
        name.push_str(".md");
    }
    name
}

fn is_ready(document: &Document) -> bool {
    matches!(*document.status.borrow(), DocStatus::Ready)
}
