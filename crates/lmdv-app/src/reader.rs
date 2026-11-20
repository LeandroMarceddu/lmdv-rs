//! The reading surface: one scroll viewport around the `lmdv-render` article.
//!
//! `lmdv-md` parses the document on a worker into an [`MdDoc`](lmdv_md::MdDoc);
//! [`lmdv_render::render`] turns that model into the article with stable block
//! (`b{i}`), heading (`h{i}`) and footnote keys. This module owns what is the
//! app's rather than the renderer's:
//!
//! * the scroll viewport and its offset report (R-12);
//! * the reading column's width and padding (R-02);
//! * the scroll-spy lookup the outline uses (O-03);
//! * the viewport-to-heading anchor that makes a remembered position survive
//!   edits above it (R-13).
//!
//! Views stay pure: the only side effects are the scroll [`Cell`] and the
//! viewport anchor [`Cell`] that `poll` observes for positions.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use lmdv_md::MdDoc;
use lmdv_render::{DocTheme, Highlight, ImageResolver, MathResolver, RenderOptions, RenderWires, heading_key, render};
use rune::{Color, Cx, El, KeyEvent, ScrollSpec, Tx, col};
use rune_kit::markdown::MarkdownStyle;

use crate::window::WindowState;

/// The article's scroll viewport key. Scroll requests and `--line` target it.
pub const ARTICLE_SCROLL_KEY: &str = "article-scroll";
/// The reading column's key.
pub const ARTICLE_KEY: &str = "article";
/// Reading column width in points, padding included (R-02).
pub const READER_MAX_WIDTH: f32 = 760.0;
/// Reading column side padding in points (R-02).
pub const READER_PAD_X: f32 = 48.0;
/// Reading column top padding in points (R-02).
pub const READER_PAD_TOP: f32 = 40.0;
/// Reading column bottom padding in points (R-02).
pub const READER_PAD_BOTTOM: f32 = 96.0;
/// Documents above this size keep the source preview: the parser cap from
/// `docs/ARCHITECTURE.md` §12 (5 MB / 200k blocks is the block side).
pub const PARSE_MAX_BYTES: usize = 5 * 1024 * 1024;
/// Documents above this line count keep the source preview.
pub const PARSE_MAX_LINES: usize = 200_000;
/// A heading is active once its top is above this line below the viewport top
/// (O-03). In the last viewport of the document the line slides down, so the
/// trailing headings — which can never reach the top — still become active.
pub const HEADING_ACTIVATION: f32 = 90.0;

/// What one `on_scroll_offset` report says beyond the position (R-06/O-04).
///
/// `report` counts the reports the viewport made; `by_hand` is Rune's own
/// verdict on the movement that caused it — a wheel, a drag, a key or an app's
/// *instant* request, but not a glide such as a heading jump. The pin uses both:
/// a hand report releases it at once, and any report after the glide has been
/// quiet releases it too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollSignal {
    /// Reports made so far, wrapped; a change means a new report.
    pub report: u64,
    /// A hand or an instant request moved the viewport.
    pub by_hand: bool,
}

/// A scroll viewport's position, copied out of [`rune::ScrollPos`] so the
/// reader view can report it through a `Cell` without re-running the view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollSnapshot {
    /// Content offset from the top, points.
    pub offset: f32,
    /// Maximum scrollable offset, points.
    pub max: f32,
    /// Viewport height, points.
    pub viewport: f32,
}

impl ScrollSnapshot {
    /// Copies the vertical parts of a runtime scroll report.
    pub fn from_pos(at: rune::ScrollPos) -> Self {
        fn clean(value: f32) -> f32 {
            if value.is_finite() { value.max(0.0) } else { 0.0 }
        }
        Self { offset: clean(at.offset[1]), max: clean(at.max[1]), viewport: clean(at.viewport[1]) }
    }

    /// Whether the viewport sits at its top.
    pub fn is_top(&self) -> bool {
        self.offset <= 0.5
    }
}

/// The nearest heading above a viewport, for the scroll spy and for positions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportAnchor {
    /// Index into [`MdDoc::headings`], when a heading is above the viewport.
    pub heading: Option<usize>,
    /// How many points the heading's top sits above the viewport top.
    pub offset: f32,
    /// Whether the viewport is at the document top.
    pub top: bool,
}

