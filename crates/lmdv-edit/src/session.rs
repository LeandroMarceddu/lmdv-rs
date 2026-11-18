//! The editor session the app binds to one Rune [`TextEdit`] per tab.
//!
//! One session owns the canonical LF text, the selection, the line ending to
//! write, the find bar state, the syntax cache and the optional
//! [`BlockEditor`] for large documents. It never touches the filesystem: the
//! app applies [`EditorSession::commit_text`] to the shared `Document`, and
//! only an explicit save calls [`EditorSession::save_text`].
//!
//! Undo in single-field mode is Rune's `TextEdit` history. In block mode it is
//! the block journal, so `Ctrl+Z` walks back across blocks (ADR-016, E-40).

use std::ops::Range;
use std::time::Instant;

use lmdv_core::{Draft, Eol, FindMatch, FindState, normalize_lf, strip_bom};
use rune::Span;
use rune::TextEdit;
use rune_kit::syntax::SyntaxTheme;
use unicode_segmentation::UnicodeSegmentation;

use crate::block::{BlockEditKind, BlockEditor, block_ranges, should_use_block_mode};
use crate::find::{FindOptions, expand_replacement, matches_in};
use crate::syntax::SyntaxCache;

/// One tab's editor state.
#[derive(Clone, Debug)]
pub struct EditorSession {
    /// The Rune field for single-field mode. Public because `text_input` binds
    /// `&session.edit` directly; call [`EditorSession::note_edit`] after the
    /// field changed through `on_edit`/`on_input_key`.
    pub edit: TextEdit,
    /// Revision of the shared `Document` this session last observed.
    pub doc_revision: u64,
    /// Revision the session was loaded from, for a draft's base comparison.
    pub base_revision: u64,
    /// The file's line ending, applied only by [`EditorSession::save_text`].
    pub eol: Eol,
    /// The find/replace bar's query, options, hits and current index.
    pub find: FindState,
    /// Markdown highlighting cache for the single field.
    pub syntax: SyntaxCache,
    /// When the text last changed, for the app's draft debounce.
    pub last_edit_at: Option<Instant>,
    /// The block editor when the document is large or the user forced it.
    pub block_mode: Option<BlockEditor>,
    /// Local revision for the syntax cache; bumps on every text edit.
    revision: u64,
    /// Which block has the focus in block mode, for find and preview scroll.
    active_block: Option<usize>,
}

/// An opaque tab-switch snapshot: text, selection, history, find state, line
/// ending and the whole block journal.
#[derive(Clone, Debug)]
pub struct EditorSnapshot {
    edit: TextEdit,
    doc_revision: u64,
    base_revision: u64,
    eol: Eol,
    find: FindState,
    last_edit_at: Option<Instant>,
    block_mode: Option<BlockEditor>,
    revision: u64,
    active_block: Option<usize>,
}

impl EditorSession {
    /// Builds a session from LF or CRLF text. A UTF-8 BOM is stripped and never
    /// remembered for writing (E-09).
    pub fn new(text: &str, eol: Eol) -> Self {
        let (_, text) = strip_bom(text);
        let text = normalize_lf(text);
        Self {
            edit: TextEdit::new(text.as_ref()),
            doc_revision: 0,
            base_revision: 0,
            eol,
            find: FindState::default(),
            syntax: SyntaxCache::default(),
            last_edit_at: None,
            block_mode: None,
            revision: 0,
            active_block: None,
        }
    }

    /// The canonical LF text, from the block editor when it is active. Direct
    /// block-field edits appear after [`EditorSession::commit_block`].
    pub fn text(&self) -> &str {
        match &self.block_mode {
            Some(block) => block.text(),
            None => &self.edit.text,
        }
    }

    /// Replaces the whole buffer for a load, reload or restore. Discards undo
    /// history and the block layout, and strips a BOM.
    pub fn set_text(&mut self, text: &str) {
        let (_, text) = strip_bom(text);
        let text = normalize_lf(text);
        self.edit.replace_text(text.into_owned());
        self.block_mode = None;
        self.active_block = None;
        self.revision = self.revision.wrapping_add(1);
        self.find.matches.clear();
        self.find.sanitize();
        self.last_edit_at = None;
    }

    /// Replaces the whole buffer as one undoable edit (Select All + Paste).
    /// This is not the find bar's Replace All; see [`Self::replace_all_matches`].
    pub fn replace_all(&mut self, text: &str) -> bool {
        self.flush_blocks();
        let (_, text) = strip_bom(text);
        let text = normalize_lf(text);
        if text.as_ref() == self.text() {
            return false;
        }
        if self.block_mode.is_some() {
            let changed = self.block_mode.as_mut().is_some_and(|block| block.replace_all_source(text.as_ref()));
            if changed {
                self.active_block = None;
                self.note_edit();
            }
            return changed;
        }
        let old_len = self.edit.text.len();
        self.edit.undo_group(|edit| {
            edit.set_selection(0, old_len);
            edit.insert(text.as_ref());
        });
        self.note_edit();
        true
    }

    /// The current selection in whole-source byte coordinates.
    pub fn selection(&self) -> Range<usize> {
        if let Some(block_editor) = &self.block_mode {
            let index = self.active_block.unwrap_or(0);
            if let Some(block) = block_editor.block(index).or_else(|| block_editor.block(0)) {
                let (from, to) = block.edit.range();
                return block.range.start + from..block.range.start + to;
            }
            return 0..0;
        }
        let (from, to) = self.edit.range();
        from..to
    }

