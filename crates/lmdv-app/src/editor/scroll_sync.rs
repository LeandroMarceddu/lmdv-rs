//! Editor→preview scroll sync (E-07, OI-075).
//!
//! One-way: the source's scroll position drives the preview, never the other
//! way. The source viewport reports its offset into [`EditorState`]'s `scroll`
//! cell; [`WindowState::poll_scroll_sync`] turns the latest offset into a
//! 0-based fractional source line and places the preview block that holds it,
//! interpolating between that block's top and the next anchor (the original's
//! `yInSpan`). After a caret move or a preview parse, the caret's top-level
//! block is revealed when it fits, else its heading, else its estimated line
//! (the original's caret block in `syncPreviewToEditor`).
//!
//! Both panes are measured once per frame, never while scrolling:
//!
//! * The source view publishes [`EditorTop`]: the content y of its first line
//!   and, in block mode, the visible row's measured top and height.
//!   `Cx::layout` reads layout as if every scroll viewport were at its top, so
//!   scrolling does not re-run the view.
//! * The preview view publishes [`PreviewGeometry`]: the content y and height
//!   of the blocks around the editor's top and caret, their heading, and a
//!   table's sticky-header cover.
//!
//! Approximations (OI-075):
//!
//! * The single source field maps its scroll offset through the field's live
//!   line height (E-37: 1.6 × the editor font), so a wrapped line counts as two
//!   visual lines and a heavily wrapped document maps a little ahead of its
//!   source lines. The original reads CodeMirror's exact `topVisibleLine()`.
//! * In block mode the visible row's measured height is scaled onto the
//!   block's source-line span, likewise assuming a uniform line height.
//! * Only the tables measured around the editor's top and caret contribute a
//!   sticky-header cover; overlapping tables are approximated by their
//!   maximum. lmdv has no section-line overlay (the outline replaces it), so
//!   the original's `sectionLine.reserve` is 0.

use std::ops::Range;
use std::time::{Duration, Instant};

use lmdv_md::{Block as MdBlock, MdDoc};
use lmdv_render::{DOC_KEY, FRONT_MATTER_KEY, heading_key};
use rune::{Cx, Tx};

use crate::edit_ui::{SOURCE_FIELD_KEY, SOURCE_SCROLL_KEY};
use crate::reader::ARTICLE_SCROLL_KEY;
use crate::window::{Mode, WindowState};

/// The gap the original keeps above the block at the preview's top
/// (`SYNC_MARGIN` in `main.ts`).
pub const SYNC_MARGIN: f32 = 16.0;
/// The source body's top padding, points (`editor-body`/`editor-blocks` `py`).
pub const EDITOR_PAD_TOP: f32 = 16.0;
/// A caret move keeps editor scrolls revealing for this long (the original's
/// `revealUntil`, 300 ms).
pub const REVEAL_WINDOW: f64 = 0.3;
/// Editor scroll reports this soon after entering edit mode settle the editor
/// rather than moving the preview (the original's `syncSuppressedUntil`).
pub const ENTRY_SUPPRESS: Duration = Duration::from_millis(200);
/// Below this many points two scroll offsets are the same line.
const OFFSET_EPSILON: f32 = 0.5;

/// What the source view measured about the editor's scrollable content.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorTop {
    /// The measurement came from real layout. `false` before the first frame
    /// and while the source pane is not on screen.
    pub measured: bool,
    /// The top of the first source line, in editor content points.
    pub start: f32,
    /// In block mode, the block whose row holds the viewport's top.
    pub block: Option<usize>,
    /// In block mode, that block row's top in editor content points.
    pub block_top: f32,
    /// In block mode, that block row's measured height, points.
    pub block_height: f32,
}

/// One part of a preview block that can reveal on its own (a heading).
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewPart {
    /// The element's key (`h{index}`).
    pub key: String,
    /// The part's top in article content points.
    pub top: f32,
    /// The part's height, points.
    pub height: f32,
}

/// A preview table's box and sticky header, in article content points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewTable {
    /// The table's own top.
    pub top: f32,
    /// The table's bottom (its rows included).
    pub bottom: f32,
    /// The height of the header row that sticks to the viewport's top.
    pub head: f32,
}