/// The nearest heading above `viewport_top`, from the last frame's layout (R-12).
///
/// `layout` returns the window rect `[x, y, w, h]` of a keyed element. A
/// heading that is not laid out (a footnote definition, or a block the frame
/// has not mounted) is skipped; the first heading below the viewport ends the
/// walk. `top` reports an at-top viewport, which needs no heading.
pub fn anchor_from_layouts<F>(doc: &MdDoc, viewport_top: f32, top: bool, layout: F) -> ViewportAnchor
where
    F: Fn(&str) -> Option<[f32; 4]>,
{
    if top {
        return ViewportAnchor { top: true, ..ViewportAnchor::default() };
    }
    let mut best: Option<(usize, f32)> = None;
    for (index, heading) in doc.headings.iter().enumerate() {
        // A footnote definition's heading is not a document block; every
        // drawn heading, including h5/h6, can anchor a position (R-12).
        if heading.block == usize::MAX {
            continue;
        }
        let Some(rect) = layout(&heading_key(index)) else { continue };
        if rect[1] > viewport_top + 0.5 {
            break;
        }
        best = Some((index, (viewport_top - rect[1]).max(0.0)));
    }
    match best {
        Some((heading, offset)) => ViewportAnchor { heading: Some(heading), offset, top: false },
        None => ViewportAnchor::default(),
    }
}

/// Compute [`ViewportAnchor`] from the window's last layout (R-12).
///
/// The layout reads are recorded as view reads, so the frame re-runs when a
/// scroll moves the headings — exactly what the scroll spy needs.
pub fn viewport_anchor<S>(cx: &Cx<S>, doc: &MdDoc, scroll: ScrollSnapshot) -> ViewportAnchor {
    let Some(viewport) = cx.window_layout(ARTICLE_SCROLL_KEY) else {
        return ViewportAnchor { top: scroll.is_top(), ..ViewportAnchor::default() };
    };
    anchor_from_layouts(doc, viewport[1], scroll.is_top(), |key| cx.window_layout(key))
}

/// The last heading whose top is above the outline activation line, or the
/// first heading while layout is not ready yet (O-03).
///
/// The activation line is [`HEADING_ACTIVATION`] below the viewport's top, and
/// slides down to the viewport's bottom edge through the document's last
/// viewport of scroll, so the trailing headings — the ones that can never reach
/// the top — still become the active row at the end (OI-017).
pub fn active_heading<S>(cx: &Cx<S>, doc: &MdDoc, scroll: ScrollSnapshot) -> Option<String> {
    /// Keep the per-frame layout reads bounded for generated documents.
    const SPY_MAX: usize = 500;
    let first = doc.headings.iter().enumerate().find(|(_, heading)| heading.in_toc).map(|(index, _)| index);
    let Some(viewport) = cx.window_layout(ARTICLE_SCROLL_KEY) else {
        return first.map(heading_key);
    };
    let activation = viewport[1] + activation_line(viewport[3], scroll);
    let mut active = None;
    for (index, heading) in doc.headings.iter().enumerate().take(SPY_MAX) {
        if !heading.in_toc {
            continue;
        }
        let Some(rect) = cx.window_layout(&heading_key(index)) else { continue };
        if rect[1] > activation {
            break;
        }
        active = Some(heading_key(index));
    }
    active.or_else(|| first.map(heading_key))
}

/// Where the outline's activation line sits below the viewport's top (O-03).
///
/// Far from the end it is [`HEADING_ACTIVATION`]; within the last viewport of
/// scroll it slides down to the viewport's bottom edge, as the original's
/// `activationLine` does, so at maximum scroll the last reachable heading is
/// active instead of the last one that could reach the top.
pub fn activation_line(viewport: f32, scroll: ScrollSnapshot) -> f32 {
    if viewport <= 0.0 || scroll.max <= 0.0 {
        return HEADING_ACTIVATION;
    }
    let remaining = (scroll.max - scroll.offset).max(0.0);
    let tail = (1.0 - remaining / viewport).clamp(0.0, 1.0);
    HEADING_ACTIVATION + tail * (viewport - HEADING_ACTIVATION)
}

/// The outline's headings: levels 1 to 4 that a rendered block owns (O-01).
pub fn toc_headings(doc: &MdDoc) -> impl Iterator<Item = (usize, &lmdv_md::Heading)> {
    doc.headings.iter().enumerate().filter(|(_, heading)| heading.in_toc && heading.block != usize::MAX)
}