    /// Selects a whole-source range. In block mode only the part inside the
    /// block containing `range.start` is selected (cross-block selection is a
    /// documented substitute).
    pub fn set_selection(&mut self, range: Range<usize>) {
        let (from, to) = if range.start <= range.end { (range.start, range.end) } else { (range.end, range.start) };
        if self.block_mode.is_some() {
            let mut active = None;
            if let Some(block_editor) = self.block_mode.as_mut() {
                let len = block_editor.text().len();
                let from = from.min(len);
                let to = to.min(len);
                if let Some(index) = block_editor.block_index_at(from) {
                    let bounds = block_editor.block(index).map(|block| (block.range.start, block.range.end));
                    if let Some((start, end)) = bounds {
                        let local_from = from.saturating_sub(start);
                        let local_to = to.min(end).saturating_sub(start).max(local_from);
                        if let Some(edit) = block_editor.block_mut(index) {
                            edit.set_selection(local_from, local_to);
                            edit.break_history_group();
                        }
                        block_editor.break_typing_group();
                        active = Some(index);
                    }
                }
            }
            self.active_block = active;
            return;
        }
        self.edit.set_selection(from, to);
        self.edit.break_history_group();
    }

    /// Replaces the selection with `text`.
    pub fn insert(&mut self, text: &str) -> bool {
        let range = self.selection();
        self.replace_range(range, text)
    }

    /// Backspace: deletes the selection, or one grapheme before the caret.
    pub fn backspace(&mut self) -> bool {
        let range = self.selection();
        if !range.is_empty() {
            return self.replace_range(range, "");
        }
        if range.start == 0 {
            return false;
        }
        let text = self.text();
        let start = text[..range.start].grapheme_indices(true).next_back().map_or(0, |(offset, _)| offset);
        self.replace_range(start..range.start, "")
    }

    /// Delete: deletes the selection, or one grapheme after the caret.
    pub fn delete(&mut self) -> bool {
        let range = self.selection();
        if !range.is_empty() {
            return self.replace_range(range, "");
        }
        let end = {
            let text = self.text();
            text[range.start..].graphemes(true).next().map_or(range.start, |grapheme| range.start + grapheme.len())
        };
        if end == range.start {
            return false;
        }
        self.replace_range(range.start..end, "")
    }