/// One preview block as the article laid it out.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewBlock {
    /// The block's index in `MdDoc::blocks`.
    pub index: usize,
    /// The block's stable key (`b{index}`).
    pub key: String,
    /// The block's top in article content points.
    pub top: f32,
    /// The block's height, points.
    pub height: f32,
    /// The next anchor's top in article content points: the following block,
    /// or the article's bottom for the last one.
    pub next_top: f32,
    /// The heading inside this block, when it has one.
    pub heading: Option<PreviewPart>,
    /// The table's box and sticky header, when the block is a table.
    pub table: Option<PreviewTable>,
}

impl PreviewBlock {
    /// The content points from `top` to the next anchor, for interpolation.
    pub fn span(&self) -> f32 {
        (self.next_top - self.top).max(1.0)
    }
}

/// What the preview view measured in its last frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewGeometry {
    /// The measured blocks, in document order.
    pub blocks: Vec<PreviewBlock>,
    /// The article's bottom in content points.
    pub content_bottom: f32,
    /// The article viewport's height, points.
    pub viewport: f32,
    /// The article viewport's maximum scroll offset from its last report.
    pub max: f32,
}

impl PreviewGeometry {
    /// The measured block at `index`.
    pub fn block(&self, index: usize) -> Option<&PreviewBlock> {
        self.blocks.iter().find(|block| block.index == index)
    }

    /// The part of the viewport's top a stuck table header covers over the
    /// content at `y` (the original's `headCover`).
    ///
    /// Over a table's rows the whole header is still at the top; across the
    /// header itself the cover grows from nothing to all of it; `after` fades
    /// it out over one header height past the table's bottom, as the editor's
    /// scroll sync does, so leaving a table does not jump.
    pub fn cover_at(&self, y: f32, after: bool) -> f32 {
        let mut cover = 0.0f32;
        for block in &self.blocks {
            let Some(table) = block.table else { continue };
            let covered = if y < table.bottom {
                (y - table.top).min(table.head)
            } else if after {
                table.head - (y - table.bottom)
            } else {
                0.0
            };
            cover = cover.max(covered.max(0.0));
        }
        cover
    }
}

/// The last editor→preview mapping, exposed for tests and headless checks.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewSync {
    /// The 0-based source line shown: the editor's top, or the caret on a reveal.
    pub line: f32,
    /// The preview element the scroll targets.
    pub key: String,
    /// The `at` handed to `Tx::scroll_key_to`, points below the viewport top.
    pub y: f32,
    /// The mapping came from a caret reveal, not only the editor's top.
    pub reveal: bool,
    /// The preview had not measured the anchor yet: the block's top was
    /// placed, and the next frame refines it.
    pub fallback: bool,
}

/// One computed preview placement.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    /// The element to place.
    pub key: String,
    /// Its top's distance below the preview viewport's top, points.
    pub y: f32,
    /// The content-space y the viewport's top should land on.
    pub target: f32,
    /// The anchored `MdDoc` block, when there is one.
    pub index: Option<usize>,
    /// The preview had not measured this block yet.
    pub fallback: bool,
}

/// The 0-based source line at the editor's top (E-07's first half).
///
/// In single-field mode the offset is divided by the field's live line height
/// (E-37's 1.6 × the editor font, 22.4 pt at the default 16 pt reading size)
/// from the field's first line. In block mode the visible row's measured top
/// and height place the offset inside that block, and the fraction is scaled
/// onto the block's source-line span. Both clamp to the document's lines.
pub fn top_line(top: &EditorTop, offset: f32, span: (usize, usize), lines: usize, line_height: f32) -> f32 {
    let last = lines.saturating_sub(1) as f32;
    if top.block.is_some() {
        let (start, end) = span;
        let end = end.max(start.saturating_add(1));
        if !top.measured || top.block_height <= 0.0 {
            return (start as f32).min(last);
        }
        let t = ((offset - top.block_top) / top.block_height).clamp(0.0, 1.0);
        (start as f32 + t * (end - start) as f32).clamp(0.0, last)
    } else {
        let start = if top.measured { top.start } else { EDITOR_PAD_TOP };
        ((offset - start) / line_height.max(1.0)).clamp(0.0, last)
    }
}