/// Whether any heading belongs in the outline.
pub fn has_toc_headings(doc: &MdDoc) -> bool {
    toc_headings(doc).next().is_some()
}

/// The source byte range of `line` (1-based, without its newline), clamped to
/// the document; `--line` paints this while the flash lasts.
pub fn line_range(doc: &MdDoc, line: usize) -> Option<Range<usize>> {
    let index = line.max(1) - 1;
    let start = *doc.line_starts.get(index)? as usize;
    let end =
        doc.line_starts.get(index + 1).map_or(doc.source.len(), |next| (*next as usize).saturating_sub(1)).max(start);
    Some(start..end)
}

/// The block key a 1-based source line lands on (`b{i}`), for `--line`.
pub fn block_key_at_line(doc: &MdDoc, line: usize) -> Option<String> {
    let index = (line.max(1) - 1).min(doc.line_count().saturating_sub(1));
    let byte = *doc.line_starts.get(index)? as usize;
    let block = doc.block_index_at(byte)?;
    doc.block_key(block).map(str::to_owned)
}

/// The top-level block under the reading viewport's top edge (OI-098).
///
/// `Cx::layout` is the unscrolled layout, so the target is the viewport's own
/// unscrolled top plus the reported scroll offset: no block moves as the page
/// scrolls and the probe costs no view run per frame. `probe` returns a block's
/// unscrolled bottom; the blocks are in document order, so a binary search over
/// them is enough (twelve reads for four thousand blocks). The block at the top
/// is the first one whose bottom is still below the target: the gaps between
/// blocks (their margins) would otherwise name a block that has already gone.
pub fn reading_block<S>(cx: &Cx<S>, doc: &MdDoc, scroll: ScrollSnapshot) -> Option<usize> {
    let viewport = cx.layout(ARTICLE_SCROLL_KEY)?;
    let target = viewport[1] + scroll.offset;
    // A frame before the first layout measures nothing; the fallback in
    // `block_at_y` must not turn that into "the last block".
    let seen = std::cell::Cell::new(false);
    let found = block_at_y(doc.blocks.len(), target, |index| {
        let key = doc.block_key(index)?;
        let bottom = cx.layout(key).map(|rect| rect[1] + rect[3]);
        if bottom.is_some() {
            seen.set(true);
        }
        bottom
    });
    found.filter(|_| seen.get())
}

/// The first block whose bottom is below `y` (OI-098).
///
/// `count` blocks are probed in order through `bottom`; a block that cannot be
/// measured counts as one that has gone, and the last block is the fallback
/// when every one is. The pure core of [`reading_block`].
pub fn block_at_y(count: usize, y: f32, bottom: impl Fn(usize) -> Option<f32>) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let mut measured = false;
    let mut lo = 0usize;
    let mut hi = count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match bottom(mid) {
            Some(at) => {
                measured = true;
                if at <= y {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            None => hi = mid,
        }
    }
    // With no measurement at all the caller cannot tell which block is at the
    // top; the last one is the safe fallback. `reading_block` filters this out
    // with its own `seen` flag when the layout has not measured anything yet.
    if measured { Some(lo.min(count - 1)) } else { Some(count - 1) }
}

/// The reading column's side and top padding at `width` (R-02).
///
/// The section line uses the same steps so its path begins where the text does.
pub fn column_padding(width: f32) -> (f32, f32) {
    if width <= 420.0 {
        (14.0, 24.0)
    } else if width <= 640.0 {
        (20.0, READER_PAD_TOP)
    } else {
        (READER_PAD_X, READER_PAD_TOP)
    }
}

/// The article inside its scroll viewport.
pub struct ReaderView<'a> {
    doc: &'a MdDoc,
    theme: &'a DocTheme,
    style: &'a MarkdownStyle,
    math: &'a dyn MathResolver,
    images: &'a dyn ImageResolver,
    wires: RenderWires,
    details: Option<&'a dyn lmdv_render::DetailsState>,
    highlights: Vec<Highlight>,
    scroll: Rc<Cell<ScrollSnapshot>>,
    /// The report signal the heading pin watches (O-04); `None` in a bare reader.
    signal: Option<Rc<Cell<ScrollSignal>>>,
    width: f32,
    /// The reading column's maximum width; `None` fills the pane (P-01's
    /// `width` preference).
    max_width: Option<f32>,
    scale: f32,
    background: Color,
    scroll_margin: f32,
}