    /// Replaces a whole-source range with LF-normalized `text`. Inside one
    /// block it edits that block; a range spanning blocks resets the layout as
    /// one undo step.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) -> bool {
        self.flush_blocks();
        let (_, text) = strip_bom(text);
        let text = normalize_lf(text);
        let range = {
            let current = self.text();
            let range = clamp_range(current, range);
            if &current[range.clone()] == text.as_ref() {
                return false;
            }
            range
        };
        if self.block_mode.is_some() {
            let changed =
                self.block_mode.as_mut().is_some_and(|block| block.replace_source(range.clone(), text.as_ref()));
            if changed {
                let caret = self.block_mode.as_ref().and_then(BlockEditor::caret_block);
                let index =
                    caret.or_else(|| self.block_mode.as_ref().and_then(|block| block.block_index_at(range.start)));
                self.active_block = index;
                self.note_edit();
            }
            return changed;
        }
        self.edit.set_selection(range.start, range.end);
        self.edit.insert(text.as_ref());
        self.note_edit();
        true
    }

    /// Undo one step, from the block journal in block mode. A structural step
    /// also moves the active block to where the restored caret is (OI-090).
    pub fn undo(&mut self) -> bool {
        if self.block_mode.is_some() {
            let (changed, caret) = match self.block_mode.as_mut() {
                Some(block) => (block.undo(), block.caret_block()),
                None => (false, None),
            };
            if changed {
                if let Some(caret) = caret {
                    self.active_block = Some(caret);
                } else {
                    self.clamp_active_block();
                }
                self.note_edit();
            }
            return changed;
        }
        let before = self.edit.text.clone();
        self.edit.undo();
        let changed = self.edit.text != before;
        if changed {
            self.note_edit();
        }
        changed
    }

    /// Redo one step, from the block journal in block mode, with the same
    /// caret and active-block restoration as [`Self::undo`].
    pub fn redo(&mut self) -> bool {
        if self.block_mode.is_some() {
            let (changed, caret) = match self.block_mode.as_mut() {
                Some(block) => (block.redo(), block.caret_block()),
                None => (false, None),
            };
            if changed {
                if let Some(caret) = caret {
                    self.active_block = Some(caret);
                } else {
                    self.clamp_active_block();
                }
                self.note_edit();
            }
            return changed;
        }
        let before = self.edit.text.clone();
        self.edit.redo();
        let changed = self.edit.text != before;
        if changed {
            self.note_edit();
        }
        changed
    }

    /// Ends the open typing run so the next edit is a new undo step.
    pub fn break_history_group(&mut self) {
        self.edit.break_history_group();
        if let Some(block) = self.block_mode.as_mut() {
            block.break_typing_group();
        }
    }

    /// A tab-switch snapshot.
    pub fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            edit: self.edit.clone(),
            doc_revision: self.doc_revision,
            base_revision: self.base_revision,
            eol: self.eol,
            find: self.find.clone(),
            last_edit_at: self.last_edit_at,
            block_mode: self.block_mode.clone(),
            revision: self.revision,
            active_block: self.active_block,
        }
    }

    /// Restores a snapshot, including its undo history and block journal.
    pub fn restore(&mut self, snapshot: EditorSnapshot) {
        self.edit = snapshot.edit;
        self.doc_revision = snapshot.doc_revision;
        self.base_revision = snapshot.base_revision;
        self.eol = snapshot.eol;
        self.find = snapshot.find;
        self.last_edit_at = snapshot.last_edit_at;
        self.block_mode = snapshot.block_mode;
        self.revision = snapshot.revision;
        self.active_block = snapshot.active_block;
        self.syntax = SyntaxCache::default();
    }

    /// The draft payload for this buffer (E-16/E-18). Call
    /// [`Self::flush_blocks`] first when a block field may be uncommitted.
    pub fn draft(&self, base: &str, at: u64) -> Draft {
        Draft::new(self.text(), base, at)
    }

    /// The LF text to apply to the shared `Document`. Call
    /// [`Self::flush_blocks`] first when a block field may be uncommitted.
    pub fn commit_text(&self) -> &str {
        self.text()
    }

    /// Commits direct block-field edits into the source before a save, draft
    /// or preview parse.
    pub fn flush_blocks(&mut self) {
        let changed = self.block_mode.as_mut().is_some_and(BlockEditor::commit_pending_edits);
        if changed {
            self.note_edit();
        }
    }

    /// The bytes to write: CRLF when the file used it, never a BOM (E-09).
    /// Call [`Self::flush_blocks`] first when a block field may be uncommitted.
    pub fn save_text(&self) -> String {
        let text = self.text();
        if self.eol == Eol::Lf || !text.contains('\n') {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len() + text.bytes().filter(|&byte| byte == b'\n').count());
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                out.push_str("\r\n");
            }
            out.push_str(part);
        }
        out
    }

    pub fn eol(&self) -> Eol {
        self.eol
    }

    pub fn set_eol(&mut self, eol: Eol) {
        self.eol = eol;
    }

    pub fn set_doc_revision(&mut self, revision: u64) {
        self.doc_revision = revision;
    }

    pub fn set_base_revision(&mut self, revision: u64) {
        self.base_revision = revision;
    }

    /// Number of LF lines, like `SourceBuffer`. In block mode this counts the
    /// fields' text, including uncommitted edits.
    pub fn line_count(&self) -> usize {
        match &self.block_mode {
            Some(block) => {
                1 + block
                    .blocks()
                    .iter()
                    .map(|block| block.edit.text.bytes().filter(|&byte| byte == b'\n').count())
                    .sum::<usize>()
            }
            None => 1 + self.edit.text.bytes().filter(|&byte| byte == b'\n').count(),
        }
    }

    /// Call after the bound `TextEdit` changed outside this session
    /// (`on_edit`, `on_input_key`): bumps the local revision and drops stale
    /// find hits.
    pub fn note_edit(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.find.matches.clear();
        self.find.sanitize();
        self.last_edit_at = Some(Instant::now());
    }

    /// The regex compile error for the current query, for the find bar to show.
    pub fn find_error(&self) -> Option<String> {
        crate::find::find_error(&self.find.query, &FindOptions::from_state(&self.find))
    }

    /// Recomputes `find.matches` from the query and options; returns the count.
    pub fn refresh_find(&mut self) -> usize {
        self.flush_blocks();
        let options = FindOptions::from_state(&self.find);
        let query = self.find.query.clone();
        let matches = {
            let text = self.text();
            let found = matches_in(text, &query, &options);
            let blocks = block_ranges(text);
            let mut line = 0;
            let mut cursor = 0;
            found
                .into_iter()
                .map(|hit| {
                    let start = hit.range.start;
                    line += text[cursor..start].bytes().filter(|&byte| byte == b'\n').count();
                    cursor = start;
                    let block = blocks
                        .partition_point(|range| range.start <= start)
                        .saturating_sub(1)
                        .min(blocks.len().saturating_sub(1));
                    FindMatch { source: hit.range, block, line }
                })
                .collect::<Vec<_>>()
        };
        self.find.matches = matches;
        self.find.sanitize();
        if let Some(index) = self.find.matches.iter().position(|found| found.source == self.selection()) {
            self.find.current = index;
        }
        self.find.matches.len()
    }

    /// Selects the next match (wrapping), or the first one after the caret. In
    /// block mode the app focuses [`Self::active_block`] afterwards.
    pub fn find_next(&mut self) -> bool {
        self.refresh_find();
        let selection = self.selection();
        let count = self.find.matches.len();
        if count == 0 {
            return false;
        }
        let index = if let Some(index) = self.find.matches.iter().position(|found| found.source == selection) {
            (index + 1) % count
        } else if let Some(index) = self
            .find
            .matches
            .iter()
            .position(|found| found.source.start <= selection.start && selection.end <= found.source.end)
        {
            index
        } else {
            self.find.matches.iter().position(|found| found.source.start >= selection.end).unwrap_or(0)
        };
        let range = self.find.matches[index].source.clone();
        self.find.current = index;
        self.set_selection(range);
        true
    }

    /// Selects the previous match (wrapping). In block mode the app focuses
    /// [`Self::active_block`] afterwards.
    pub fn find_prev(&mut self) -> bool {
        self.refresh_find();
        let selection = self.selection();
        let count = self.find.matches.len();
        if count == 0 {
            return false;
        }
        let index = if let Some(index) = self.find.matches.iter().position(|found| found.source == selection) {
            if count == 1 { 0 } else { (index + count - 1) % count }
        } else {
            self.find.matches.iter().rposition(|found| found.source.end <= selection.start).unwrap_or(count - 1)
        };
        let range = self.find.matches[index].source.clone();
        self.find.current = index;
        self.set_selection(range);
        true
    }

    /// Replaces the selected match (or the current hit) and advances; one undo
    /// step. Returns false when the query has no hits. In block mode the app
    /// focuses [`Self::active_block`] afterwards.
    pub fn replace_current(&mut self) -> bool {
        self.refresh_find();
        if self.find.matches.is_empty() {
            return false;
        }
        let selection = self.selection();
        let index = self
            .find
            .matches
            .iter()
            .position(|found| found.source == selection)
            .unwrap_or_else(|| self.find.current.min(self.find.matches.len() - 1));
        let range = self.find.matches[index].source.clone();
        let replacement = self.replacement_for(index);
        let inserted = normalize_lf(strip_bom(&replacement).1).len();
        if !self.replace_range(range.clone(), &replacement) {
            return self.find_next();
        }
        let caret = range.start + inserted;
        self.refresh_find();
        self.find.current = self.find.matches.iter().position(|found| found.source.start >= caret).unwrap_or(0);
        self.set_selection(caret..caret);
        true
    }

    /// Replaces every hit in one undo step; returns how many matched. In block
    /// mode the layout is rebuilt and no block keeps the focus.
    pub fn replace_all_matches(&mut self) -> usize {
        self.refresh_find();
        if self.find.matches.is_empty() {
            return 0;
        }
        let options = FindOptions::from_state(&self.find);
        let query = self.find.query.clone();
        let replace = self.find.replace.clone();
        let (found, old) = {
            let text = self.text();
            (matches_in(text, &query, &options), text.to_owned())
        };
        if found.is_empty() {
            return 0;
        }
        let mut edits = Vec::with_capacity(found.len());
        for hit in &found {
            let replacement = if options.regex { expand_replacement(&replace, &hit.groups) } else { replace.clone() };
            let replacement = normalize_lf(strip_bom(&replacement).1).into_owned();
            edits.push((hit.range.clone(), replacement));
        }
        let mut new = old.clone();
        for (range, replacement) in edits.iter().rev() {
            new.replace_range(range.clone(), replacement);
        }
        if new == old {
            self.find.current = 0;
            return edits.len();
        }
        let selection = map_selection_through(self.selection(), &edits);
        if self.block_mode.is_some() {
            let changed = self.block_mode.as_mut().is_some_and(|block| block.replace_all_source(&new));
            if changed {
                self.active_block = None;
                self.note_edit();
            }
        } else {
            let old_len = old.len();
            self.edit.undo_group(|edit| {
                edit.set_selection(0, old_len);
                edit.insert(&new);
            });
            self.note_edit();
        }
        self.set_selection(selection);
        self.refresh_find();
        self.find.current = 0;
        edits.len()
    }

    /// Enables block mode when the text is above the threshold; returns the
    /// editor. Below the threshold this returns the already-enabled editor, or
    /// `None` if the pref did not force it. Once enabled it stays enabled until
    /// [`Self::disable_block_mode`], so a shrinking document does not lose its
    /// journal.
    pub fn block_mode(&mut self) -> Option<&mut BlockEditor> {
        if self.block_mode.is_none() && should_use_block_mode(self.text()) {
            self.enable_block_mode();
        }
        self.block_mode.as_mut()
    }

    /// Forces block mode on, keeping the caret's block selected.
    pub fn enable_block_mode(&mut self) -> Option<&mut BlockEditor> {
        if self.block_mode.is_none() {
            let source = self.edit.text.clone();
            let (from, to) = self.edit.range();
            let mut editor = BlockEditor::new(&source);
            let mut active = None;
            if let Some(index) = editor.block_index_at(from) {
                let bounds = editor.block(index).map(|block| (block.range.start, block.range.end));
                if let Some((start, end)) = bounds
                    && let Some(edit) = editor.block_mut(index)
                {
                    edit.set_selection(from.saturating_sub(start), to.min(end).saturating_sub(start));
                }
                active = Some(index);
            }
            self.active_block = active;
            self.block_mode = Some(editor);
        }
        self.block_mode.as_mut()
    }

    /// Returns to single-field mode with the current block source, discarding
    /// the block journal.
    pub fn disable_block_mode(&mut self) -> bool {
        let Some(mut block) = self.block_mode.take() else {
            return false;
        };
        let last_edit_at = self.last_edit_at;
        let pending = block.commit_pending_edits();
        let selection = self
            .active_block
            .and_then(|index| block.block(index))
            .map(|block| {
                let (from, to) = block.edit.range();
                block.range.start + from..block.range.start + to
            })
            .unwrap_or(0..0);
        let text = block.text().to_owned();
        self.edit.replace_text(text);
        self.edit.set_selection(selection.start, selection.end);
        self.active_block = None;
        self.note_edit();
        if !pending {
            self.last_edit_at = last_edit_at;
        }
        true
    }

    pub fn block_mode_active(&self) -> bool {
        self.block_mode.is_some()
    }

    pub fn block_editor(&self) -> Option<&BlockEditor> {
        self.block_mode.as_ref()
    }

    pub fn block_editor_mut(&mut self) -> Option<&mut BlockEditor> {
        self.block_mode.as_mut()
    }

    /// The block with the focus in block mode.
    pub fn active_block(&self) -> Option<usize> {
        self.active_block
    }

    pub fn set_active_block(&mut self, index: Option<usize>) {
        self.active_block =
            index.filter(|&index| self.block_mode.as_ref().is_some_and(|block| index < block.blocks().len()));
    }

    /// Applies an edit to one block and records it in the global journal.
    pub fn apply_block<R>(
        &mut self,
        index: usize,
        kind: BlockEditKind,
        f: impl FnOnce(&mut TextEdit) -> R,
    ) -> Option<R> {
        let (result, caret) = match self.block_mode.as_mut() {
            Some(block) => (block.apply(index, kind, f), block.caret_block()),
            None => (None, None),
        };
        if result.is_some() {
            if let Some(caret) = caret {
                self.active_block = Some(caret);
            }
            self.note_edit();
        }
        result
    }

    /// Copies a directly edited block back into the source as a discrete edit.
    pub fn commit_block(&mut self, index: usize) -> bool {
        self.commit_block_as(index, BlockEditKind::Discrete)
    }

    /// Copies a directly edited block back with an explicit journal kind:
    /// `Typing`/`Deleting` from the field's key handler coalesce, `Discrete`
    /// starts one undo step. A commit that normalizes a blank-line boundary
    /// also moves the active block to where the caret landed (OI-089).
    pub fn commit_block_as(&mut self, index: usize, kind: BlockEditKind) -> bool {
        let (changed, caret) = match self.block_mode.as_mut() {
            Some(block) => (block.commit_block_as(index, kind), block.caret_block()),
            None => (false, None),
        };
        if changed {
            if let Some(caret) = caret {
                self.active_block = Some(caret);
            }
            self.note_edit();
        }
        changed
    }

    /// Splits a block at a line or blank-line boundary; one undo step.
    pub fn split_block(&mut self, index: usize, at: usize) -> bool {
        let (changed, caret) = match self.block_mode.as_mut() {
            Some(block) => (block.split_block(index, at), block.caret_block()),
            None => (false, None),
        };
        if changed {
            if let Some(caret) = caret {
                self.active_block = Some(caret);
            } else {
                self.clamp_active_block();
            }
            self.note_edit();
        }
        changed
    }

    /// Merges a block with the next one; one undo step.
    pub fn merge_blocks(&mut self, index: usize) -> bool {
        let (changed, caret) = match self.block_mode.as_mut() {
            Some(block) => (block.merge_blocks(index), block.caret_block()),
            None => (false, None),
        };
        if changed {
            if let Some(caret) = caret {
                self.active_block = Some(caret);
            } else {
                self.clamp_active_block();
            }
            self.note_edit();
        }
        changed
    }

    /// Cached Markdown highlighting for the single field. In block mode the
    /// app calls [`crate::syntax::syntax_spans`] per visible block instead.
    pub fn syntax_spans(&mut self, theme: SyntaxTheme, visible: Option<Range<usize>>) -> Vec<Span> {
        let revision = self.revision;
        self.syntax.spans(revision, &self.edit, theme, visible).to_vec()
    }

    fn clamp_active_block(&mut self) {
        let len = self.block_mode.as_ref().map_or(0, |block| block.blocks().len());
        if self.active_block.is_some_and(|index| index >= len) {
            self.active_block = None;
        }
    }

    fn replacement_for(&self, index: usize) -> String {
        if !self.find.regex {
            return self.find.replace.clone();
        }
        let options = FindOptions::from_state(&self.find);
        let query = self.find.query.clone();
        let found = {
            let text = self.text();
            matches_in(text, &query, &options)
        };
        found
            .get(index)
            .map(|hit| expand_replacement(&self.find.replace, &hit.groups))
            .unwrap_or_else(|| self.find.replace.clone())
    }
}

