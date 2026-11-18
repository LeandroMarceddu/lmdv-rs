//! The large-document block editor and its global undo journal (ADR-016).
//!
//! Above [`BLOCK_MODE_MAX_BYTES`] or [`BLOCK_MODE_MAX_LINES`] the app renders
//! one [`TextEdit`] per blank-line block instead of one field for the whole
//! document. Blocks cover the source exactly and are contiguous: each block is
//! a maximal run of non-blank lines plus the blank lines that follow it, so an
//! edit never falls between two ranges. Leading blank lines join the first
//! block.
//!
//! Each block keeps its own Rune history, but undo is owned by the journal
//! here: each journal operation records the block index and the [`EditDelta`]
//! of its text change, so `Ctrl+Z` can walk back across blocks in commit order.
//! Every
//! structural change (split, merge, whole-source replacement) is journaled
//! too, which keeps the stored block indices valid for a last-in-first-out
//! undo. Truncation drops the oldest entries and keeps the latest step, so a
//! single oversized replace-all still undoes once.

use std::ops::Range;

use lmdv_core::{EditDelta, normalize_lf};
use rune::TextEdit;

/// Text at or below this size stays in one field while the line count is also
/// below [`BLOCK_MODE_MAX_LINES`].
pub const BLOCK_MODE_MAX_BYTES: usize = 64 * 1024;

/// Line count at or below this stays in one field while the text is also at or
/// below [`BLOCK_MODE_MAX_BYTES`].
pub const BLOCK_MODE_MAX_LINES: usize = 1_500;

/// Undo entries kept by the block journal.
pub const JOURNAL_MAX_ENTRIES: usize = 128;

/// Approximate bytes of before/after block text kept by the block journal.
pub const JOURNAL_MAX_BYTES: usize = 1024 * 1024;

/// One blank-line-separated block: its range in the LF source and the field
/// the app binds.
#[derive(Clone, Debug)]
pub struct EditBlock {
    /// Byte range in the whole source; always on character boundaries.
    pub range: Range<usize>,
    /// The block's text and selection. Its `text` mirrors `source[range]` once
    /// the edit is committed.
    pub edit: TextEdit,
}

/// How an edit joins undo history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockEditKind {
    /// A typed character: coalesces with the previous typing run.
    Typing,
    /// Backspace or Delete: coalesces with another deletion; typed text after
    /// a deletion joins it. A deletion after typing starts a new step, as
    /// Rune's editor history does.
    Deleting,
    /// A command, paste or IME commit: one undo step of its own.
    Discrete,
}

impl BlockEditKind {
    /// Whether this kind may join a previous entry instead of starting one.
    pub fn joins_with(self, previous: BlockEditKind) -> bool {
        matches!(
            (previous, self),
            (BlockEditKind::Typing, BlockEditKind::Typing)
                | (BlockEditKind::Deleting, BlockEditKind::Typing)
                | (BlockEditKind::Deleting, BlockEditKind::Deleting)
        )
    }

    fn is_typing(self) -> bool {
        matches!(self, BlockEditKind::Typing | BlockEditKind::Deleting)
    }
}

#[derive(Clone, Debug)]
enum JournalOp {
    Text {
        block: usize,
        delta: EditDelta,
        before: String,
        after: String,
        before_sel: (usize, usize),
        after_sel: (usize, usize),
        kind: BlockEditKind,
    },
    /// A text edit that also broke or created blank-line boundaries, so the
    /// affected blocks were re-partitioned. `before_ends`/`after_ends` are the
    /// block ends relative to the affected span, which is what lets undo and
    /// redo restore the field layout along with the text (OI-089).
    Relayout {
        block: usize,
        delta: EditDelta,
        before: String,
        after: String,
        before_ends: Vec<usize>,
        after_ends: Vec<usize>,
        before_sel: (usize, usize),
        after_sel: (usize, usize),
        /// The block that owned the caret before and after the edit; a boundary
        /// offset can belong to either field, so the entry stores the choice.
        before_block: usize,
        after_block: usize,
    },
    Split {
        block: usize,
        at: usize,
    },
    Merge {
        block: usize,
        at: usize,
    },
    /// A change spanning blocks, or a replace-all: the whole source is reset.
    Whole {
        before: String,
        after: String,
    },
}

#[derive(Clone, Debug)]
struct JournalEntry {
    op: JournalOp,
    bytes: usize,
}

impl JournalEntry {
    fn new(op: JournalOp) -> Self {
        let bytes = match &op {
            JournalOp::Text { before, after, .. }
            | JournalOp::Relayout { before, after, .. }
            | JournalOp::Whole { before, after } => before.len() + after.len(),
            JournalOp::Split { .. } | JournalOp::Merge { .. } => 16,
        };
        Self { op, bytes }
    }
}

/// Whether `text` needs block mode.
pub fn should_use_block_mode(text: &str) -> bool {
    text.len() > BLOCK_MODE_MAX_BYTES || line_count(text) > BLOCK_MODE_MAX_LINES
}

/// Splits `text` into contiguous blank-line-separated blocks.
///
/// A block is a maximal run of non-blank lines plus the blank lines that
/// follow it; leading blank lines join the first block. Blank lines inside a
/// fenced code block do not split, so the fence keeps its language. The ranges
/// cover the whole text with no gaps or overlap, and an empty text yields one
/// empty block so the app always has a field to bind.
pub fn block_ranges(text: &str) -> Vec<Range<usize>> {
    if text.is_empty() {
        return std::iter::once(0..0).collect();
    }

    let mut out = Vec::new();
    let mut offset = 0;
    let mut block_start = 0;
    let mut seen_content = false;
    let mut in_blank_run = false;
    let mut fence: Option<(char, usize)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let mut content = false;
        if let Some((marker, run)) = fence {
            content = true;
            if is_fence_close(trimmed, marker, run) {
                fence = None;
            }
        } else if let Some((marker, run, _)) = fence_open(trimmed) {
            fence = Some((marker, run));
            content = true;
        }
        if !content {
            content = !line.trim().is_empty();
        }
        if content {
            if seen_content && in_blank_run {
                out.push(block_start..offset);
                block_start = offset;
            }
            seen_content = true;
            in_blank_run = false;
        } else if seen_content {
            in_blank_run = true;
        }
        offset += line.len();
    }
    out.push(block_start..offset);
    out
}

/// The blank-line block editor: the source, one field per block and the global
/// undo journal.
#[derive(Clone, Debug)]
pub struct BlockEditor {
    source: String,
    blocks: Vec<EditBlock>,
    journal: Vec<JournalEntry>,
    redo: Vec<JournalEntry>,
    journal_bytes: usize,
    typing_open: bool,
    /// The block a structural operation last moved the caret into, for the app
    /// to focus the right field after a split, merge, undo or redo (OI-090).
    caret_block: Option<usize>,
}