/// The preview placement for a fractional source line.
///
/// `at_top` reproduces the original's rule that an editor at its very top
/// shows the preview from its very top (above the reading column's padding and
/// any front matter).
pub fn preview_placement(doc: &MdDoc, line: f32, at_top: bool, geometry: &PreviewGeometry) -> Option<Placement> {
    let lines = doc.line_count().max(1);
    let line = line.clamp(0.0, lines.saturating_sub(1) as f32);
    let floor = (line.floor() as usize).min(lines.saturating_sub(1));
    let byte = *doc.line_starts.get(floor)? as usize;
    let (index, key) = match block_at_or_above(doc, floor) {
        Some(index) => (index, doc.block_key(index)?.to_owned()),
        None => {
            // A line in the front matter sits before the first block; the card
            // is its anchor. Without one the line has no preview element.
            let first = doc.block_spans.first().map_or(usize::MAX, |span| span.source.start);
            if doc.front_matter.is_some() && byte < first {
                // The card is not an `MdDoc` block; place its top at the
                // margin. Its own layout is not interpolated (OI-075).
                return Some(Placement {
                    key: FRONT_MATTER_KEY.to_owned(),
                    y: -SYNC_MARGIN,
                    target: 0.0,
                    index: None,
                    fallback: false,
                });
            }
            return None;
        }
    };
    let Some(block) = geometry.block(index) else {
        // The preview has not measured this block yet (a jump across blocks):
        // land its top at the margin and refine on the next measurement.
        return Some(Placement { key, y: -SYNC_MARGIN, target: 0.0, index: Some(index), fallback: true });
    };
    let t = anchor_span(doc, index).map_or(0.0, |(start, end)| span_fraction(line, start, end));
    let y_line = block.top + t * block.span();
    let cover = geometry.cover_at(y_line, true);
    let raw = y_line - SYNC_MARGIN - cover;
    let target = if at_top { 0.0 } else { clamp_target(raw, geometry) };
    // `scroll_key_to` takes the anchor's top *below* the viewport top; the
    // viewport's top sits at content `target`, so the anchor's top is
    // `block.top - target` below it (negative: scrolled into the block).
    Some(Placement { key, y: block.top - target, target, index: Some(index), fallback: false })
}

/// The preview placement that keeps the caret's block in view (E-07's second
/// half).
///
/// The whole top-level block is used when it fits in the viewport (a table, a
/// formula, a short list or paragraph); else its heading; else the caret's
/// estimated line inside the block. `base` is the top mapping's placement, so
/// a caret already visible does not move the preview.
pub fn reveal_placement(
    doc: &MdDoc,
    caret_line: f32,
    base: &Placement,
    geometry: &PreviewGeometry,
) -> Option<Placement> {
    let lines = doc.line_count().max(1);
    let line = caret_line.clamp(0.0, lines.saturating_sub(1) as f32);
    let floor = (line.floor() as usize).min(lines.saturating_sub(1));
    let index = block_at_or_above(doc, floor)?;
    let key = doc.block_key(index)?.to_owned();
    let Some(block) = geometry.block(index) else {
        return Some(Placement { key, y: SYNC_MARGIN, target: 0.0, index: Some(index), fallback: true });
    };
    let (t, next) = anchor_span(doc, index)
        .map_or((0.0, 0.0), |(start, end)| (span_fraction(line, start, end), span_fraction(line + 1.0, start, end)));
    let viewport = geometry.viewport;
    let pad = SYNC_MARGIN * 3.0;
    let room = viewport - pad - SYNC_MARGIN;
    let (mut caret_top, mut caret_bottom) = (block.top + t * block.span(), block.top + next * block.span());
    if room > 0.0 && block.height <= room {
        caret_top = block.top;
        caret_bottom = block.top + block.height;
    } else if room > 0.0
        && let Some(heading) = block.heading.as_ref()
        && heading.height <= room
    {
        caret_top = heading.top;
        caret_bottom = heading.top + heading.height;
    }
    let mut target = base.target;
    if viewport > 0.0 {
        if caret_bottom > target + viewport - pad {
            target = caret_bottom - viewport + pad;
        }
        let above = SYNC_MARGIN + geometry.cover_at(caret_top, false);
        if caret_top < target + above {
            target = caret_top - above;
        }
    }
    let target = clamp_target(target, geometry);
    Some(Placement { key, y: block.top - target, target, index: Some(index), fallback: false })
}