fn clamp_range(text: &str, range: Range<usize>) -> Range<usize> {
    let mut start = range.start.min(text.len());
    let mut end = range.end.min(text.len());
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    start = text.floor_char_boundary(start);
    end = text.floor_char_boundary(end);
    start..end
}

fn map_selection_through(selection: Range<usize>, edits: &[(Range<usize>, String)]) -> Range<usize> {
    map_offset(selection.start, edits)..map_offset(selection.end, edits)
}

fn map_offset(offset: usize, edits: &[(Range<usize>, String)]) -> usize {
    let mut delta = 0isize;
    for (range, replacement) in edits {
        if offset < range.start {
            break;
        }
        if offset < range.end {
            return (range.start as isize + delta + replacement.len() as isize).max(0) as usize;
        }
        delta += replacement.len() as isize - (range.end - range.start) as isize;
    }
    (offset as isize + delta).max(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_normalizes_crlf_strips_bom_and_keeps_eol() {
        let mut session = EditorSession::new("\u{feff}a\r\nb\r\n", Eol::CrLf);
        assert_eq!(session.text(), "a\nb\n");
        assert_eq!(session.eol(), Eol::CrLf);
        assert_eq!(session.save_text(), "a\r\nb\r\n");
        assert!(!session.save_text().starts_with('\u{feff}'));
        session.set_eol(Eol::Lf);
        assert_eq!(session.save_text(), "a\nb\n");
    }

    #[test]
    fn save_text_preserves_the_final_newline_and_never_adds_a_bom() {
        assert_eq!(EditorSession::new("a", Eol::CrLf).save_text(), "a");
        assert_eq!(EditorSession::new("a\n", Eol::CrLf).save_text(), "a\r\n");
        assert_eq!(EditorSession::new("a\n\n", Eol::CrLf).save_text(), "a\r\n\r\n");
        assert_eq!(EditorSession::new("a\rb\n", Eol::CrLf).save_text(), "a\rb\r\n");
    }

    #[test]
    fn insert_replaces_the_selection_and_can_be_undone() {
        let mut session = EditorSession::new("one two", Eol::Lf);
        session.set_selection(4..7);
        assert!(session.insert("three"));
        assert_eq!(session.text(), "one three");
        assert_eq!(session.selection(), 9..9);
        assert!(session.undo());
        assert_eq!(session.text(), "one two");
        assert!(session.redo());
        assert_eq!(session.text(), "one three");
    }

    #[test]
    fn backspace_and_delete_work_on_graphemes() {
        let mut session = EditorSession::new("ae\u{301}", Eol::Lf);
        assert!(session.backspace());
        assert_eq!(session.text(), "a");

        let mut family = EditorSession::new("a👨‍👩‍👧b", Eol::Lf);
        family.set_selection(19..19);
        assert!(family.backspace());
        assert_eq!(family.text(), "ab");
        assert!(family.delete());
        assert_eq!(family.text(), "a");

        let mut start = EditorSession::new("abc", Eol::Lf);
        start.set_selection(0..0);
        assert!(!start.backspace());
        start.set_selection(3..3);
        assert!(!start.delete());
    }

    #[test]
    fn backspace_deletes_a_selection_whole() {
        let mut session = EditorSession::new("abcdef", Eol::Lf);
        session.set_selection(1..4);
        assert!(session.backspace());
        assert_eq!(session.text(), "aef");
    }

    #[test]
    fn replace_range_normalizes_inserted_crlf_and_ignores_a_bom() {
        let mut session = EditorSession::new("ab", Eol::Lf);
        assert!(session.replace_range(1..1, "x\r\ny\u{feff}"));
        assert_eq!(session.text(), "ax\ny\u{feff}b");
        assert!(!session.replace_range(0..0, ""));
        assert!(!session.replace_range(0..1, "a"));
    }

    #[test]
    fn replace_all_is_one_undo_step_and_keeps_eol() {
        let mut session = EditorSession::new("a\nb", Eol::CrLf);
        assert!(session.replace_all("x\ny\n"));
        assert_eq!(session.text(), "x\ny\n");
        assert_eq!(session.save_text(), "x\r\ny\r\n");
        assert!(session.undo());
        assert_eq!(session.text(), "a\nb");
        assert!(!session.replace_all("a\nb"));
    }

    #[test]
    fn snapshot_restores_text_selection_find_and_history() {
        let mut session = EditorSession::new("one two", Eol::Lf);
        assert!(session.insert("!"));
        session.find.query = "two".into();
        assert_eq!(session.refresh_find(), 1);
        session.set_selection(0..3);
        let snapshot = session.snapshot();

        assert!(session.insert("?"));
        session.set_selection(0..1);
        session.find.query = "one".into();
        session.restore(snapshot);

        assert_eq!(session.text(), "one two!");
        assert_eq!(session.selection(), 0..3);
        assert_eq!(session.find.query, "two");
        assert_eq!(session.find.matches.len(), 1);
        assert!(session.undo());
        assert_eq!(session.text(), "one two");
    }

    #[test]
    fn draft_round_trips_the_buffer_and_base() {
        let mut session = EditorSession::new("text\n", Eol::Lf);
        session.insert("!");
        let draft = session.draft("text\n", 42);
        assert_eq!(draft.text, "text\n!");
        assert_eq!(draft.base, "text\n");
        assert_eq!(draft.at, 42);
        assert!(!draft.matches_disk("text\n"));
    }

    #[test]
    fn find_state_records_block_and_line() {
        let mut session = EditorSession::new("one\n\none two\n", Eol::Lf);
        session.find.query = "one".into();
        assert_eq!(session.refresh_find(), 2);
        assert_eq!(session.find.matches[0].block, 0);
        assert_eq!(session.find.matches[0].line, 0);
        assert_eq!(session.find.matches[1].block, 1);
        assert_eq!(session.find.matches[1].line, 2);

        session.set_selection(0..0);
        assert!(session.find_next());
        assert_eq!(session.selection(), 0..3);
        assert_eq!(session.find.current, 0);
        assert!(session.find_next());
        assert_eq!(session.selection(), 5..8);
        assert_eq!(session.find.current, 1);
        assert!(session.find_next());
        assert_eq!(session.selection(), 0..3, "find wraps");
        assert!(session.find_prev());
        assert_eq!(session.selection(), 5..8);
    }

    #[test]
    fn find_options_are_respected() {
        let mut session = EditorSession::new("Cat cat CAT", Eol::Lf);
        session.find.query = "cat".into();
        assert_eq!(session.refresh_find(), 3);
        session.find.case_sensitive = true;
        assert_eq!(session.refresh_find(), 1);
        session.find.case_sensitive = false;
        session.find.whole_word = true;
        assert_eq!(session.refresh_find(), 3);
        session.find.query = "(".into();
        session.find.regex = true;
        assert!(session.find_error().is_some());
        assert_eq!(session.refresh_find(), 0);
    }

    #[test]
    fn replace_current_replaces_and_advances_one_step() {
        let mut session = EditorSession::new("a a", Eol::Lf);
        session.find.query = "a".into();
        session.find.replace = "bb".into();
        session.set_selection(0..0);
        assert!(session.replace_current());
        assert_eq!(session.text(), "bb a");
        assert!(session.undo());
        assert_eq!(session.text(), "a a");

        session.set_selection(0..0);
        assert!(session.replace_current());
        assert!(session.replace_current());
        assert_eq!(session.text(), "bb bb");
    }

    #[test]
    fn replace_all_matches_is_one_undo_step_and_maps_the_selection() {
        let mut session = EditorSession::new("a a a", Eol::Lf);
        session.find.query = "a".into();
        session.find.replace = "bb".into();
        session.set_selection(0..5);
        assert_eq!(session.replace_all_matches(), 3);
        assert_eq!(session.text(), "bb bb bb");
        assert_eq!(session.selection(), 2..8);
        assert!(session.undo());
        assert_eq!(session.text(), "a a a");

        session.find.query = "a".into();
        session.find.replace = "a".into();
        assert_eq!(session.replace_all_matches(), 3);
        assert_eq!(session.text(), "a a a");
    }

    #[test]
    fn regex_replace_expands_captures() {
        let mut session = EditorSession::new("one two", Eol::Lf);
        session.find.query = r"(\w+) (\w+)".into();
        session.find.regex = true;
        session.find.replace = "$2 $1".into();
        assert_eq!(session.replace_all_matches(), 1);
        assert_eq!(session.text(), "two one");
    }

    #[test]
    fn block_mode_thresholds_and_round_trip() {
        let small = "a\n".repeat(100);
        let mut session = EditorSession::new(&small, Eol::Lf);
        assert!(session.block_mode().is_none());
        assert!(!session.block_mode_active());
        assert!(session.enable_block_mode().is_some());
        assert!(session.block_mode_active());
        assert_eq!(session.text(), small);
        assert!(session.disable_block_mode());
        assert_eq!(session.text(), small);

        let mut big = String::new();
        big.push_str(&"x".repeat(64 * 1024));
        big.push('\n');
        let mut large = EditorSession::new(&big, Eol::Lf);
        assert!(large.block_mode().is_some());

        let many_lines = "x\n".repeat(1_501);
        let mut lines = EditorSession::new(&many_lines, Eol::Lf);
        assert!(lines.block_mode().is_some());
    }

    #[test]
    fn block_mode_undo_walks_across_blocks() {
        let mut session = EditorSession::new("one\n\ntwo\n\nthree", Eol::Lf);
        assert!(session.enable_block_mode().is_some());
        session.set_selection(3..3);
        assert!(session.insert("!"));
        session.set_selection(16..16);
        assert!(session.insert("?"));
        assert_eq!(session.text(), "one!\n\ntwo\n\nthree?");
        assert!(session.undo());
        assert_eq!(session.text(), "one!\n\ntwo\n\nthree");
        assert!(session.undo());
        assert_eq!(session.text(), "one\n\ntwo\n\nthree");
        assert!(session.redo());
        assert_eq!(session.text(), "one!\n\ntwo\n\nthree");
    }

    #[test]
    fn block_mode_replaces_across_blocks_and_keeps_find_working() {
        let mut session = EditorSession::new("one\n\ntwo\n", Eol::Lf);
        assert!(session.enable_block_mode().is_some());
        session.find.query = "two".into();
        assert_eq!(session.refresh_find(), 1);
        session.find.replace = "TWO".into();
        assert_eq!(session.replace_all_matches(), 1);
        assert_eq!(session.text(), "one\n\nTWO\n");
        assert_eq!(session.refresh_find(), 1);
        assert!(session.undo());
        assert_eq!(session.text(), "one\n\ntwo\n");
    }

    #[test]
    fn snapshot_carries_the_block_journal() {
        let mut session = EditorSession::new("a\n\nb\n", Eol::Lf);
        assert!(session.enable_block_mode().is_some());
        session.set_selection(1..1);
        assert!(session.insert("!"));
        let snapshot = session.snapshot();

        session.set_selection(5..5);
        assert!(session.insert("?"));
        assert_eq!(session.text(), "a!\n\nb?\n");
        session.restore(snapshot);

        assert_eq!(session.text(), "a!\n\nb\n");
        assert!(session.undo());
        assert_eq!(session.text(), "a\n\nb\n");
    }

    #[test]
    fn session_commit_block_as_coalesces_typing() {
        let mut session = EditorSession::new("a\n\nb\n", Eol::Lf);
        assert!(session.enable_block_mode().is_some());
        if let Some(block) = session.block_editor_mut()
            && let Some(edit) = block.block_mut(0)
        {
            edit.set_caret(1);
            edit.insert("x");
        }
        assert!(session.commit_block_as(0, BlockEditKind::Typing));
        if let Some(block) = session.block_editor_mut()
            && let Some(edit) = block.block_mut(0)
        {
            edit.insert("y");
        }
        assert!(session.commit_block_as(0, BlockEditKind::Typing));
        assert_eq!(session.text(), "axy\n\nb\n");
        assert!(session.undo());
        assert_eq!(session.text(), "a\n\nb\n");
    }

    #[test]
    fn block_mode_find_next_moves_the_active_block() {
        let mut session = EditorSession::new("one\n\ntwo\n", Eol::Lf);
        assert!(session.enable_block_mode().is_some());
        session.find.query = "two".into();
        session.set_selection(0..0);
        assert!(session.find_next());
        assert_eq!(session.selection(), 5..8);
        assert_eq!(session.active_block(), Some(1));
    }

    #[test]
    fn disabling_block_mode_keeps_pending_direct_edits() {
        let mut session = EditorSession::new("a\n\nb\n", Eol::Lf);
        assert!(session.enable_block_mode().is_some());
        if let Some(block) = session.block_editor_mut()
            && let Some(edit) = block.block_mut(0)
        {
            edit.set_caret(1);
            edit.insert("!");
        }
        assert!(session.disable_block_mode());
        assert_eq!(session.text(), "a!\n\nb\n");
    }

    #[test]
    fn set_text_does_not_mark_a_user_edit() {
        let mut session = EditorSession::new("a", Eol::Lf);
        session.note_edit();
        assert!(session.last_edit_at.is_some());
        session.set_text("b");
        assert!(session.last_edit_at.is_none());
        assert_eq!(session.text(), "b");
    }

    #[test]
    fn set_text_clears_block_mode_and_find_hits() {
        let mut session = EditorSession::new("a\n", Eol::Lf);
        assert!(session.enable_block_mode().is_some());
        session.find.query = "a".into();
        session.refresh_find();
        session.set_text("\u{feff}b\r\n");
        assert_eq!(session.text(), "b\n");
        assert!(!session.block_mode_active());
        assert!(session.find.matches.is_empty());
        assert_eq!(session.find.query, "a");
    }

    #[test]
    fn syntax_spans_are_cached_per_revision() {
        let mut session = EditorSession::new("# Title\n\n`code`\n", Eol::Lf);
        let theme = SyntaxTheme::neutral(true);
        let first = session.syntax_spans(theme, None);
        assert!(!first.is_empty());
        let second = session.syntax_spans(theme, None);
        assert_eq!(first, second);
        session.note_edit();
        let third = session.syntax_spans(theme, None);
        assert_eq!(first, third);
    }

    #[test]
    fn line_count_matches_the_source_buffer() {
        assert_eq!(EditorSession::new("", Eol::Lf).line_count(), 1);
        assert_eq!(EditorSession::new("a", Eol::Lf).line_count(), 1);
        assert_eq!(EditorSession::new("a\n", Eol::Lf).line_count(), 2);
        assert_eq!(EditorSession::new("a\nb\n", Eol::Lf).line_count(), 3);
    }

    #[test]
    fn a_boundary_commit_splits_and_undo_restores_the_caret() {
        let mut session = EditorSession::new("a\n\n", Eol::Lf);
        assert!(session.enable_block_mode().is_some());
        session.set_active_block(Some(0));
        if let Some(field) = session.block_editor_mut().and_then(|blocks| blocks.block_mut(0)) {
            field.set_caret(3);
            field.insert("x");
        }
        assert!(session.commit_block_as(0, BlockEditKind::Typing));
        assert_eq!(session.text(), "a\n\nx");
        assert_eq!(session.block_editor().map(|blocks| blocks.blocks().len()), Some(2));
        assert_eq!(session.active_block(), Some(1), "the active field followed the caret");
        assert_eq!(session.selection(), 4..4);

        assert!(session.undo());
        assert_eq!(session.text(), "a\n\n");
        assert_eq!(session.block_editor().map(|blocks| blocks.blocks().len()), Some(1));
        assert_eq!(session.active_block(), Some(0));
        assert_eq!(session.selection(), 3..3, "undo returns to the pre-edit caret");
        assert!(session.redo());
        assert_eq!(session.block_editor().map(|blocks| blocks.blocks().len()), Some(2));
        assert_eq!(session.active_block(), Some(1));
    }

    #[test]
    fn structural_undo_restores_the_caret_and_active_block() {
        let mut session = EditorSession::new("abcd\n\nef\n", Eol::Lf);
        assert!(session.enable_block_mode().is_some());
        assert!(session.split_block(0, 2));
        assert_eq!(session.active_block(), Some(1));
        assert_eq!(session.selection(), 2..2);
        assert!(session.undo());
        assert_eq!(session.active_block(), Some(0));
        assert_eq!(session.selection(), 2..2, "the merged field keeps the split offset");
        assert!(session.redo());
        assert_eq!(session.active_block(), Some(1));
        assert_eq!(session.selection(), 2..2);

        assert!(session.merge_blocks(0));
        assert_eq!(session.active_block(), Some(0));
        assert_eq!(session.selection(), 2..2);
        assert!(session.undo());
        assert_eq!(session.active_block(), Some(1));
        assert_eq!(session.selection(), 2..2);
        assert!(session.redo());
        assert_eq!(session.active_block(), Some(0));
        assert_eq!(session.selection(), 2..2);
    }
}