impl BlockEditor {
    /// Splits `text` into blocks. CRLF is normalized to LF.
    pub fn new(text: &str) -> Self {
        let source = normalize_lf(text).into_owned();
        let blocks = build_blocks(&source);
        Self {
            source,
            blocks,
            journal: Vec::new(),
            redo: Vec::new(),
            journal_bytes: 0,
            typing_open: false,
            caret_block: None,
        }
    }

    /// The block that the last structural or relayout operation placed the
    /// caret in. The app focuses this field after a split, merge, undo or redo.
    pub fn caret_block(&self) -> Option<usize> {
        self.caret_block
    }

    /// The committed LF source. Edits made through [`Self::block_mut`] appear
    /// here after [`Self::commit_block`].
    pub fn text(&self) -> &str {
        &self.source
    }

    /// The blocks in source order.
    pub fn blocks(&self) -> &[EditBlock] {
        &self.blocks
    }

    /// One block.
    pub fn block(&self, index: usize) -> Option<&EditBlock> {
        self.blocks.get(index)
    }

    /// One block's field, for a binding or a direct edit followed by
    /// [`Self::commit_block`].
    pub fn block_mut(&mut self, index: usize) -> Option<&mut TextEdit> {
        self.blocks.get_mut(index).map(|block| &mut block.edit)
    }