/// The line span of one preview anchor: its own first line to the next
/// block's first line (the original's `anchorSpan` `l1..l2`), which includes
/// the blank gap and the following block's top margin.
fn anchor_span(doc: &MdDoc, index: usize) -> Option<(usize, usize)> {
    let span = doc.block_spans.get(index)?;
    let end = doc.block_spans.get(index.saturating_add(1)).map_or(span.end_line, |next| next.line);
    Some((span.line, end.max(span.line.saturating_add(1))))
}

/// The fraction of a block's source-line span a line sits at, clamped.
fn span_fraction(line: f32, start: usize, end: usize) -> f32 {
    let width = end.saturating_sub(start).max(1) as f32;
    ((line - start as f32) / width).clamp(0.0, 1.0)
}

fn clamp_target(target: f32, geometry: &PreviewGeometry) -> f32 {
    if geometry.max > OFFSET_EPSILON && geometry.viewport > OFFSET_EPSILON {
        target.clamp(0.0, geometry.max)
    } else {
        target.max(0.0)
    }
}

/// The last preview block whose source line is at or above `line`.
///
/// The original's anchor list keeps every element with a source line, so a
/// blank line between blocks belongs to the block above it; `block_index_at`
/// alone answers `None` for the blank run between two blocks.
fn block_at_or_above(doc: &MdDoc, line: usize) -> Option<usize> {
    if doc.block_spans.is_empty() {
        return None;
    }
    let line = line.min(doc.line_count().saturating_sub(1));
    let after = doc.block_spans.partition_point(|span| span.line <= line);
    (after > 0).then(|| after - 1)
}

impl WindowState {
    /// The 0-based source line at the editor's top, fractional (E-07).
    pub(crate) fn editor_top_line(&self) -> Option<f32> {
        let (lines, span) = self.editor_span()?;
        let top = self.editor.editor_top.get();
        let offset = self.editor.scroll.get().offset;
        Some(top_line(&top, offset, span, lines, self.editor_metrics().line_height))
    }

    /// The 0-based line the editor's caret is on.
    pub(crate) fn editor_caret_line(&self) -> Option<usize> {
        let tab = self.active_tab()?;
        let editor = tab.view.editor.as_ref()?;
        let doc = tab.document()?;
        Some(doc.text().line_of(editor.selection().start))
    }

    /// The line count and the visible block's source-line span `start..end`.
    fn editor_span(&self) -> Option<(usize, (usize, usize))> {
        let tab = self.active_tab()?;
        let editor = tab.view.editor.as_ref()?;
        let doc = tab.document()?;
        let lines = doc.text().line_count().max(1);
        let top = self.editor.editor_top.get();
        let Some(index) = top.block else { return Some((lines, (0, lines))) };
        let Some(block) = editor.block_editor().and_then(|blocks| blocks.block(index)) else {
            // The block count changed under a stale measurement; skip this
            // frame instead of mapping through the wrong span.
            return None;
        };
        let start = doc.text().line_of(block.range.start);
        let end = doc.text().line_of(block.range.end).max(start.saturating_add(1));
        Some((lines, (start, end)))
    }

    /// Records where the source field's first line sits (called by the view).
    pub(crate) fn publish_editor_top(&self, cx: &Cx<WindowState>) {
        let Some(tab) = self.active_tab() else { return };
        let Some(editor) = tab.view.editor.as_ref() else { return };
        let viewport = cx.layout(SOURCE_SCROLL_KEY);
        let top = if editor.block_mode_active() {
            let index = self
                .editor
                .visible_blocks
                .as_ref()
                .map(|range| range.start)
                .or_else(|| editor.active_block())
                .unwrap_or(0);
            match (viewport, cx.layout(&crate::block_ui::block_field_key(index))) {
                (Some(view), Some(field)) => EditorTop {
                    measured: true,
                    start: field[1] - view[1],
                    block: Some(index),
                    block_top: field[1] - view[1],
                    block_height: field[3].max(1.0),
                },
                _ => EditorTop { block: Some(index), ..EditorTop::default() },
            }
        } else {
            match (viewport, cx.layout(SOURCE_FIELD_KEY)) {
                (Some(view), Some(field)) => {
                    EditorTop { measured: true, start: field[1] - view[1], ..EditorTop::default() }
                }
                _ => EditorTop { start: EDITOR_PAD_TOP, ..EditorTop::default() },
            }
        };
        self.editor.editor_top.set(top);
    }

