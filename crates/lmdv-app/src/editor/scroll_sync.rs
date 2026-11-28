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
//! * The same view publishes [`VisualLines`]: the exact top of every source
//!   line's first visual line, measured with the field's own font, spans and
//!   wrap width (`rune::text::measure`, the runtime's shaper). Wrapped prose
//!   therefore maps by the line it is drawn on, not by a count of visual
//!   lines, and a block row maps inside its own block the same way.
//! * The preview view publishes [`PreviewGeometry`]: the content y and height
//!   of the blocks around the editor's top and caret, their heading, the front
//!   matter card, every top-level table that can stick at the viewport's top,
//!   and the section-line reserve actually drawn.
//!
//! The remaining approximation is the preview side of a wrapped source line:
//! the block anchor interpolates a fractional source line across the block's
//! source-line span; the preview itself has no per-source-line geometry.

use std::ops::Range;
use std::time::{Duration, Instant};

use lmdv_md::{Block as MdBlock, MdDoc};
use lmdv_render::{DOC_KEY, FRONT_MATTER_KEY, heading_key};
use rune::{Cx, Family, Span, TextStyle, Tx};

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
/// E-34: the least blank space after the last source line.
pub const TAIL_MIN: f32 = 160.0;
/// E-34: the tail as a share of the source viewport's height.
pub const TAIL_RATIO: f32 = 0.6;
/// Below this many points two scroll offsets are the same line.
const OFFSET_EPSILON: f32 = 0.5;

/// The blank space after the last source line (E-34).
///
/// The editor may scroll past its end so the preview can follow to the end of
/// the document; 60% of the viewport, never less than [`TAIL_MIN`]. A frame
/// without a measured viewport still gets the minimum, so the scrollable extent
/// does not change once it is measured.
pub fn tail_space(viewport: f32) -> f32 {
    if viewport.is_finite() && viewport > 0.0 { (viewport * TAIL_RATIO).max(TAIL_MIN) } else { TAIL_MIN }
}

/// The source line a double-click at `y` inside a measured preview block maps
/// to (E-35's preview-height caret).
///
/// `top`/`height` are the block's article-content box, `first_line`/`end_line`
/// its source span (the end is exclusive), and `y` the press in the same
/// content points. The fraction of the block's height is the fraction of its
/// source lines, clamped into the block: a click on the block's padding lands on
/// its first or last line rather than doing nothing. Without a measurement the
/// block's first line is the fallback.
pub fn line_from_preview_y(top: f32, height: f32, first_line: usize, end_line: usize, y: f32) -> usize {
    let last = end_line.saturating_sub(1).max(first_line);
    if !(height.is_finite() && height > 0.0) || !y.is_finite() {
        return first_line;
    }
    let t = ((y - top) / height).clamp(0.0, 1.0);
    let line = first_line as f32 + t * last.saturating_sub(first_line) as f32;
    (line.round() as usize).clamp(first_line, last)
}

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

/// A preview anchor that is not an `MdDoc` block (the front matter card).
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewAnchor {
    /// The element's key.
    pub key: String,
    /// The anchor's top in article content points.
    pub top: f32,
    /// The anchor's height, points.
    pub height: f32,
    /// The next anchor's top: the first block's top, or the card's bottom.
    pub next_top: f32,
}

impl PreviewAnchor {
    /// The content points from `top` to the next anchor, for interpolation.
    pub fn span(&self) -> f32 {
        (self.next_top - self.top).max(1.0)
    }
}

/// The exact visual-line geometry of a source field.
///
/// `tops[i]` is where source line `i`'s first visual line begins, in points
/// from the field's text top, and `tops` has one extra entry for the text's
/// bottom. A wrapped line therefore owns the whole band between two entries:
/// mapping a point inside it gives that source line plus the fraction of the
/// band it sits at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VisualLines {
    /// The source line `tops[0]` covers: 0 for the single field, the visible
    /// block's first line in block mode.
    pub first_line: usize,
    /// Source line tops, then the text's bottom.
    pub tops: Vec<f32>,
    /// Whether the values came from real text shaping.
    pub measured: bool,
}