    /// The block containing `byte`; the last block for the end of the text.
    pub fn block_index_at(&self, byte: usize) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        let byte = byte.min(self.source.len());
        let index = self.blocks.partition_point(|block| block.range.start <= byte).saturating_sub(1);
        Some(index.min(self.blocks.len() - 1))
    }

    /// Applies `f` to one block's field. Typing kinds coalesce with the open
    /// journal entry; [`BlockEditKind::Discrete`] runs inside one Rune undo
    /// group so the command is one step even if it edits several times.
    pub fn apply<R>(&mut self, index: usize, kind: BlockEditKind, f: impl FnOnce(&mut TextEdit) -> R) -> Option<R> {
        self.commit_pending_at(index);
        self.caret_block = None;
        let (before, before_sel, after, after_sel, result) = {
            let block = self.blocks.get_mut(index)?;
            let before = block.edit.text.clone();
            let before_sel = (block.edit.anchor, block.edit.focus);
            let result = if kind == BlockEditKind::Discrete { block.edit.undo_group(f) } else { f(&mut block.edit) };
            let after = block.edit.text.clone();
            let after_sel = (block.edit.anchor, block.edit.focus);
            (before, before_sel, after, after_sel, result)
        };
        if before != after {
            self.record_text(index, before, after, before_sel, after_sel, kind);
        }
        Some(result)
    }

    /// Copies a directly edited field back into the source as a discrete edit.
    pub fn commit_block(&mut self, index: usize) -> bool {
        self.commit_block_as(index, BlockEditKind::Discrete)
    }

    /// Copies a directly edited field back into the source with an explicit
    /// coalescing kind. CRLF pasted into the field is normalized first.
    pub fn commit_block_as(&mut self, index: usize, kind: BlockEditKind) -> bool {
        self.caret_block = None;
        let Some(block) = self.blocks.get(index) else {
            return false;
        };
        let Some(before) = self.source.get(block.range.clone()).map(str::to_owned) else {
            return false;
        };
        let after = normalize_lf(&block.edit.text).into_owned();
        if before == after {
            return false;
        }
        if after != block.edit.text {
            let (from, to) = block.edit.range();
            if let Some(block) = self.blocks.get_mut(index) {
                block.edit.replace_text(after.clone());
                block.edit.set_selection(from, to);
            }
        }
        let selection = self.blocks.get(index).map(|block| (block.edit.anchor, block.edit.focus)).unwrap_or((0, 0));
        let before_sel = selection_before(&before, &after, selection);
        self.record_text(index, before, after, before_sel, selection, kind);
        true
    }

    /// Replaces a range of the whole source. A range inside one block edits
    /// that block; a range spanning blocks resets the layout as one undo step.
    pub fn replace_source(&mut self, range: Range<usize>, text: &str) -> bool {
        self.commit_pending();
        let text = normalize_lf(text);
        let (range, unchanged) = {
            let source = &self.source;
            let range = clamp_range(source, range);
            (range.clone(), &source[range] == text.as_ref())
        };
        if unchanged {
            return false;
        }
        let bounds = self
            .block_index_at(range.start)
            .and_then(|index| self.blocks.get(index).map(|block| (index, block.range.start, block.range.end)));
        if let Some((index, block_start, block_end)) = bounds
            && range.start >= block_start
            && range.end <= block_end
        {
            let start = range.start - block_start;
            let end = range.end - block_start;
            return self
                .apply(index, BlockEditKind::Discrete, |edit| {
                    edit.set_selection(start, end);
                    edit.insert(text.as_ref());
                })
                .is_some();
        }

        let before = self.source.clone();
        let mut after = before.clone();
        after.replace_range(range, text.as_ref());
        self.record_whole(before, after.clone());
        self.reset_source(&after);
        true
    }

    /// Replaces the whole source, keeping the layout reset in one undo step.
    pub fn replace_all_source(&mut self, text: &str) -> bool {
        self.commit_pending();
        let text = normalize_lf(text);
        if text.as_ref() == self.source {
            return false;
        }
        let before = std::mem::replace(&mut self.source, text.into_owned());
        self.blocks = build_blocks(&self.source);
        let after = self.source.clone();
        self.record_whole(before, after);
        true
    }

    /// Commits direct field edits in every block, so the source is current.
    /// Returns whether any block changed.
    pub fn commit_pending_edits(&mut self) -> bool {
        self.commit_pending()
    }

    /// Splits block `index` at local byte `at` (a line or blank-line boundary
    /// the app chose). Undoable with [`Self::undo`]; the caret lands at the
    /// start of the new right block.
    pub fn split_block(&mut self, index: usize, at: usize) -> bool {
        self.commit_pending_at(index);
        self.caret_block = None;
        if !self.split_structural(index, at) {
            return false;
        }
        self.push_structural(JournalOp::Split { block: index, at });
        self.caret_block = Some(index + 1);
        true
    }

    /// Merges block `index` and the next one. Undoable with [`Self::undo`]; the
    /// caret lands at the former boundary.
    pub fn merge_blocks(&mut self, index: usize) -> bool {
        self.commit_pending_at(index);
        self.commit_pending_at(index + 1);
        self.caret_block = None;
        let Some(at) = self.blocks.get(index).map(|block| block.edit.text.len()) else {
            return false;
        };
        if !self.merge_structural(index) {
            return false;
        }
        self.push_structural(JournalOp::Merge { block: index, at });
        self.caret_block = Some(index);
        true
    }

    /// Closes the open typing run so the next edit starts a new undo step.
    pub fn break_typing_group(&mut self) {
        self.typing_open = false;
    }

    /// How many global undo steps are available.
    pub fn undo_depth(&self) -> usize {
        self.journal.len()
    }

    /// How many redo steps are available.
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// The delta of the most recent text edit, for the app's line index and
    /// preview invalidation.
    pub fn last_delta(&self) -> Option<EditDelta> {
        match self.journal.last().map(|entry| &entry.op) {
            Some(JournalOp::Text { delta, .. } | JournalOp::Relayout { delta, .. }) => Some(*delta),
            _ => None,
        }
    }

    /// Undoes the most recent journal entry, across blocks. A structural entry
    /// restores the caret where the edit left it (OI-090); see
    /// [`Self::caret_block`].
    pub fn undo(&mut self) -> bool {
        self.commit_pending();
        let Some(entry) = self.journal.pop() else {
            return false;
        };
        self.journal_bytes = self.journal_bytes.saturating_sub(entry.bytes);
        self.typing_open = false;
        self.caret_block = None;
        match &entry.op {
            JournalOp::Text { block, before, before_sel, .. } => {
                if self.set_block_text(*block, before) {
                    let selection = *before_sel;
                    if let Some(block) = self.blocks.get_mut(*block) {
                        set_local_selection(block, selection);
                    }
                    self.caret_block = Some(*block);
                }
            }
            JournalOp::Relayout { block, before, before_ends, after_ends, before_sel, before_block, .. } => {
                self.restore_layout(*block, after_ends, before, before_ends, *before_sel, *before_block);
            }
            JournalOp::Split { block, at } => {
                if self.merge_structural(*block) {
                    if let Some(block) = self.blocks.get_mut(*block) {
                        set_local_selection(block, (*at, *at));
                    }
                    self.caret_block = Some(*block);
                }
            }
            JournalOp::Merge { block, at } => {
                if self.split_structural(*block, *at) {
                    self.caret_block = Some(*block + 1);
                }
            }
            JournalOp::Whole { before, .. } => {
                self.reset_source(before);
            }
        }
        self.redo.push(entry);
        true
    }

    /// Redoes the most recently undone entry, across blocks, with the same
    /// caret preference as [`Self::undo`].
    pub fn redo(&mut self) -> bool {
        self.commit_pending();
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        self.typing_open = false;
        self.caret_block = None;
        match &entry.op {
            JournalOp::Text { block, after, after_sel, .. } => {
                if self.set_block_text(*block, after) {
                    let selection = *after_sel;
                    if let Some(block) = self.blocks.get_mut(*block) {
                        set_local_selection(block, selection);
                    }
                    self.caret_block = Some(*block);
                }
            }
            JournalOp::Relayout { block, after, before_ends, after_ends, after_sel, after_block, .. } => {
                self.restore_layout(*block, before_ends, after, after_ends, *after_sel, *after_block);
            }
            JournalOp::Split { block, at } => {
                if self.split_structural(*block, *at) {
                    self.caret_block = Some(*block + 1);
                }
            }
            JournalOp::Merge { block, at } => {
                if self.merge_structural(*block) {
                    if let Some(block) = self.blocks.get_mut(*block) {
                        set_local_selection(block, (*at, *at));
                    }
                    self.caret_block = Some(*block);
                }
            }
            JournalOp::Whole { after, .. } => {
                self.reset_source(after);
            }
        }
        self.journal_bytes = self.journal_bytes.saturating_add(entry.bytes);
        self.journal.push(entry);
        true
    }

    fn record_text(
        &mut self,
        index: usize,
        before: String,
        after: String,
        before_sel: (usize, usize),
        after_sel: (usize, usize),
        kind: BlockEditKind,
    ) {
        let Some(block) = self.blocks.get(index) else {
            return;
        };
        let start = block.range.start;
        let old_len = before.len();

        let joins = self.typing_open
            && kind.is_typing()
            && self.journal.last().is_some_and(|entry| match &entry.op {
                JournalOp::Text { block: previous, kind: previous_kind, .. } => {
                    *previous == index && kind.joins_with(*previous_kind)
                }
                _ => false,
            });

        let delta = if joins {
            match self.journal.last().map(|entry| &entry.op) {
                Some(JournalOp::Text { before: first, .. }) => compute_delta(&self.source, start, first, &after),
                _ => compute_delta(&self.source, start, &before, &after),
            }
        } else {
            compute_delta(&self.source, start, &before, &after)
        };

        self.source.replace_range(start..start + old_len, &after);
        self.blocks[index].range.end = start + after.len();
        shift_later_ranges(&mut self.blocks, index, after.len() as isize - old_len as isize);

        // OI-089: a commit that creates or removes a blank-line boundary must
        // re-partition the nearby fields in memory instead of waiting for a
        // reload. The text change and the layout change stay one journal entry.
        if let Some(layout) = self.layout_change(index, &before, before_sel, after_sel) {
            self.apply_layout(&layout);
            let after_block = self.caret_block.unwrap_or(layout.first);
            let entry = JournalEntry::new(JournalOp::Relayout {
                block: layout.first,
                delta,
                before: layout.before,
                after: layout.after,
                before_ends: layout.before_ends,
                after_ends: layout.after_ends,
                before_sel: layout.before_sel,
                after_sel: layout.after_sel,
                before_block: index,
                after_block,
            });
            self.journal_bytes = self.journal_bytes.saturating_add(entry.bytes);
            self.journal.push(entry);
            self.typing_open = false;
            self.redo.clear();
            self.trim_journal();
            return;
        }

        self.caret_block = Some(index);
        if joins {
            if let Some(entry) = self.journal.last_mut() {
                let previous_bytes = entry.bytes;
                let (first_before, first_selection) = match &entry.op {
                    JournalOp::Text { before, before_sel, .. } => (before.clone(), *before_sel),
                    _ => (before.clone(), before_sel),
                };
                entry.op = JournalOp::Text {
                    block: index,
                    delta,
                    before: first_before,
                    after: after.clone(),
                    before_sel: first_selection,
                    after_sel,
                    kind,
                };
                entry.bytes = match &entry.op {
                    JournalOp::Text { before, after, .. } => before.len() + after.len(),
                    _ => previous_bytes,
                };
                self.journal_bytes = self.journal_bytes.saturating_sub(previous_bytes) + entry.bytes;
            }
        } else {
            let entry =
                JournalEntry::new(JournalOp::Text { block: index, delta, before, after, before_sel, after_sel, kind });
            self.journal_bytes = self.journal_bytes.saturating_add(entry.bytes);
            self.journal.push(entry);
        }
        self.typing_open = kind.is_typing();
        self.redo.clear();
        self.trim_journal();
    }

    /// The layout re-partition a committed edit needs, when the nearby fields
    /// no longer follow the blank-line rule. The affected span is the edited
    /// block plus one neighbour on each side, so a split inside a fence and a
    /// merge across a removed separator are both covered without scanning a
    /// large document per keystroke.
    fn layout_change(
        &self,
        index: usize,
        before_block: &str,
        before_sel: (usize, usize),
        after_sel: (usize, usize),
    ) -> Option<LayoutChange> {
        let first = index.saturating_sub(1);
        let last = (index + 1).min(self.blocks.len().checked_sub(1)?);
        if first > last {
            return None;
        }
        let region_start = self.blocks[first].range.start;
        let region_end = self.blocks[last].range.end;
        let after = self.source.get(region_start..region_end)?.to_owned();
        let after_ends: Vec<usize> = block_ranges(&after).iter().map(|range| range.end).collect();
        let old_ends: Vec<usize> =
            self.blocks[first..=last].iter().map(|block| block.range.end - region_start).collect();
        if old_ends == after_ends {
            return None;
        }

        // Rebuild the pre-edit text of the same span with the edited field's
        // before text; the neighbours' fields were not touched.
        let edit_offset = self.blocks[index].range.start - region_start;
        let mut before = String::with_capacity(after.len().max(before_block.len()));
        let mut before_ends = Vec::with_capacity(old_ends.len());
        for block in first..=last {
            if block == index {
                before.push_str(before_block);
            } else {
                before.push_str(self.source.get(self.blocks[block].range.clone())?);
            }
            before_ends.push(before.len());
        }
        let before_sel = (before.len().min(edit_offset + before_sel.0), before.len().min(edit_offset + before_sel.1));
        let after_sel = (after.len().min(edit_offset + after_sel.0), after.len().min(edit_offset + after_sel.1));
        Some(LayoutChange { first, before, before_ends, after, after_ends, before_sel, after_sel })
    }

    /// Replaces the affected fields with the canonical blank-line partition.
    fn apply_layout(&mut self, change: &LayoutChange) {
        let first = change.first;
        let old_count = change.before_ends.len();
        let new_count = change.after_ends.len();
        if new_count == 0 || first + old_count > self.blocks.len() {
            return;
        }
        let region_start = self.blocks[first].range.start;
        let mut new_blocks = Vec::with_capacity(new_count);
        let mut previous = 0;
        for end in &change.after_ends {
            let range = region_start + previous..region_start + end;
            new_blocks.push(EditBlock { range: range.clone(), edit: TextEdit::new(self.source[range].to_owned()) });
            previous = *end;
        }
        self.blocks.splice(first..first + old_count, new_blocks);
        self.place_caret(first, new_count, region_start, change.after_sel, None);
    }

    /// Restores one affected span's text and field partition, then puts the
    /// caret at the saved offset inside the restored layout.
    fn restore_layout(
        &mut self,
        first: usize,
        current_ends: &[usize],
        text: &str,
        target_ends: &[usize],
        selection: (usize, usize),
        caret_block: usize,
    ) {
        let last = first + current_ends.len();
        if target_ends.is_empty() || last > self.blocks.len() {
            return;
        }
        let Some(start) = self.blocks.get(first).map(|block| block.range.start) else { return };
        let end = self.blocks[last - 1].range.end;
        let old_len = end - start;
        self.source.replace_range(start..end, text);
        let mut new_blocks = Vec::with_capacity(target_ends.len());
        let mut previous = 0;
        for target in target_ends {
            let range = start + previous..start + target;
            new_blocks.push(EditBlock { range: range.clone(), edit: TextEdit::new(self.source[range].to_owned()) });
            previous = *target;
        }
        self.blocks.splice(first..last, new_blocks);
        let shift = text.len() as isize - old_len as isize;
        shift_later_ranges(&mut self.blocks, first + target_ends.len() - 1, shift);
        self.place_caret(first, target_ends.len(), start, selection, Some(caret_block));
    }

    /// Puts the caret at `region_start + selection.0` inside the new fields.
    /// `preferred` names the field that owned the caret when the offset sits
    /// exactly on a block boundary.
    fn place_caret(
        &mut self,
        first: usize,
        count: usize,
        region_start: usize,
        selection: (usize, usize),
        preferred: Option<usize>,
    ) {
        if count == 0 || first >= self.blocks.len() {
            return;
        }
        let caret = region_start + selection.0;
        let index = preferred
            .unwrap_or_else(|| self.block_index_at(caret).unwrap_or(first))
            .clamp(first, (first + count - 1).min(self.blocks.len() - 1));
        let start = self.blocks[index].range.start;
        let from = caret.saturating_sub(start);
        let to = (region_start + selection.1).saturating_sub(start);
        set_local_selection(&mut self.blocks[index], (from, to));
        self.caret_block = Some(index);
    }

    fn record_whole(&mut self, before: String, after: String) {
        self.caret_block = None;
        let entry = JournalEntry::new(JournalOp::Whole { before, after });
        self.journal_bytes = self.journal_bytes.saturating_add(entry.bytes);
        self.journal.push(entry);
        self.typing_open = false;
        self.redo.clear();
        self.trim_journal();
    }

    fn push_structural(&mut self, op: JournalOp) {
        let entry = JournalEntry::new(op);
        self.journal_bytes = self.journal_bytes.saturating_add(entry.bytes);
        self.journal.push(entry);
        self.typing_open = false;
        self.redo.clear();
        self.trim_journal();
    }

    fn trim_journal(&mut self) {
        // Keep at least the latest entry even when it alone exceeds the byte
        // cap: replace-all on a large document must still undo once.
        while self.journal.len() > 1
            && (self.journal.len() > JOURNAL_MAX_ENTRIES || self.journal_bytes > JOURNAL_MAX_BYTES)
        {
            let entry = self.journal.remove(0);
            self.journal_bytes = self.journal_bytes.saturating_sub(entry.bytes);
        }
    }

    fn commit_pending_at(&mut self, index: usize) -> bool {
        let pending = match self.blocks.get(index) {
            Some(block) => self.source.get(block.range.clone()).is_some_and(|text| text != block.edit.text),
            None => false,
        };
        pending && self.commit_block(index)
    }

    fn commit_pending(&mut self) -> bool {
        let mut changed = false;
        for index in 0..self.blocks.len() {
            changed |= self.commit_pending_at(index);
        }
        changed
    }

    fn split_structural(&mut self, index: usize, at: usize) -> bool {
        let Some(block) = self.blocks.get(index) else {
            return false;
        };
        let text = block.edit.text.clone();
        if at == 0 || at >= text.len() || !text.is_char_boundary(at) {
            return false;
        }
        let start = block.range.start;
        let end = block.range.end;
        let left = text[..at].to_owned();
        let right = text[at..].to_owned();
        self.blocks[index].edit = TextEdit::new(left);
        self.blocks[index].range.end = start + at;
        let mut field = TextEdit::new(right);
        field.set_selection(0, 0);
        self.blocks.insert(index + 1, EditBlock { range: start + at..end, edit: field });
        true
    }

    fn merge_structural(&mut self, index: usize) -> bool {
        if index + 1 >= self.blocks.len() {
            return false;
        }
        let mut text = self.blocks[index].edit.text.clone();
        let boundary = text.len();
        let right = self.blocks[index + 1].edit.text.clone();
        text.push_str(&right);
        let range = self.blocks[index].range.start..self.blocks[index + 1].range.end;
        self.blocks[index].edit = TextEdit::new(text);
        self.blocks[index].edit.set_selection(boundary, boundary);
        self.blocks[index].range = range;
        self.blocks.remove(index + 1);
        true
    }

    fn set_block_text(&mut self, index: usize, text: &str) -> bool {
        let Some(block) = self.blocks.get(index) else {
            return false;
        };
        let start = block.range.start;
        let old_len = block.range.end - start;
        if start + old_len > self.source.len() || !self.source.is_char_boundary(start) {
            return false;
        }
        self.source.replace_range(start..start + old_len, text);
        self.blocks[index].range.end = start + text.len();
        self.blocks[index].edit.replace_text(text);
        shift_later_ranges(&mut self.blocks, index, text.len() as isize - old_len as isize);
        true
    }

    fn reset_source(&mut self, text: &str) {
        self.source = normalize_lf(text).into_owned();
        self.blocks = build_blocks(&self.source);
    }
}