impl<'a> ReaderView<'a> {
    /// A reader for `doc`, styled by `theme`, targeting `doc`'s stable keys.
    pub fn new(
        doc: &'a MdDoc,
        theme: &'a DocTheme,
        style: &'a MarkdownStyle,
        scroll: Rc<Cell<ScrollSnapshot>>,
    ) -> Self {
        Self {
            doc,
            theme,
            style,
            math: &lmdv_render::NoMath,
            images: &lmdv_render::NoImages,
            wires: RenderWires::default(),
            details: None,
            highlights: Vec::new(),
            scroll,
            signal: None,
            width: READER_MAX_WIDTH,
            max_width: Some(READER_MAX_WIDTH),
            scale: 1.0,
            background: Color::TRANSPARENT,
            scroll_margin: 0.0,
        }
    }

    /// The page colour behind the article, so a narrow column does not show
    /// the split view's panel tint.
    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// The viewport width the reader was laid out in, for R-02's padding steps.
    pub fn width(mut self, width: f32) -> Self {
        if width.is_finite() {
            self.width = width.max(1.0);
        }
        self
    }

    /// The reading column's maximum width in points; `None` lets it fill the
    /// viewport (P-01's Narrow/Normal/Wide/Full).
    pub fn max_width(mut self, width: Option<f32>) -> Self {
        self.max_width = width.filter(|width| width.is_finite() && *width > 0.0);
        self
    }

    /// The formula resolver (`lmdv-math` through the window cache).
    pub fn math(mut self, math: &'a dyn MathResolver) -> Self {
        self.math = math;
        self
    }

    /// The image resolver (`rune::ImageCache` through the window cache).
    pub fn images(mut self, images: &'a dyn ImageResolver) -> Self {
        self.images = images;
        self
    }

    /// The app handlers for links and task checkboxes.
    pub fn wires(mut self, wires: RenderWires) -> Self {
        self.wires = wires;
        self
    }

    /// The open `<details>`/front-matter cards (M-22).
    pub fn details(mut self, details: Option<&'a dyn lmdv_render::DetailsState>) -> Self {
        self.details = details;
        self
    }

    /// Paint-only source ranges (find matches, the current match, `--line`).
    ///
    /// Empty by default. The renderer paints them without reshaping the text,
    /// and keys them `hl-{index}` so `Tx::scroll_into_view` can reveal the
    /// current match (OI-081/OI-084).
    pub fn highlights(mut self, highlights: impl IntoIterator<Item = Highlight>) -> Self {
        self.highlights = highlights.into_iter().collect();
        self
    }

    /// The space `Tx::scroll_into_view` leaves above a revealed match.
    ///
    /// The viewer find bar floats over the top of the article; while it is
    /// open the viewport uses its height as a scroll margin so a revealed
    /// match stops below the bar instead of under it (R-19).
    pub fn scroll_margin(mut self, points: f32) -> Self {
        if points.is_finite() {
            self.scroll_margin = points.max(0.0);
        }
        self
    }

    /// The report signal the heading pin watches (O-04); `None` leaves it unwired.
    pub fn scroll_signal(mut self, cell: Rc<Cell<ScrollSignal>>) -> Self {
        self.signal = Some(cell);
        self
    }

    /// The device scale, for `srcset` and fixed-size pictures (M-26).
    pub fn scale(mut self, scale: f32) -> Self {
        if scale.is_finite() {
            self.scale = scale.max(0.01);
        }
        self
    }

    /// The reading column's side and top padding at `width` (R-02).
    fn padding(&self) -> (f32, f32) {
        column_padding(self.width)
    }