    /// Records the preview blocks around the editor's top and caret (called by
    /// the reader view while editing).
    pub(crate) fn publish_preview_geometry(&self, doc: &MdDoc, cx: &Cx<WindowState>) {
        if self.active_tab().is_none_or(|tab| tab.view.mode != Mode::Editor) {
            return;
        }
        let Some(top_line) = self.editor_top_line() else { return };
        let Some(caret_line) = self.editor_caret_line() else { return };
        let last = doc.blocks.len().saturating_sub(1);
        let mut wanted: Vec<usize> = Vec::new();
        if let Some(top) = block_at_or_above(doc, top_line.floor() as usize) {
            wanted.extend(top.saturating_sub(1)..=top.saturating_add(4).min(last));
        }
        if let Some(caret) = block_at_or_above(doc, caret_line) {
            wanted.extend(caret.saturating_sub(1)..=caret.saturating_add(1).min(last));
        }
        wanted.sort_unstable();
        wanted.dedup();
        let content_bottom = cx.layout(DOC_KEY).map_or(0.0, |rect| rect[1] + rect[3]);
        let mut blocks = Vec::with_capacity(wanted.len());
        for index in wanted {
            let Some(key) = doc.block_key(index).map(str::to_owned) else { continue };
            let Some(rect) = cx.layout(&key) else { continue };
            let next_top = doc
                .block_key(index.saturating_add(1))
                .and_then(|next| cx.layout(next))
                .map_or_else(|| next_top_of_last(index, doc, rect, content_bottom), |next| next[1]);
            let heading = doc.headings.iter().position(|heading| heading.block == index).and_then(|heading| {
                let key = heading_key(heading);
                cx.layout(&key).map(|rect| PreviewPart { key, top: rect[1], height: rect[3] })
            });
            blocks.push(PreviewBlock {
                index,
                key,
                top: rect[1],
                height: rect[3],
                next_top,
                heading,
                table: table_geometry(doc, cx, index),
            });
        }
        *self.editor.preview.borrow_mut() = PreviewGeometry {
            blocks,
            content_bottom,
            viewport: cx.layout_size(ARTICLE_SCROLL_KEY).map_or(0.0, |size| size[1]),
            max: self.scroll_snapshot().max,
        };
    }

    /// Drops the measured preview geometry (leaving edit mode, or a source-only
    /// frame that does not draw the preview).
    pub(crate) fn clear_preview_geometry(&self) {
        *self.editor.preview.borrow_mut() = PreviewGeometry::default();
    }

    /// Remembers the virtual rows the source is showing (block mode).
    pub(crate) fn note_visible_blocks(&mut self, range: Range<usize>) -> bool {
        if self.editor.visible_blocks.as_ref() == Some(&range) {
            return false;
        }
        self.editor.visible_blocks = Some(range);
        true
    }

    /// Watches the source viewport and the caret for changes to sync (E-07).
    ///
    /// Manual preview scrolling never reaches this: only the source's reported
    /// offset, a caret move or a parse completion asks for a preview scroll.
    pub(crate) fn poll_scroll_sync(&mut self, now: f64, tx: &mut Tx) -> bool {
        let Some(tab) = self.active_tab() else { return false };
        if tab.view.mode != Mode::Editor
            || tab.view.reader.is_none()
            || tab.view.editor.is_none()
            || self.editor.preview_narrow
            || !self.editor.preview_visible
        {
            return false;
        }
        let caret = self.editor_caret_line();
        if self.editor.sync_suppressed_until.is_some_and(|until| Instant::now() < until) {
            // Edit entry: the field's first scroll settling must not move the
            // preview off the reading position (the original's 200 ms window).
            self.editor.last_offset = Some(self.editor.scroll.get().offset);
            self.editor.last_caret = caret;
            return false;
        }
        self.editor.sync_suppressed_until = None;
        let offset = self.editor.scroll.get().offset;
        let offset_changed = self.editor.last_offset.is_none_or(|last| (last - offset).abs() > OFFSET_EPSILON);
        let caret_changed = caret.is_some() && self.editor.last_caret != caret;
        self.editor.last_offset = Some(offset);
        self.editor.last_caret = caret;
        if caret_changed {
            self.editor.reveal_until = now + REVEAL_WINDOW;
        }
        let pending = self.editor.reveal_pending;
        if !offset_changed && !caret_changed && !pending {
            return false;
        }
        self.editor.reveal_pending = false;
        let reveal = caret_changed || pending || (offset_changed && now < self.editor.reveal_until);
        if self.apply_scroll_sync(tx, reveal) {
            // The preview has not measured the new anchor yet: retry on the
            // next frame, once the commit that places it has laid it out.
            self.editor.last_offset = None;
        }
        false
    }