/// The marker and run length of a fenced code block opener, with its info
/// string; shared by block splitting and the comment command.
pub(crate) fn fence_open(trimmed: &str) -> Option<(char, usize, &str)> {
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|&ch| ch == marker).count();
    if run < 3 {
        return None;
    }
    Some((marker, run, &trimmed[run..]))
}

/// Whether `trimmed` closes a fence opened with `marker` repeated `run` times.
pub(crate) fn is_fence_close(trimmed: &str, marker: char, run: usize) -> bool {
    let closing = trimmed.chars().take_while(|&ch| ch == marker).count();
    closing >= run && trimmed[closing..].trim().is_empty()
}

/// One affected span's pre- and post-edit partition, for [`BlockEditor::apply_layout`].
struct LayoutChange {
    first: usize,
    before: String,
    before_ends: Vec<usize>,
    after: String,
    after_ends: Vec<usize>,
    before_sel: (usize, usize),
    after_sel: (usize, usize),
}

/// Maps a post-edit selection back through the changed span to the pre-edit
/// text, so undo returns to where the edit began.
fn selection_before(before: &str, after: &str, selection: (usize, usize)) -> (usize, usize) {
    let mut prefix = 0;
    for (a, b) in before.chars().zip(after.chars()) {
        if a != b {
            break;
        }
        prefix += a.len_utf8();
    }
    let max_suffix = (before.len() - prefix).min(after.len() - prefix);
    let before_bytes = before.as_bytes();
    let after_bytes = after.as_bytes();
    let mut suffix = 0;
    while suffix < max_suffix && before_bytes[before.len() - 1 - suffix] == after_bytes[after.len() - 1 - suffix] {
        suffix += 1;
    }
    while suffix > 0
        && !(before.is_char_boundary(before.len() - suffix) && after.is_char_boundary(after.len() - suffix))
    {
        suffix -= 1;
    }
    let removed = before.len() - prefix - suffix;
    let inserted = after.len() - prefix - suffix;
    let map = |offset: usize| {
        if offset <= prefix {
            offset
        } else if offset >= prefix + inserted {
            offset - inserted + removed
        } else {
            prefix
        }
    };
    (map(selection.0).min(before.len()), map(selection.1).min(before.len()))
}