    /// Builds the scrollable article.
    pub fn build(self) -> El {
        let (pad_x, pad_top) = self.padding();
        // R-02/OI-138: tables, code and pictures fit the reading column, not the
        // window. A 760 pt column inside a wide window leaves a 664 pt text
        // width, so a table wider than the column scrolls inside it instead of
        // overflowing the article.
        let column = self.max_width.map_or(self.width, |max| self.width.min(max));
        let content_width = (column - 2.0 * pad_x).max(1.0);
        let opts = RenderOptions {
            width: content_width,
            style: self.style,
            math: self.math,
            images: self.images,
            wires: self.wires,
            details: self.details,
            selectable: true,
            sticky_tables: true,
            sticky_inset: 0.0,
            scale: self.scale,
            highlights: self.highlights,
        };
        let article = render(self.doc, self.theme, &opts);
        let mut column = col().key(ARTICLE_KEY).w_full().px(pad_x).pt(pad_top).pb(READER_PAD_BOTTOM).child(article);
        if let Some(max_width) = self.max_width {
            column = column.max_w(max_width);
        }
        let scroll = Rc::clone(&self.scroll);
        let signal = self.signal.clone();

        col()
            .key(ARTICLE_SCROLL_KEY)
            .w_full()
            .h_full()
            .min_h(0.0)
            .fill(self.background)
            .scroll(ScrollSpec { stick_bottom: false })
            .scroll_edge_fade(12.0)
            .scroll_margin(self.scroll_margin)
            // R-07: Space pages the article even when a clicked block owns the
            // focus, which Rune's focus machine would otherwise press.
            .on_key(|state: &mut WindowState, event: &KeyEvent, tx: &mut Tx| state.article_space(event, tx))
            // A notched mouse wheel glides like the reading keys; a trackpad's
            // precise deltas, modified wheels and horizontal ones stay Rune's.
            .on_wheel(|state: &mut WindowState, event: rune::WheelEvent, tx: &mut Tx| state.article_wheel(&event, tx))
            // F-21: Ctrl+wheel or a pinch over the document changes the reader
            // size, never the panels'.
            .on_magnify(|state: &mut WindowState, event, _: &mut Tx| state.reader_magnify(event.delta))
            .on_scroll_offset(move |at| {
                scroll.set(ScrollSnapshot::from_pos(at));
                if let Some(signal) = &signal {
                    // Rune's own verdict on what moved the viewport: a wheel,
                    // a drag or an instant request, but not a glide (O-04).
                    let (by_hand, _, _) = rune::scroll_watch_state();
                    let seen = signal.get();
                    signal.set(ScrollSignal { report: seen.report.wrapping_add(1), by_hand });
                }
            })
            .child(col().key("article-row").w_full().center_y().child(column))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const FIXTURE: &str = r#"# A quiet page

Opening paragraph.

## What this page shows

- one
- two

### A nested heading

> A quote.

```rust
fn main() {}
```

#### A fourth-level heading

| Surface | State |
| --- | --- |
| Reader | P2 |

Setext heading
--------------

Closing paragraph.
"#;

    #[test]
    fn the_outline_lists_levels_one_to_four() {
        let doc = lmdv_md::parse(FIXTURE);
        let headings: Vec<_> = toc_headings(&doc).map(|(_, heading)| (heading.level, heading.text.as_str())).collect();
        assert_eq!(
            headings,
            vec![
                (1, "A quiet page"),
                (2, "What this page shows"),
                (3, "A nested heading"),
                (4, "A fourth-level heading"),
                (2, "Setext heading"),
            ]
        );
        assert!(has_toc_headings(&doc));

        let deep = lmdv_md::parse("##### Five\n\n###### Six\n");
        assert!(!has_toc_headings(&deep), "a document of only h5/h6 has no outline");
    }

    #[test]
    fn footnote_definition_headings_are_not_in_the_outline() {
        let doc = lmdv_md::parse("A note.[^1]\n\n[^1]: # Inside the note\n    Body\n");
        assert!(doc.headings.iter().any(|heading| heading.block == usize::MAX), "the parser registers one");
        assert!(
            toc_headings(&doc).all(|(_, heading)| heading.block != usize::MAX),
            "the outline skips the usize::MAX guard entry"
        );
    }

    #[test]
    fn a_line_lands_on_its_top_level_block() {
        let doc = lmdv_md::parse(FIXTURE);
        assert_eq!(block_key_at_line(&doc, 1).as_deref(), Some("b0"));
        assert_eq!(block_key_at_line(&doc, 5).as_deref(), Some("b2"), "the h2 block");
        assert_eq!(block_key_at_line(&doc, 8).as_deref(), Some("b3"), "the list");
        assert_eq!(block_key_at_line(&doc, usize::MAX), block_key_at_line(&doc, doc.line_count()));
    }

    #[test]
    fn line_ranges_exclude_the_newline() {
        let doc = lmdv_md::parse(FIXTURE);
        let range = line_range(&doc, 1).expect("line one");
        assert_eq!(&doc.source[range], "# A quiet page");
        assert_eq!(line_range(&doc, 0), line_range(&doc, 1), "line numbers clamp to one");
        assert!(line_range(&doc, doc.line_count() + 1).is_none());
    }

    #[test]
    fn the_anchor_is_the_nearest_heading_above_the_viewport() {
        let doc = lmdv_md::parse("# Top\n\nText\n\n## Second\n\nMore\n");
        let mut tops = HashMap::new();
        tops.insert("h0".to_owned(), [0.0, 40.0, 700.0, 30.0]);
        tops.insert("h1".to_owned(), [0.0, 420.0, 700.0, 22.0]);
        let layout = |key: &str| tops.get(key).copied();

        let anchor = anchor_from_layouts(&doc, 500.0, false, layout);
        assert_eq!(anchor.heading, Some(1));
        assert!((anchor.offset - 80.0).abs() < 0.01, "the heading sits 80 pt above the top");

        let anchor = anchor_from_layouts(&doc, 200.0, false, layout);
        assert_eq!(anchor.heading, Some(0));
        assert!((anchor.offset - 160.0).abs() < 0.01);

        let top = anchor_from_layouts(&doc, 500.0, true, layout);
        assert!(top.top);
        assert_eq!(top.heading, None);
    }

    #[test]
    fn an_empty_or_headless_document_has_no_anchor_heading() {
        let doc = lmdv_md::parse("");
        assert_eq!(anchor_from_layouts(&doc, 0.0, false, |_| None), ViewportAnchor::default());
        let doc = lmdv_md::parse("Just a paragraph.\n");
        assert_eq!(anchor_from_layouts(&doc, 100.0, false, |_| Some([0.0, 10.0, 700.0, 20.0])).heading, None);
    }

    #[test]
    fn the_activation_line_slides_through_the_last_viewport() {
        let top = ScrollSnapshot { offset: 0.0, max: 1000.0, viewport: 700.0 };
        assert_eq!(activation_line(700.0, top), HEADING_ACTIVATION, "a fixed line far from the end");
        let one_viewport_left = ScrollSnapshot { offset: 300.0, max: 1000.0, viewport: 700.0 };
        assert_eq!(activation_line(700.0, one_viewport_left), HEADING_ACTIVATION);
        let half = ScrollSnapshot { offset: 650.0, max: 1000.0, viewport: 700.0 };
        let line = activation_line(700.0, half);
        assert!(line > HEADING_ACTIVATION && line < 700.0, "half way down the last viewport: {line}");
        let end = ScrollSnapshot { offset: 1000.0, max: 1000.0, viewport: 700.0 };
        assert_eq!(activation_line(700.0, end), 700.0, "at the end the line is the viewport's bottom");
        assert_eq!(activation_line(0.0, end), HEADING_ACTIVATION, "no viewport yet");
        assert_eq!(activation_line(700.0, ScrollSnapshot::default()), HEADING_ACTIVATION, "a short document");
    }

    #[test]
    fn the_reading_block_is_the_first_one_not_scrolled_past() {
        let bottoms = [20.0f32, 60.0, 140.0, 420.0];
        let probe = |index: usize| bottoms.get(index).copied();
        assert_eq!(block_at_y(bottoms.len(), 0.0, probe), Some(0));
        assert_eq!(block_at_y(bottoms.len(), 20.0, probe), Some(1), "the first block has gone");
        assert_eq!(block_at_y(bottoms.len(), 30.0, probe), Some(1), "a margin gap still names the next block");
        assert_eq!(block_at_y(bottoms.len(), 60.0, probe), Some(2));
        assert_eq!(block_at_y(bottoms.len(), 399.0, probe), Some(3));
        assert_eq!(block_at_y(bottoms.len(), 3999.0, probe), Some(3), "past the last clamps to it");
        assert_eq!(block_at_y(0, 10.0, probe), None, "an empty document");
        assert_eq!(block_at_y(2, 10.0, |_| None), Some(1), "unmeasurable blocks fall back to the last");
    }
}
