//! Block-mode editor UI for large documents (ADR-016, OI-057).
//!
//! Above [`lmdv_edit::BLOCK_MODE_MAX_BYTES`] or
//! [`lmdv_edit::BLOCK_MODE_MAX_LINES`] the app renders one virtualized
//! [`rune::El::virtual_rows`] row per blank-line block instead of the single
//! source field. [`LargeFileMode::Block`] forces the layout for every document;
//! [`LargeFileMode::Auto`] uses the model's measured threshold. The choice is
//! made once, when the edit session starts, and never flips mid-session.
//!
//! Text still reaches the shared document through
//! [`WindowState::editor_changed`](crate::window::WindowState): a block field's
//! change is committed into the model's block journal first, so undo, drafts,
//! saving and the live preview keep working exactly as in single-field mode.
//! `Ctrl+Z`/`Ctrl+Y` are intercepted on the field and walk the global journal,
//! which spans blocks (E-40).
//!
//! Blank-line layout is kept true while typing: Enter twice inside a block
//! splits it at the new blank line and a boundary Backspace/Delete merges a
//! block with its neighbour, both through the model's structural operations so
//! no two fields ever own overlapping source.

use lmdv_core::LargeFileMode;
use lmdv_edit::{BlockEditKind, EditorSession};
use rune::{Key, KeyEvent, TextEdit, Tx};

use crate::window::WindowState;

/// The key prefix of one block row. `virtual_rows` gives the row this key, so
/// `Tx::focus("editor-block-{i}")` addresses the field itself.
pub const BLOCK_FIELD_PREFIX: &str = "editor-block-";
/// Row height estimated before a block has been measured.
pub const BLOCK_ROW_ESTIMATE: f32 = 40.0;
/// Extra space under a block field, on top of its own trailing blank line.
pub const BLOCK_ROW_GAP: f32 = 8.0;

/// The Rune key of block `index`.
pub fn block_field_key(index: usize) -> String {
    format!("{BLOCK_FIELD_PREFIX}{index}")
}

/// Applies the preference once, at the start of an edit session: `Block`
/// always enables the layout, `Auto` enables it only above the measured
/// threshold. Never called while the user is typing.
pub(crate) fn apply_large_file_mode(mode: LargeFileMode, session: &mut EditorSession) {
    match mode {
        LargeFileMode::Block => {
            session.enable_block_mode();
        }
        LargeFileMode::Auto => {
            session.block_mode();
        }
    }
}

/// Puts the model's focus on block `index` and clears the previously active
/// block's field, so only one field asks for the keyboard.
fn focus_field_in_model(session: &mut EditorSession, index: usize) {
    let previous = session.active_block();
    session.set_active_block(Some(index));
    if let Some(previous) = previous.filter(|&previous| previous != index)
        && let Some(field) = session.block_editor_mut().and_then(|blocks| blocks.block_mut(previous))
    {
        field.focused = false;
    }
    if let Some(field) = session.block_editor_mut().and_then(|blocks| blocks.block_mut(index)) {
        field.focused = true;
    }
}

/// Puts keyboard focus where the session can take it: the single field, or the
/// active block's field in block mode.
pub(crate) fn prime_editor_focus(session: &mut EditorSession) {
    if !session.block_mode_active() {
        session.edit.focused = true;
        return;
    }
    session.edit.focused = false;
    let len = session.block_editor().map_or(0, |blocks| blocks.blocks().len());
    if len == 0 {
        return;
    }
    let index = session.active_block().unwrap_or(0).min(len - 1);
    focus_field_in_model(session, index);
}

/// Whether `byte` in `text` sits inside a fenced code block. Blank lines inside
/// a fence are code, not paragraph separators, so Enter must not split there.
fn in_fence(text: &str, byte: usize) -> bool {
    let byte = byte.min(text.len());
    let line_start = text[..byte].rfind('\n').map_or(0, |offset| offset + 1);
    let mut fence: Option<(char, usize)> = None;
    for line in text[..line_start].split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some((marker, run)) = fence {
            let closing = trimmed.chars().take_while(|&ch| ch == marker).count();
            if closing >= run && trimmed[closing..].trim().is_empty() {
                fence = None;
            }
        } else if let Some(open) = fence_open(trimmed) {
            fence = Some(open);
        }
    }
    fence.is_some()
}