fn build_blocks(source: &str) -> Vec<EditBlock> {
    block_ranges(source)
        .into_iter()
        .map(|range| {
            let edit = TextEdit::new(source[range.clone()].to_owned());
            EditBlock { range, edit }
        })
        .collect()
}

fn line_count(text: &str) -> usize {
    1 + text.bytes().filter(|&byte| byte == b'\n').count()
}

fn shift_later_ranges(blocks: &mut [EditBlock], index: usize, delta: isize) {
    if delta == 0 {
        return;
    }
    for block in blocks.iter_mut().skip(index + 1) {
        block.range.start = block.range.start.checked_add_signed(delta).unwrap_or(0);
        block.range.end = block.range.end.checked_add_signed(delta).unwrap_or(0);
    }
}

fn set_local_selection(block: &mut EditBlock, selection: (usize, usize)) {
    let len = block.edit.text.len();
    block.edit.set_selection(selection.0.min(len), selection.1.min(len));
}

fn clamp_range(text: &str, range: Range<usize>) -> Range<usize> {
    let mut start = range.start.min(text.len());
    let mut end = range.end.min(text.len());
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    start = floor_boundary(text, start);
    end = floor_boundary(text, end);
    start..end
}

fn floor_boundary(text: &str, byte: usize) -> usize {
    text.floor_char_boundary(byte)
}