    /// Reveals the caret's block after a preview parse or an edit (E-07).
    ///
    /// Kept as the parse completion's entry point; [`Self::poll_scroll_sync`]
    /// handles source scrolling and caret moves.
    pub(crate) fn sync_preview_to_caret(&mut self, tx: &mut Tx) {
        if self.editor.sync_suppressed_until.is_some_and(|until| Instant::now() < until) {
            // Edit entry or a very recent session: reveal once the entry
            // window is over, so a parse that lands early is not lost.
            self.editor.reveal_pending = true;
            return;
        }
        self.editor.reveal_pending = false;
        self.editor.last_offset = Some(self.editor.scroll.get().offset);
        if self.apply_scroll_sync(tx, true) {
            // Retry once the preview has measured the caret's block.
            self.editor.last_caret = None;
        } else {
            self.editor.last_caret = self.editor_caret_line();
        }
    }

    /// Maps the editor's current top and caret onto the preview and requests
    /// one scroll of `article-scroll` (E-07).
    /// True when the preview had not measured the anchor yet and the next
    /// frame should refine the placement.
    fn apply_scroll_sync(&mut self, tx: &mut Tx, reveal: bool) -> bool {
        let Some(sync) = self.compute_scroll_sync(reveal) else { return false };
        let fallback = sync.fallback;
        tx.scroll_key_to(sync.key.clone(), sync.y);
        self.editor.last_sync = Some(sync);
        fallback
    }

    fn compute_scroll_sync(&self, reveal: bool) -> Option<PreviewSync> {
        let tab = self.active_tab()?;
        if tab.view.mode != Mode::Editor {
            return None;
        }
        tab.view.editor.as_ref()?;
        tab.document()?;
        let doc = tab.view.reader.clone()?;
        let top = self.editor.editor_top.get();
        let reported = self.editor.scroll.get();
        let (lines, span) = self.editor_span()?;
        let line = top_line(&top, reported.offset, span, lines, self.editor_metrics().line_height);
        let geometry = self.editor.preview.borrow();
        // No preview on screen (narrow source-only, or a preview that has not
        // been laid out yet): there is nothing to place.
        if geometry.blocks.is_empty() && geometry.viewport <= OFFSET_EPSILON {
            return None;
        }
        let base = preview_placement(&doc, line, reported.is_top(), &geometry)?;
        if !reveal {
            return Some(PreviewSync { line, key: base.key, y: base.y, reveal: false, fallback: base.fallback });
        }
        let caret = self.editor_caret_line().unwrap_or_else(|| (line.floor() as usize).min(lines.saturating_sub(1)));
        let base_fallback = base.fallback;
        let placement = reveal_placement(&doc, caret as f32, &base, &geometry).unwrap_or(base);
        Some(PreviewSync {
            line: caret as f32,
            key: placement.key,
            y: placement.y,
            reveal: true,
            fallback: base_fallback || placement.fallback,
        })
    }
}

/// Where a block's next anchor sits: the following block's top, else the
/// article's bottom (or the block's own bottom when the article is not laid
/// out yet).
fn next_top_of_last(index: usize, doc: &MdDoc, rect: [f32; 4], content_bottom: f32) -> f32 {
    let bottom = rect[1] + rect[3];
    if index.saturating_add(1) < doc.blocks.len() { bottom } else { content_bottom.max(bottom) }
}

/// A table's measured box and sticky header for `index`, when it is a table.
fn table_geometry(doc: &MdDoc, cx: &Cx<WindowState>, index: usize) -> Option<PreviewTable> {
    if !matches!(doc.blocks.get(index), Some(MdBlock::Table(_))) {
        return None;
    }
    let key = doc.block_key(index)?;
    let direct = format!("{key}/table");
    let wide = format!("{key}/scroll/table");
    let rect = cx.layout(&direct).or_else(|| cx.layout(&wide))?;
    let head =
        cx.layout(&format!("{direct}/head")).or_else(|| cx.layout(&format!("{wide}/head"))).map_or(0.0, |head| head[3]);
    Some(PreviewTable { top: rect[1], bottom: rect[1] + rect[3], head })
}