fn fence_open(trimmed: &str) -> Option<(char, usize)> {
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|&ch| ch == marker).count();
    (run >= 3).then_some((marker, run))
}

/// The journal kind a key starts: deletions coalesce with a typing run,
/// pastes are one step, everything else types.
fn kind_for(event: &KeyEvent) -> BlockEditKind {
    match &event.key {
        Key::Backspace | Key::Delete => BlockEditKind::Deleting,
        Key::Command('v' | 'x') => BlockEditKind::Discrete,
        _ => BlockEditKind::Typing,
    }
}

impl WindowState {
    /// The `on_edit` accessor of block `index`. Rune calls it for the focused
    /// field; when that field owns the focus, this is also where a pointer
    /// click lands, so the block becomes the active one for format, find,
    /// go-to-line and `/` commands.
    pub(crate) fn active_block_edit(&mut self, index: usize) -> &mut TextEdit {
        let active = self.active;
        let tabs = &mut self.tabs;
        if let Some(tab) = tabs.get_mut(active)
            && let Some(editor) = tab.view.editor.as_mut()
        {
            let focused =
                editor.block_editor().and_then(|blocks| blocks.block(index)).is_some_and(|block| block.edit.focused);
            if focused {
                editor.set_active_block(Some(index));
            }
            if let Some(field) = editor.block_editor_mut().and_then(|blocks| blocks.block_mut(index)) {
                return field;
            }
        }
        &mut self.editor.scratch
    }

    /// A block field changed through its `on_edit` binding: make it the active
    /// block and sync the document, preview, drafts and dirty flag. When the
    /// commit normalizes the blank-line layout (OI-089), focus follows the
    /// caret into the field that now owns it.
    pub(crate) fn block_changed(&mut self, index: usize, tx: &mut Tx) {
        {
            let Some(tab) = self.active_tab_mut() else { return };
            let Some(editor) = tab.view.editor.as_mut() else { return };
            editor.set_active_block(Some(index));
        }
        self.editor_changed();
        let active = self.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(EditorSession::active_block);
        if let Some(active) = active.filter(|&active| active != index) {
            self.focus_block(active, tx);
            tx.instant();
        }
    }

    /// The key handler of a block field, before Rune's editor. Returns true when
    /// the app consumed the key.
    pub(crate) fn block_key(&mut self, index: usize, event: &KeyEvent, tx: &mut Tx) -> bool {
        // Any key other than Tab cancels an armed Escape: the leave gesture is
        // Escape followed by Tab, not Escape and then anything else.
        if event.key != Key::Tab && event.key != Key::Escape {
            self.editor.last_escape_at = None;
        }
        self.editor.block_kind = kind_for(event);
        if self.block_history(event, tx) {
            return true;
        }
        // E-33: Tab indents the active block only; it never moves focus to
        // another block. After Escape, the same Tab leaves the editor instead.
        if crate::indent::plain_tab(event) {
            if self.escape_then_tab(tx) {
                return true;
            }
            return self.block_tab(index, event.mods.shift);
        }
        let plain = !event.mods.cmd && !event.mods.ctrl && !event.mods.alt && !event.mods.shift;
        if event.key == Key::Enter && plain {
            return self.block_enter(index, tx);
        }
        if event.key == Key::Backspace
            && plain
            && index > 0
            && let Some((start, end, _)) = self.block_caret(index)
            && start == 0
            && end == 0
        {
            return self.block_merge(index - 1, tx);
        }
        if event.key == Key::Delete
            && plain
            && self.block_count() > index + 1
            && let Some((start, end, len)) = self.block_caret(index)
            && start == len
            && end == len
        {
            return self.block_merge(index, tx);
        }
        false
    }