/// Common character prefix and suffix of two strings. Both ends land on
/// character boundaries.
fn compute_delta(source: &str, block_start: usize, before: &str, after: &str) -> EditDelta {
    let mut prefix = 0;
    for (a, b) in before.chars().zip(after.chars()) {
        if a != b {
            break;
        }
        prefix += a.len_utf8();
    }

    let max_suffix = (before.len() - prefix).min(after.len() - prefix);
    let before_bytes = before.as_bytes();
    let after_bytes = after.as_bytes();
    let mut suffix = 0;
    while suffix < max_suffix && before_bytes[before.len() - 1 - suffix] == after_bytes[after.len() - 1 - suffix] {
        suffix += 1;
    }
    while suffix > 0
        && !(before.is_char_boundary(before.len() - suffix) && after.is_char_boundary(after.len() - suffix))
    {
        suffix -= 1;
    }

    let start = block_start + prefix;
    let removed = before.len() - prefix - suffix;
    let inserted = after.len() - prefix - suffix;
    let first_line = source[..start.min(source.len())].bytes().filter(|&byte| byte == b'\n').count();
    let removed_slice = &before[prefix..before.len() - suffix];
    let inserted_slice = &after[prefix..after.len() - suffix];
    let line_delta = inserted_slice.bytes().filter(|&byte| byte == b'\n').count() as isize
        - removed_slice.bytes().filter(|&byte| byte == b'\n').count() as isize;

    EditDelta { start, removed, inserted, first_line, line_delta }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(editor: &BlockEditor) -> Vec<String> {
        editor.blocks().iter().map(|block| block.edit.text.clone()).collect()
    }

    #[test]
    fn ranges_cover_the_source_and_split_at_blank_runs() {
        assert_eq!(block_ranges(""), vec![0..0]);
        assert_eq!(block_ranges("a"), vec![0..1]);
        assert_eq!(block_ranges("a\n\nb\n"), vec![0..3, 3..5]);
        assert_eq!(block_ranges("\n\na\n"), vec![0..4]);
        assert_eq!(block_ranges("a\n  \nb"), vec![0..5, 5..6]);
        assert_eq!(block_ranges("a\nb\n\nc"), vec![0..5, 5..6]);
        assert_eq!(
            block_ranges("a\n\n```\n\ncode\n```\nb\n"),
            vec![0..3, 3..19],
            "blank lines inside a fence do not split"
        );
    }

    #[test]
    fn ranges_are_contiguous_for_random_text() {
        let pieces = ["a", "b", " ", "\n", "\n\n", "  \n", ""];
        let mut text = String::new();
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..200 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            text.push_str(pieces[(state as usize) % pieces.len()]);
        }
        let ranges = block_ranges(&text);
        assert_eq!(ranges.first().map(|range| range.start), Some(0));
        assert_eq!(ranges.last().map(|range| range.end), Some(text.len()));
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        let joined: String = ranges.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn new_builds_one_field_per_block() {
        let editor = BlockEditor::new("one\n\ntwo\n");
        assert_eq!(editor.blocks().len(), 2);
        assert_eq!(texts(&editor), ["one\n\n", "two\n"]);
        assert_eq!(editor.text(), "one\n\ntwo\n");
        assert_eq!(editor.block_index_at(0), Some(0));
        assert_eq!(editor.block_index_at(4), Some(0));
        assert_eq!(editor.block_index_at(5), Some(1));
        assert_eq!(editor.block_index_at(9), Some(1));

        let empty = BlockEditor::new("");
        assert_eq!(empty.blocks().len(), 1);
        assert_eq!(empty.text(), "");
    }

    #[test]
    fn typing_coalesces_into_one_undo_step() {
        let mut editor = BlockEditor::new("x\n");
        editor.apply(0, BlockEditKind::Typing, |edit| {
            edit.set_caret(1);
            edit.insert("a");
        });
        editor.apply(0, BlockEditKind::Typing, |edit| edit.insert("b"));
        assert_eq!(editor.text(), "xab\n");
        assert_eq!(editor.last_delta().map(|delta| delta.inserted), Some(2));

        assert!(editor.undo());
        assert_eq!(editor.text(), "x\n");
        assert!(!editor.undo());
        assert!(editor.redo());
        assert_eq!(editor.text(), "xab\n");
    }

    #[test]
    fn deletions_join_typing_and_break_starts_a_new_step() {
        let mut editor = BlockEditor::new("");
        editor.apply(0, BlockEditKind::Typing, |edit| edit.insert("ab"));
        editor.apply(0, BlockEditKind::Deleting, |edit| {
            edit.set_selection(1, 2);
            edit.insert("");
            edit.set_selection(0, 1);
            edit.insert("");
        });
        assert_eq!(editor.text(), "");
        assert_eq!(editor.journal.len(), 2);
        assert!(editor.undo());
        assert_eq!(editor.text(), "ab");
        assert!(editor.undo());
        assert_eq!(editor.text(), "");
        assert_eq!(editor.journal.len(), 0);

        editor.apply(0, BlockEditKind::Typing, |edit| edit.insert("c"));
        editor.break_typing_group();
        editor.apply(0, BlockEditKind::Typing, |edit| edit.insert("d"));
        assert_eq!(editor.journal.len(), 2);
        assert!(editor.undo());
        assert_eq!(editor.text(), "c");
        assert!(editor.undo());
        assert_eq!(editor.text(), "");
    }

    #[test]
    fn discrete_edits_are_separate_steps() {
        let mut editor = BlockEditor::new("");
        editor.apply(0, BlockEditKind::Discrete, |edit| edit.insert("a"));
        editor.apply(0, BlockEditKind::Discrete, |edit| edit.insert("b"));
        assert_eq!(editor.journal.len(), 2);
        assert!(editor.undo());
        assert_eq!(editor.text(), "a");
        assert!(editor.undo());
        assert_eq!(editor.text(), "");
    }

    #[test]
    fn undo_crosses_blocks_in_commit_order() {
        let mut editor = BlockEditor::new("one\n\ntwo\n\nthree\n");
        assert_eq!(editor.blocks().len(), 3);
        editor.apply(0, BlockEditKind::Discrete, |edit| {
            edit.set_caret(3);
            edit.insert("!");
        });
        editor.apply(2, BlockEditKind::Discrete, |edit| {
            edit.set_caret(5);
            edit.insert("?");
        });
        assert_eq!(editor.text(), "one!\n\ntwo\n\nthree?\n");

        assert!(editor.undo());
        assert_eq!(editor.text(), "one!\n\ntwo\n\nthree\n");
        assert!(editor.undo());
        assert_eq!(editor.text(), "one\n\ntwo\n\nthree\n");
        assert!(!editor.undo());

        assert!(editor.redo());
        assert_eq!(editor.text(), "one!\n\ntwo\n\nthree\n");
        assert!(editor.redo());
        assert_eq!(editor.text(), "one!\n\ntwo\n\nthree?\n");
    }

    #[test]
    fn direct_edits_commit_and_shift_later_ranges() {
        let mut editor = BlockEditor::new("a\n\nb\n");
        assert_eq!(editor.blocks()[1].range, 3..5);
        if let Some(edit) = editor.block_mut(0) {
            edit.set_caret(1);
            edit.insert("123");
        }
        assert!(editor.commit_block(0));
        assert_eq!(editor.text(), "a123\n\nb\n");
        assert_eq!(editor.blocks()[0].range, 0..6);
        assert_eq!(editor.blocks()[1].range, 6..8);
        assert!(editor.undo());
        assert_eq!(editor.text(), "a\n\nb\n");
        assert_eq!(editor.blocks()[1].range, 3..5);
    }

    #[test]
    fn split_and_merge_are_undoable_and_keep_ranges() {
        let mut editor = BlockEditor::new("abcd\n\nef\n");
        assert!(editor.split_block(0, 2));
        assert_eq!(texts(&editor), ["ab", "cd\n\n", "ef\n"]);
        assert_eq!(editor.blocks()[1].range, 2..6);
        assert_eq!(editor.blocks()[2].range, 6..9);
        assert_eq!(editor.text(), "abcd\n\nef\n");

        assert!(editor.undo());
        assert_eq!(texts(&editor), ["abcd\n\n", "ef\n"]);
        assert!(editor.redo());
        assert_eq!(texts(&editor), ["ab", "cd\n\n", "ef\n"]);

        assert!(editor.merge_blocks(0));
        assert_eq!(texts(&editor), ["abcd\n\n", "ef\n"]);
        assert_eq!(editor.text(), "abcd\n\nef\n");
        assert!(editor.undo());
        assert_eq!(texts(&editor), ["ab", "cd\n\n", "ef\n"]);
        assert!(editor.redo());
        assert_eq!(texts(&editor), ["abcd\n\n", "ef\n"]);

        assert!(!editor.split_block(0, 0));
        assert!(!editor.split_block(0, "abcd\n\n".len()));
        assert!(!editor.merge_blocks(editor.blocks().len() - 1));
    }

    #[test]
    fn replace_inside_one_block_stays_one_entry() {
        let mut editor = BlockEditor::new("one\n\ntwo\n");
        assert!(editor.replace_source(5..8, "TWO"));
        assert_eq!(editor.text(), "one\n\nTWO\n");
        assert_eq!(editor.journal.len(), 1);
        assert!(editor.undo());
        assert_eq!(editor.text(), "one\n\ntwo\n");
        assert!(editor.redo());
        assert_eq!(editor.text(), "one\n\nTWO\n");
        assert!(!editor.replace_source(5..8, "TWO"));
    }

    #[test]
    fn replace_across_blocks_resets_the_layout_one_step() {
        let mut editor = BlockEditor::new("one\n\ntwo\n");
        assert!(editor.replace_source(4..8, "x"));
        assert_eq!(editor.text(), "one\nx\n");
        assert!(editor.undo());
        assert_eq!(editor.text(), "one\n\ntwo\n");
        assert!(editor.redo());
        assert_eq!(editor.text(), "one\nx\n");

        assert!(editor.replace_all_source("a\n\nb\n"));
        assert_eq!(texts(&editor), ["a\n\n", "b\n"]);
        assert!(editor.undo());
        assert_eq!(editor.text(), "one\nx\n");
    }

    #[test]
    fn crlf_is_normalized_and_empty_documents_keep_a_field() {
        let editor = BlockEditor::new("a\r\n\r\nb\r\n");
        assert_eq!(editor.text(), "a\n\nb\n");
        assert_eq!(editor.blocks().len(), 2);
        let empty = BlockEditor::new("");
        assert_eq!(empty.blocks().len(), 1);
        assert_eq!(empty.blocks()[0].range, 0..0);
    }

    #[test]
    fn journal_is_bounded_and_keeps_the_newest_entries() {
        let mut editor = BlockEditor::new("");
        for i in 0..(JOURNAL_MAX_ENTRIES + 20) {
            let text = char::from(b'a' + (i % 26) as u8).to_string();
            editor.apply(0, BlockEditKind::Discrete, |edit| edit.insert(&text));
        }
        assert!(editor.journal.len() <= JOURNAL_MAX_ENTRIES);
        assert!(editor.journal_bytes <= JOURNAL_MAX_BYTES);
        assert_eq!(editor.text().chars().count(), JOURNAL_MAX_ENTRIES + 20);
        for _ in 0..JOURNAL_MAX_ENTRIES {
            assert!(editor.undo());
        }
        assert!(!editor.undo());
    }

    #[test]
    fn replace_source_commits_pending_edits_first() {
        let mut editor = BlockEditor::new("a\n\nb\n");
        if let Some(edit) = editor.block_mut(0) {
            edit.set_caret(1);
            edit.insert("!");
        }
        assert!(editor.replace_source(4..5, "B"));
        assert_eq!(editor.text(), "a!\n\nB\n");
        assert!(editor.undo());
        assert_eq!(editor.text(), "a!\n\nb\n");
        assert!(editor.undo());
        assert_eq!(editor.text(), "a\n\nb\n");
    }

    #[test]
    fn a_single_oversized_whole_edit_stays_undoable() {
        let text = "a".repeat(600 * 1024);
        let replacement = "b".repeat(600 * 1024);
        let mut editor = BlockEditor::new(&text);
        assert!(editor.replace_all_source(&replacement));
        assert!(editor.undo());
        assert_eq!(editor.text(), text);
        assert!(editor.redo());
        assert_eq!(editor.text(), replacement);
    }

    #[test]
    fn undo_commits_pending_direct_edits_first() {
        let mut editor = BlockEditor::new("a\n");
        if let Some(edit) = editor.block_mut(0) {
            edit.set_caret(1);
            edit.insert("b");
        }
        assert!(editor.undo());
        assert_eq!(editor.text(), "a\n");

        let mut mixed = BlockEditor::new("a\n");
        if let Some(edit) = mixed.block_mut(0) {
            edit.set_caret(1);
            edit.insert("b");
        }
        mixed.apply(0, BlockEditKind::Discrete, |edit| {
            edit.set_caret(2);
            edit.insert("c");
        });
        assert_eq!(mixed.text(), "abc\n");
        assert!(mixed.undo());
        assert_eq!(mixed.text(), "ab\n");
        assert!(mixed.undo());
        assert_eq!(mixed.text(), "a\n");
    }

    #[test]
    fn delta_reports_the_splice_in_source_coordinates() {
        let mut editor = BlockEditor::new("one\n\ntwo\n");
        editor.apply(1, BlockEditKind::Discrete, |edit| {
            edit.set_caret(3);
            edit.insert("!");
        });
        let delta = editor.last_delta().expect("text edit recorded");
        assert_eq!(delta.start, 8);
        assert_eq!(delta.removed, 0);
        assert_eq!(delta.inserted, 1);
        assert_eq!(delta.first_line, 2);
        assert_eq!(delta.line_delta, 0);
    }

    fn caret(editor: &BlockEditor) -> (usize, usize) {
        let index = editor.caret_block().expect("a caret block");
        let block = editor.block(index).expect("the caret's block");
        (index, block.range.start + block.edit.anchor)
    }

    #[test]
    fn typing_into_a_separator_splits_and_undoes_in_one_step() {
        let mut editor = BlockEditor::new("a\n\nb\n");
        editor.block_mut(0).expect("block").set_caret(3);
        editor.apply(0, BlockEditKind::Typing, |edit| {
            edit.insert("x");
        });
        assert_eq!(editor.text(), "a\n\nxb\n");
        assert_eq!(texts(&editor), ["a\n\n", "xb\n"], "the layout followed the text");
        assert_eq!(editor.journal.len(), 1, "text and split are one undo step");
        assert_eq!(caret(&editor), (1, 4), "the caret followed the typed text");

        assert!(editor.undo());
        assert_eq!(texts(&editor), ["a\n\n", "b\n"], "undo restores the pre-edit field layout");
        assert_eq!(caret(&editor), (0, 3), "and the caret before the edit");
        assert!(editor.redo());
        assert_eq!(texts(&editor), ["a\n\n", "xb\n"]);
        assert_eq!(caret(&editor), (1, 4));
    }

    #[test]
    fn pasting_paragraphs_splits_at_every_boundary_in_one_step() {
        let mut editor = BlockEditor::new("head\n\ntail\n");
        editor.block_mut(0).expect("block").set_caret(4);
        editor.apply(0, BlockEditKind::Discrete, |edit| {
            edit.insert("P1\n\nP2\n\nP3");
        });
        assert_eq!(editor.text(), "headP1\n\nP2\n\nP3\n\ntail\n");
        assert_eq!(texts(&editor), ["headP1\n\n", "P2\n\n", "P3\n\n", "tail\n"]);
        assert_eq!(editor.journal.len(), 1, "one paste, one undo step");
        assert_eq!(caret(&editor), (2, 14));

        assert!(editor.undo());
        assert_eq!(editor.text(), "head\n\ntail\n");
        assert_eq!(texts(&editor), ["head\n\n", "tail\n"]);
        assert_eq!(caret(&editor), (0, 4));
        assert!(editor.redo());
        assert_eq!(texts(&editor), ["headP1\n\n", "P2\n\n", "P3\n\n", "tail\n"]);
    }

    #[test]
    fn deleting_the_separator_merges_the_next_block_in_one_step() {
        let mut editor = BlockEditor::new("a\n\nb\n");
        editor.block_mut(0).expect("block").set_caret(3);
        editor.apply(0, BlockEditKind::Deleting, |edit| {
            edit.set_selection(2, 3);
            edit.insert("");
        });
        assert_eq!(editor.text(), "a\nb\n");
        assert_eq!(texts(&editor), ["a\nb\n"], "one field after the boundary went");
        assert_eq!(editor.journal.len(), 1);
        assert_eq!(caret(&editor), (0, 2));

        assert!(editor.undo());
        assert_eq!(texts(&editor), ["a\n\n", "b\n"]);
        assert_eq!(caret(&editor), (0, 3));
        assert!(editor.redo());
        assert_eq!(texts(&editor), ["a\nb\n"]);
    }

    #[test]
    fn a_fenced_block_never_splits_on_its_blank_lines() {
        let mut editor = BlockEditor::new("```\na\n\nb\n```\n\nafter\n");
        assert_eq!(editor.blocks().len(), 2);
        editor.apply(0, BlockEditKind::Typing, |edit| {
            edit.set_caret(8);
            edit.insert("\n\n");
        });
        assert_eq!(editor.blocks().len(), 2, "blank lines inside the fence stay in one field");

        let mut pasted = BlockEditor::new("intro\n\ntail\n");
        pasted.apply(0, BlockEditKind::Discrete, |edit| {
            edit.set_caret(7);
            edit.insert("```\n\ncode\n\n```\n\nmore\n\n");
        });
        assert_eq!(pasted.blocks().len(), 4, "only the boundaries outside the fence split");
        assert_eq!(texts(&pasted), ["intro\n\n", "```\n\ncode\n\n```\n\n", "more\n\n", "tail\n"]);
        assert!(pasted.undo());
        assert_eq!(texts(&pasted), ["intro\n\n", "tail\n"]);
    }

    #[test]
    fn deleting_a_whole_block_removes_its_empty_field() {
        let mut editor = BlockEditor::new("a\n\nb\n\nc\n");
        editor.apply(1, BlockEditKind::Discrete, |edit| {
            let len = edit.text.len();
            edit.set_selection(0, len);
            edit.insert("");
        });
        assert_eq!(editor.text(), "a\n\nc\n");
        assert_eq!(texts(&editor), ["a\n\n", "c\n"]);
        assert!(editor.undo());
        assert_eq!(texts(&editor), ["a\n\n", "b\n\n", "c\n"]);
        assert!(editor.redo());
        assert_eq!(texts(&editor), ["a\n\n", "c\n"]);
    }

    #[test]
    fn splitting_at_eof_keeps_trailing_blanks_with_the_last_field() {
        let mut editor = BlockEditor::new("a\n");
        editor.block_mut(0).expect("block").set_caret(1);
        editor.apply(0, BlockEditKind::Typing, |edit| {
            edit.insert("\n\nmore");
        });
        assert_eq!(editor.text(), "a\n\nmore\n");
        assert_eq!(texts(&editor), ["a\n\n", "more\n"]);

        editor.apply(1, BlockEditKind::Discrete, |edit| {
            let len = edit.text.len();
            edit.set_selection(0, len);
            edit.insert("");
        });
        assert_eq!(editor.text(), "a\n\n");
        assert_eq!(texts(&editor), ["a\n\n"], "the empty last field merged back");
        assert!(editor.undo());
        assert_eq!(texts(&editor), ["a\n\n", "more\n"]);
    }

    #[test]
    fn structural_undo_and_redo_restore_the_structural_caret() {
        let mut editor = BlockEditor::new("abcd\n\nef\n");
        assert!(editor.split_block(0, 2));
        assert_eq!(caret(&editor), (1, 2), "split leaves the caret at the new block's start");
        assert!(editor.undo());
        assert_eq!(caret(&editor), (0, 2), "undo returns to the merged boundary");
        assert!(editor.redo());
        assert_eq!(caret(&editor), (1, 2));

        assert!(editor.merge_blocks(0));
        assert_eq!(caret(&editor), (0, 2), "merge keeps the boundary");
        assert!(editor.undo());
        assert_eq!(caret(&editor), (1, 2), "undo of a merge restores the right field");
        assert!(editor.redo());
        assert_eq!(caret(&editor), (0, 2));
    }
}