impl VisualLines {
    /// Measures `text` with the field's own `spans`, `style` and wrap `width`.
    ///
    /// Every source line is measured on its own, so a wrapped line takes the
    /// height the field draws it at (the same shaper and fonts: a view's
    /// measure uses the window's text system). `first_line` is the source line
    /// `text` starts at.
    pub fn measure(text: &str, spans: &[Span], style: TextStyle, width: f32, first_line: usize) -> VisualLines {
        let line_height = style.line_height.max(1.0);
        if !(width.is_finite() && width > 0.0) {
            return VisualLines::default();
        }
        let mut tops = Vec::with_capacity(text.bytes().filter(|byte| *byte == b'\n').count() + 2);
        let mut y = 0.0f32;
        tops.push(y);
        let mut start = 0usize;
        for line in text.split('\n') {
            let end = start + line.len();
            let height = if line.is_empty() {
                line_height
            } else {
                let clipped = clip_spans(spans, start, end);
                let measured = rune::text::measure(line, &clipped, style, Some(width)).1;
                if measured.is_finite() && measured > 0.0 { measured.max(line_height) } else { line_height }
            };
            y += height;
            tops.push(y);
            start = end + 1;
        }
        VisualLines { first_line, tops, measured: true }
    }

    /// `true` when there is no usable measurement.
    pub fn is_empty(&self) -> bool {
        !self.measured || self.tops.len() < 2
    }

    /// The fractional source line at `y` points below the text's top, clamped
    /// into the measured text.
    pub fn line_at(&self, y: f32) -> Option<f32> {
        if self.is_empty() {
            return None;
        }
        let y = y.max(0.0);
        let last = self.tops.len() - 2;
        let index = self.tops.partition_point(|top| *top <= y).saturating_sub(1).min(last);
        let top = self.tops[index];
        let bottom = self.tops[index + 1];
        let fraction = if bottom > top + 1e-4 { ((y - top) / (bottom - top)).clamp(0.0, 1.0) } else { 0.0 };
        Some(self.first_line as f32 + index as f32 + fraction)
    }

    /// How many visual lines source line `line` takes, when measured.
    #[cfg(test)]
    pub fn visual_count(&self, line: usize) -> Option<f32> {
        let index = line.checked_sub(self.first_line)?;
        let top = *self.tops.get(index)?;
        let bottom = *self.tops.get(index + 1)?;
        Some((bottom - top).max(0.0))
    }
}

/// A source line's spans clipped to one line, in line-local byte offsets.
fn clip_spans(spans: &[Span], start: usize, end: usize) -> Vec<Span> {
    let mut out = Vec::new();
    for span in spans {
        let from = (span.start as usize).max(start);
        let to = (span.end as usize).min(end);
        if to <= from {
            continue;
        }
        let mut clipped = span.clone();
        clipped.start = (from - start) as u32;
        clipped.end = (to - start) as u32;
        // The source has no inline images; a stray one would not measure.
        clipped.image = None;
        out.push(clipped);
    }
    out
}

/// The measured visual lines a view keeps between frames, keyed by what can
/// invalidate them (session revision, wrap width, font, and the block they
/// cover in block mode).
#[derive(Clone, Debug, Default)]
pub struct VisualCache {
    key: Option<VisualKey>,
    lines: VisualLines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisualKey {
    revision: u64,
    width: u32,
    size: u32,
    weight: u32,
    family: Family,
    line_height: u32,
    first_line: usize,
}

impl VisualCache {
    /// The last measured lines.
    pub fn get(&self) -> &VisualLines {
        &self.lines
    }

    /// Re-measures when the session revision, font, width or block changed.
    pub fn refresh(
        &mut self,
        revision: u64,
        text: &str,
        spans: &[Span],
        style: TextStyle,
        width: f32,
        first_line: usize,
    ) {
        let key = VisualKey {
            revision,
            width: width.to_bits(),
            size: style.size.to_bits(),
            weight: style.weight.to_bits(),
            family: style.family,
            line_height: style.line_height.to_bits(),
            first_line,
        };
        if self.key == Some(key) {
            return;
        }
        self.lines = VisualLines::measure(text, spans, style, width, first_line);
        self.key = Some(key);
    }