    /// The global journal's undo/redo. It spans blocks, so the per-field Rune
    /// histories are never consulted in block mode. The session restores the
    /// structural caret and active block (OI-090); focus follows that field.
    pub(crate) fn block_history(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if event.mods.alt {
            return false;
        }
        let undo = event.is_command('z') && !event.mods.shift;
        let redo = (event.is_command('z') && event.mods.shift) || (event.is_command('y') && !event.mods.shift);
        if !undo && !redo {
            return false;
        }
        let changed = {
            let Some(tab) = self.active_tab_mut() else { return true };
            let Some(editor) = tab.view.editor.as_mut() else { return true };
            if redo { editor.redo() } else { editor.undo() }
        };
        // An empty journal still consumes the chord: letting Rune's field undo
        // it would change the field without the source, leaving two owners for
        // the same range.
        if changed {
            let len = self.block_count();
            let index = self
                .active_tab()
                .and_then(|tab| tab.view.editor.as_ref())
                .and_then(EditorSession::active_block)
                .unwrap_or(0)
                .min(len.saturating_sub(1));
            if let Some(editor) = self.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()) {
                editor.set_active_block(Some(index));
            }
            self.editor_changed();
            self.focus_block(index, tx);
            tx.instant();
        }
        true
    }

    /// Tab/Shift+Tab in block mode: indent or outdent the selected lines of
    /// `index` only, as one journal step. Always consumes Tab, so focus stays
    /// in the field (E-33).
    fn block_tab(&mut self, index: usize, outdent: bool) -> bool {
        let changed = {
            let Some(editor) = self.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()) else { return true };
            editor.set_active_block(Some(index));
            editor
                .apply_block(index, BlockEditKind::Discrete, |edit| crate::indent::indent_with_tab(edit, outdent))
                .unwrap_or(false)
        };
        if changed {
            self.editor_changed();
        }
        true
    }

    /// Enter inside a block: insert the newline, commit it (the commit moves a
    /// caret that landed after a blank line into the canonical new field,
    /// OI-089), and split a blank-only field the layout rule would otherwise
    /// keep at end of file. The caret follows the text into the right field.
    fn block_enter(&mut self, index: usize, tx: &mut Tx) -> bool {
        {
            let Some(tab) = self.active_tab_mut() else { return false };
            let Some(editor) = tab.view.editor.as_mut() else { return false };
            editor.set_active_block(Some(index));
            let Some(field) = editor.block_editor_mut().and_then(|blocks| blocks.block_mut(index)) else {
                return false;
            };
            field.insert("\n");
            editor.commit_block_as(index, BlockEditKind::Typing);
        }
        let target = {
            let Some(editor) = self.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()) else { return false };
            let caret = editor.selection().start;
            let Some(blocks) = editor.block_editor() else { return false };
            let Some(block_index) = blocks.block_index_at(caret) else { return false };
            let Some(block) = blocks.block(block_index) else { return false };
            let local = caret.saturating_sub(block.range.start);
            let text = &block.edit.text;
            let blank_only = local >= 2
                && local < text.len()
                && text.as_bytes()[local - 2..local] == *b"\n\n"
                && !in_fence(text, local);
            if blank_only && editor.split_block(block_index, local) {
                if let Some(field) = editor.block_editor_mut().and_then(|blocks| blocks.block_mut(block_index + 1)) {
                    field.set_selection(0, 0);
                }
                editor.set_active_block(Some(block_index + 1));
                block_index + 1
            } else {
                editor.set_active_block(Some(block_index));
                block_index
            }
        };
        self.editor_changed();
        self.focus_block(target, tx);
        tx.instant();
        true
    }

    /// Merges block `left` with its neighbour, keeping the caret at the former
    /// boundary and the text whole. One journal step, undoable.
    fn block_merge(&mut self, left: usize, tx: &mut Tx) -> bool {
        let boundary = {
            let Some(editor) = self.active_tab().and_then(|tab| tab.view.editor.as_ref()) else { return false };
            let Some(block) = editor.block_editor().and_then(|blocks| blocks.block(left)) else { return false };
            block.edit.text.len()
        };
        let merged = {
            let Some(tab) = self.active_tab_mut() else { return false };
            let Some(editor) = tab.view.editor.as_mut() else { return false };
            editor.merge_blocks(left)
        };
        if !merged {
            return false;
        }
        {
            let Some(tab) = self.active_tab_mut() else { return false };
            let Some(editor) = tab.view.editor.as_mut() else { return false };
            focus_field_in_model(editor, left);
            if let Some(field) = editor.block_editor_mut().and_then(|blocks| blocks.block_mut(left)) {
                field.set_selection(boundary.min(field.text.len()), boundary.min(field.text.len()));
            }
        }
        self.editor_changed();
        self.focus_block(left, tx);
        tx.instant();
        true
    }

    fn block_caret(&self, index: usize) -> Option<(usize, usize, usize)> {
        let editor = self.active_tab()?.view.editor.as_ref()?;
        let block = editor.block_editor()?.block(index)?;
        let (from, to) = block.edit.range();
        Some((from, to, block.edit.text.len()))
    }

    /// How many block fields the active editor has, or 0 outside block mode.
    pub(crate) fn block_count(&self) -> usize {
        self.active_tab()
            .and_then(|tab| tab.view.editor.as_ref())
            .and_then(EditorSession::block_editor)
            .map_or(0, |blocks| blocks.blocks().len())
    }

    /// Gives `index` the focus (clamped to the blocks that exist), revealing
    /// its row first: a block deep in a long document may not be built yet.
    pub(crate) fn focus_block(&mut self, index: usize, tx: &mut Tx) {
        let key = {
            let Some(tab) = self.active_tab_mut() else { return };
            let Some(editor) = tab.view.editor.as_mut() else { return };
            if !editor.block_mode_active() {
                return;
            }
            let len = editor.block_editor().map_or(0, |blocks| blocks.blocks().len());
            if len == 0 {
                return;
            }
            let index = index.min(len - 1);
            focus_field_in_model(editor, index);
            block_field_key(index)
        };
        tx.scroll_into_view(key.clone());
        tx.focus(key, false);
    }

    /// Returns the keyboard to the editor: the active block field in block
    /// mode, the single source field otherwise.
    pub(crate) fn focus_editor_field(&mut self, tx: &mut Tx) {
        let index = self
            .active_tab()
            .and_then(|tab| tab.view.editor.as_ref())
            .filter(|editor| editor.block_mode_active())
            .map(|editor| editor.active_block().unwrap_or(0));
        match index {
            Some(index) => self.focus_block(index, tx),
            None => tx.focus(crate::edit_ui::SOURCE_FIELD_KEY, false),
        }
    }
}

#[cfg(test)]
impl WindowState {
    /// Test seam for the block field the user would click.
    pub(crate) fn block_editor_mut_for_test(&mut self, index: usize) -> &mut TextEdit {
        let editor = self.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor");
        editor.block_editor_mut().and_then(|blocks| blocks.block_mut(index)).expect("block field")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_state_keeps_blank_lines_inside_code() {
        let text = "```\n\ncode\n\n```\n\nafter";
        assert!(in_fence(text, 4), "on the blank line after the opener");
        assert!(in_fence(text, 9), "inside the fence body");
        assert!(!in_fence(text, 15), "after the closing fence");
        assert!(!in_fence("no fence\n\n", 8));
        assert!(in_fence("~~~~js\n\n", 8), "tilde fences count too");
    }

    #[test]
    fn keys_map_to_journal_kinds() {
        let mut event = KeyEvent { key: Key::Backspace, mods: rune::Mods::default() };
        assert_eq!(kind_for(&event), BlockEditKind::Deleting);
        event.key = rune::Key::Command('v');
        assert_eq!(kind_for(&event), BlockEditKind::Discrete);
        event.key = Key::Text("a".to_owned());
        assert_eq!(kind_for(&event), BlockEditKind::Typing);
    }
}