    /// Drops the measurement (leaving edit mode, a new session).
    pub fn clear(&mut self) {
        self.key = None;
        self.lines = VisualLines::default();
    }
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
    /// The block's drawn text boxes in article content points, in reading
    /// order: where a double-click is on text rather than on the block's
    /// padding (E-35). Empty when the text elements were not measured.
    pub text_boxes: Vec<[f32; 4]>,
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
    /// Every top-level table that can hold the viewport's top, measured with
    /// its sticky header.
    pub tables: Vec<PreviewTable>,
    /// The front matter card, when the document has one.
    pub front_matter: Option<PreviewAnchor>,
    /// The section line's measured height when it floats over this viewport
    /// (O-07 keeps it out of edit mode, so the editor's preview measures 0).
    pub reserve: f32,
    /// The article's bottom in content points.
    pub content_bottom: f32,
    /// The article viewport's top in unscrolled window points, so a pointer
    /// point can be turned into article content points (E-35).
    pub viewport_top: f32,
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
    /// scroll sync does, so leaving a table does not jump. Every table that
    /// can stick was measured, so a table crossing the top is exact.
    pub fn cover_at(&self, y: f32, after: bool) -> f32 {
        let mut cover = 0.0f32;
        for table in &self.tables {
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
///
/// This is the fallback for a frame that has no [`VisualLines`] measurement
/// yet; [`mapped_top_line`] is the exact mapping.
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

/// [`top_line`] from the field's measured visual lines (E-07).
///
/// `visual` must cover the visible field (the single field or the visible
/// block). Its source line must agree with `span`'s start, or the map belongs
/// to another block/frame and the measured fallback is used.
pub fn mapped_top_line(
    top: &EditorTop,
    visual: Option<&VisualLines>,
    offset: f32,
    span: (usize, usize),
    lines: usize,
    line_height: f32,
) -> f32 {
    let last = lines.saturating_sub(1) as f32;
    let expected = if top.block.is_some() { span.0 } else { 0 };
    if let Some(visual) = visual.filter(|visual| visual.measured && visual.first_line == expected)
        && let Some(line) = visual.line_at(offset - top_text_top(top))
    {
        // A block field's text ends with the newline before the next block, so
        // its last display line is the *next* source line; the block owns its
        // lines up to `span.1` exclusive, and never a line the field draws
        // beyond the document.
        let upper = if top.block.is_some() { (span.1 as f32).min(last) } else { last };
        return line.clamp(span.0 as f32, upper);
    }
    top_line(top, offset, span, lines, line_height)
}

/// The points below the editor viewport's top where the measured field's text
/// begins.
fn top_text_top(top: &EditorTop) -> f32 {
    if top.block.is_some() {
        top.block_top
    } else if top.measured {
        top.start
    } else {
        EDITOR_PAD_TOP
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
    let first_block = doc.block_spans.first();
    if doc.front_matter.is_some() && first_block.is_none_or(|span| byte < span.source.start) {
        return front_matter_placement(doc, line, at_top, geometry);
    }
    let index = block_at_or_above(doc, floor)?;
    let key = doc.block_key(index)?.to_owned();
    let Some(block) = geometry.block(index) else {
        // The preview has not measured this block yet (a jump across blocks):
        // land its top at the margin and refine on the next measurement.
        return Some(Placement { key, y: -SYNC_MARGIN, target: 0.0, index: Some(index), fallback: true });
    };
    let t = anchor_span(doc, index).map_or(0.0, |(start, end)| span_fraction(line, start, end));
    let y_line = block.top + t * block.span();
    let cover = geometry.cover_at(y_line, true);
    let raw = y_line - SYNC_MARGIN - geometry.reserve - cover;
    let target = if at_top { 0.0 } else { clamp_target(raw, geometry) };
    // `scroll_key_to` takes the anchor's top *below* the viewport top; the
    // viewport's top sits at content `target`, so the anchor's top is
    // `block.top - target` below it (negative: scrolled into the block).
    Some(Placement { key, y: block.top - target, target, index: Some(index), fallback: false })
}

/// The front matter card's interpolation: line 0 to the first block's line
/// across the card's own top and height.
fn front_matter_placement(doc: &MdDoc, line: f32, at_top: bool, geometry: &PreviewGeometry) -> Option<Placement> {
    let next_line = doc.block_spans.first().map_or(1, |span| span.line).max(1);
    let Some(card) = geometry.front_matter.as_ref() else {
        // The card is drawn in the article but not measured yet: place its top
        // at the margin and refine on the next frame.
        return Some(Placement {
            key: FRONT_MATTER_KEY.to_owned(),
            y: -SYNC_MARGIN,
            target: 0.0,
            index: None,
            fallback: true,
        });
    };
    let t = span_fraction(line, 0, next_line);
    let y_line = card.top + t * card.span();
    let raw = y_line - SYNC_MARGIN - geometry.reserve;
    let target = if at_top { 0.0 } else { clamp_target(raw, geometry) };
    Some(Placement { key: card.key.clone(), y: card.top - target, target, index: None, fallback: false })
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
        let above = SYNC_MARGIN + geometry.reserve + geometry.cover_at(caret_top, false);
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
        let visual = self.editor.visual.borrow();
        Some(mapped_top_line(&top, Some(visual.get()), offset, span, lines, self.editor_metrics().line_height))
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

    /// Records where the source field's first line sits and the field's exact
    /// visual lines (called by the view; E-07).
    ///
    /// `spans` are the field's own syntax spans and `font` its style, so the
    /// measurement wraps exactly as the field draws (the view's measure uses
    /// the window's text system). The measurement is cached by session
    /// revision, width, font and block.
    pub(crate) fn publish_editor_top(&self, cx: &Cx<WindowState>, spans: &[Span], font: TextStyle) {
        let Some(tab) = self.active_tab() else { return };
        let Some(editor) = tab.view.editor.as_ref() else { return };
        let Some(doc) = tab.document() else { return };
        let viewport = cx.layout(SOURCE_SCROLL_KEY);
        let revision = editor.revision();
        // Without a preview beside the source there is nothing to sync, and
        // typing must not pay for measuring lines nobody will map.
        let sync_active = !self.editor.preview_narrow && self.editor.preview_visible && tab.view.reader.is_some();
        if editor.block_mode_active() {
            let index = self
                .editor
                .visible_blocks
                .as_ref()
                .map(|range| range.start)
                .or_else(|| editor.active_block())
                .unwrap_or(0);
            let field = cx.layout(&crate::block_ui::block_field_key(index));
            let top = match (viewport, field) {
                (Some(view), Some(field)) => EditorTop {
                    measured: true,
                    start: field[1] - view[1],
                    block: Some(index),
                    block_top: field[1] - view[1],
                    block_height: field[3].max(1.0),
                },
                _ => EditorTop { block: Some(index), ..EditorTop::default() },
            };
            self.editor.editor_top.set(top);
            let block = editor.block_editor().and_then(|blocks| blocks.block(index));
            match (field, block) {
                (Some(field), Some(block)) if sync_active => {
                    let first_line = doc.text().line_of(block.range.start);
                    let mut visual = self.editor.visual.borrow_mut();
                    visual.refresh(revision, &block.edit.text, spans, font, field[2], first_line);
                }
                _ => self.editor.visual.borrow_mut().clear(),
            }
        } else {
            let field = cx.layout(SOURCE_FIELD_KEY);
            let top = match (viewport, field) {
                (Some(view), Some(field)) => {
                    EditorTop { measured: true, start: field[1] - view[1], ..EditorTop::default() }
                }
                _ => EditorTop { start: EDITOR_PAD_TOP, ..EditorTop::default() },
            };
            self.editor.editor_top.set(top);
            match field {
                Some(field) if sync_active => {
                    let mut visual = self.editor.visual.borrow_mut();
                    visual.refresh(revision, &editor.edit.text, spans, font, field[2], 0);
                }
                _ => self.editor.visual.borrow_mut().clear(),
            }
        }
    }

    /// Records the preview blocks around the editor's top and caret, every
    /// table that can stick, the front matter card and the section-line
    /// reserve (called by the reader view while editing; E-07).
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
        // E-35: the block a double-click last moved over, so the next frame
        // measures where its text really is.
        if let Some(pressed) = self.editor.preview_block.get() {
            wanted.extend(pressed.saturating_sub(1)..=pressed.saturating_add(1).min(last));
        }
        wanted.sort_unstable();
        wanted.dedup();
        let content_bottom = cx.layout(DOC_KEY).map_or(0.0, |rect| rect[1] + rect[3]);
        let viewport_rect = cx.layout(ARTICLE_SCROLL_KEY);
        let viewport = viewport_rect.map_or(0.0, |rect| rect[3]);
        let viewport_top = viewport_rect.map_or(0.0, |rect| rect[1]);
        let scroll = self.scroll_snapshot();
        // Layout reads are unscrolled window points: the article viewport's own
        // top plus the reported content offset is where its visible top sits.
        let visible_top = viewport_top + scroll.offset;
        let mut blocks = Vec::with_capacity(wanted.len());
        for index in wanted {
            let Some(key) = doc.block_key(index).map(str::to_owned) else { continue };
            let Some(rect) = cx.layout(key.as_str()) else { continue };
            let next_top = doc
                .block_key(index.saturating_add(1))
                .and_then(|next| cx.layout(next))
                .map_or_else(|| next_top_of_last(index, doc, rect, content_bottom), |next| next[1]);
            let heading = doc.headings.iter().position(|heading| heading.block == index).and_then(|heading| {
                let key = heading_key(heading);
                cx.layout(&key).map(|rect| PreviewPart { key, top: rect[1], height: rect[3] })
            });
            let mut text_boxes = preview_text_boxes(&key, cx);
            if let Some(part) = &heading {
                // A heading keeps its `h{n}` key rather than a text identity;
                // its own band still counts as text for the padding guard.
                text_boxes.push([0.0, part.top, 1.0, part.height]);
            }
            blocks.push(PreviewBlock { index, key, top: rect[1], height: rect[3], next_top, heading, text_boxes });
        }
        // Every table whose rows can hold the viewport's top, not only the
        // ones near the source caret: a table crossing the top keeps its header
        // stuck there, and its rows may reach far past the editor's line.
        let tables = measure_tables(doc, cx, visible_top, viewport);
        let front_matter = doc.front_matter.is_some().then(|| front_matter_geometry(doc, cx)).flatten();
        // O-07 keeps the section line out of edit mode; the reserve is the
        // overlay's measured height whenever a frame does draw it.
        let reserve = cx.layout(crate::section::SECTION_LINE_KEY).map_or(0.0, |rect| rect[3]).max(0.0);
        *self.editor.preview.borrow_mut() = PreviewGeometry {
            blocks,
            tables,
            front_matter,
            reserve,
            content_bottom,
            viewport_top,
            viewport,
            max: scroll.max,
        };
    }

    /// Drops the measured preview geometry (leaving edit mode, or a source-only
    /// frame that does not draw the preview).
    pub(crate) fn clear_preview_geometry(&self) {
        *self.editor.preview.borrow_mut() = PreviewGeometry::default();
    }

    /// Remembers the virtual rows the source is showing (block mode) (OI-102).
    ///
    /// The range itself is only bookkeeping: the spell checker reads it and the
    /// editor top picks its first row. Rune already remounts the virtual window
    /// itself, so asking for another view pass on every reported range is the
    /// expensive part. A rebuild is needed only when the *first* row moved and
    /// E-07 is actually measuring that row against a preview; a tail-only
    /// change (resize, overscan) or a source-only pane needs no rebuild.
    pub(crate) fn note_visible_blocks(&mut self, range: Range<usize>) -> bool {
        let first_changed = self.editor.visible_blocks.as_ref().is_none_or(|old| old.start != range.start);
        let sync_active = self.active_tab().is_some_and(|tab| {
            tab.view.mode == Mode::Editor
                && !self.editor.preview_narrow
                && self.editor.preview_visible
                && tab.view.reader.is_some()
        });
        self.editor.visible_blocks = Some(range);
        first_changed && sync_active
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
        let visual = self.editor.visual.borrow();
        let line =
            mapped_top_line(&top, Some(visual.get()), reported.offset, span, lines, self.editor_metrics().line_height);
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

/// The front matter card's box and the first block's top.
fn front_matter_geometry(doc: &MdDoc, cx: &Cx<WindowState>) -> Option<PreviewAnchor> {
    let rect = cx.layout(FRONT_MATTER_KEY)?;
    let next_top = doc.block_key(0).and_then(|key| cx.layout(key)).map_or(rect[1] + rect[3], |next| next[1]);
    Some(PreviewAnchor { key: FRONT_MATTER_KEY.to_owned(), top: rect[1], height: rect[3], next_top })
}

/// How many drawn text boxes one preview block may contribute (E-35).
const MAX_PREVIEW_TEXT_BOXES: usize = 128;

/// The measured boxes of a block's drawn texts, in article content points.
///
/// The renderer keys every text `{block}-text-{index}`, and headings keep
/// `h{n}`; the probe stops at the first key the block did not build, so an
/// ordinary paragraph costs one read.
fn preview_text_boxes(key: &str, cx: &Cx<WindowState>) -> Vec<[f32; 4]> {
    let mut boxes = Vec::new();
    for index in 0..MAX_PREVIEW_TEXT_BOXES {
        let Some(rect) = cx.layout(&format!("{key}-text-{index}")) else { break };
        boxes.push(rect);
    }
    boxes
}

/// Every top-level table whose rows can hold `top_offset` under the viewport,
/// with its sticky header.
///
/// A table sticks from its first row until its bottom passes the viewport's
/// top; one more header height above the top is measured so the `after` fade
/// has data on the frame it leaves.
fn measure_tables(doc: &MdDoc, cx: &Cx<WindowState>, top_offset: f32, viewport: f32) -> Vec<PreviewTable> {
    let mut tables = Vec::new();
    let viewport_bottom = top_offset + viewport;
    for (index, block) in doc.blocks.iter().enumerate() {
        if !matches!(block, MdBlock::Table(_)) {
            continue;
        }
        let Some(key) = doc.block_key(index) else { continue };
        // The block wrapper is the table's own box (its action bar floats over
        // it); the sticky header lives under the app's `TableWire` wrapper, or
        // directly under the block when no wire is set.
        let Some(rect) = cx.layout(key) else { continue };
        let (top, bottom) = (rect[1], rect[1] + rect[3]);
        let head = table_head_paths(key).iter().find_map(|path| cx.layout(path)).map_or(0.0, |r| r[3]);
        // Tables can hold the top from their first row to their bottom; one
        // more header height above the top keeps the `after` fade exact.
        if bottom < top_offset - head || top > viewport_bottom {
            continue;
        }
        tables.push(PreviewTable { top, bottom, head });
    }
    tables
}

/// Where a top-level table's sticky header can be drawn: under the app's
/// `TableWire` wrapper (keyed `table-block-{key}`), in its own box, or beneath
/// the sideways scroller a wide table uses; the bare paths cover a wire-less
/// renderer tree.
pub(crate) fn table_head_paths(key: &str) -> [String; 4] {
    [
        format!("{key}/table-block-{key}/table/head"),
        format!("{key}/table-block-{key}/scroll/table/head"),
        format!("{key}/table/head"),
        format!("{key}/scroll/table/head"),
    ]
}
