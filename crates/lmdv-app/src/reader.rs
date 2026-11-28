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
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use lmdv_md::{Block, HtmlChild, HtmlNode, HtmlTag, Inline, InlineObjectKind, MdDoc};
use lmdv_render::{DocTheme, Highlight, ImageResolver, MathResolver, RenderOptions, RenderWires, heading_key, render};
use rune::{Color, Cursor, Cx, Drag, El, KeyEvent, Phase, PointerButton, ScrollSpec, Tx, col};
use rune_kit::markdown::MarkdownStyle;

use crate::window::WindowState;

/// The article's scroll viewport key. Scroll requests and `--line` target it.
pub const ARTICLE_SCROLL_KEY: &str = "article-scroll";
/// The reading column's key.
pub const ARTICLE_KEY: &str = "article";
/// Pointer travel, in points, before a press becomes a drag-to-scroll (D-06).
pub const PAN_THRESHOLD: f32 = 3.0;
/// The edge band, in points, that starts the drag-to-scroll autoscroll (D-06).
pub const AUTOSCROLL_BAND: f32 = 28.0;
/// The fastest autoscroll, in points a second, at the very edge (D-06).
pub const AUTOSCROLL_MAX_SPEED: f32 = 1400.0;
/// Reading column width in points, padding included (R-02).
pub const READER_MAX_WIDTH: f32 = 760.0;
/// Reading column side padding in points (R-02).
pub const READER_PAD_X: f32 = 48.0;
/// Reading column top padding in points (R-02).
pub const READER_PAD_TOP: f32 = 32.0;
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

/// R-11: the keys overlay holds the reading surface's keyboard.
///
/// The overlay is a Rune modal: focus and Tab stay inside it, its own buttons
/// take Enter and Space, and `app::on_key` has already handled the keys that
/// close it (`Esc`, `Ctrl+W`, `F1`, `?`). Everything else belongs to the
/// dialog, so the reading surface answers `true` and no window command (open,
/// new tab, quit, print) runs behind the scrim. Tab stays Rune's so a frame
/// without focus inside the dialog can still traverse into it.
pub(crate) fn help_overlay_holds_keys(state: &WindowState, event: &KeyEvent) -> bool {
    if state.help_view().is_none() {
        return false;
    }
    if event.key == rune::Key::Tab {
        return false;
    }
    // A plain scroll/activation key belongs to the dialog's scroll view and
    // focus, not to the reading surface behind it. Modified chords (Ctrl+T,
    // Ctrl+Q, Ctrl+F, …) still stop here so no window command runs behind the
    // scrim.
    let plain = !event.mods.cmd && !event.mods.ctrl && !event.mods.alt && !event.mods.logo;
    let dialog_key = matches!(
        event.key,
        rune::Key::PageDown
            | rune::Key::PageUp
            | rune::Key::Home
            | rune::Key::End
            | rune::Key::Up
            | rune::Key::Down
            | rune::Key::Left
            | rune::Key::Right
            | rune::Key::Enter
    ) || matches!(&event.key, rune::Key::Text(text) if text == " ");
    !(plain && dialog_key)
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

/// Whether the article draws a picture or a diagram that a later frame must
/// build again for.
///
/// A picture decodes on a worker: the frame that first resolves it draws its
/// declared box, and the frame after it is ready draws the picture's real
/// aspect ratio (or its placeholder, when the decode failed). A diagram is
/// drawn from the appearance's own Mermaid style. Both therefore need the
/// article built on the frame they change, which rules out reusing it
/// ([`ReaderView::memo`]).
///
/// Formulas are drawn in the building frame (`MathResolver`), so a document
/// may hold them and still keep its article.
pub fn draws_live_media(doc: &MdDoc) -> bool {
    doc.blocks.iter().any(block_draws_live_media)
        || doc.footnotes.iter().any(|footnote| footnote.blocks.iter().any(block_draws_live_media))
}

/// [`draws_live_media`] for one block, nested blocks included.
fn block_draws_live_media(block: &Block) -> bool {
    match block {
        Block::Image(_) | Block::Mermaid { .. } => true,
        Block::Para(inline) | Block::Heading { inline, .. } => inline_draws_live_media(inline),
        Block::List { items, .. } => items.iter().flatten().any(block_draws_live_media),
        Block::Quote { blocks, .. } => blocks.iter().any(block_draws_live_media),
        Block::Table(table) => {
            table.head.iter().any(inline_draws_live_media) || table.rows.iter().flatten().any(inline_draws_live_media)
        }
        Block::HtmlBlock(node) => html_draws_live_media(node),
        Block::Code { .. } | Block::Rule | Block::MathBlock { .. } | Block::FootnoteDefs | Block::Toc => false,
    }
}

/// [`draws_live_media`] for one inline run's objects.
///
/// A `<picture>` resolves through the same image resolver as an `Image` (its
/// best `<source>`/`<img>` candidate decodes on a worker), so it keeps the
/// article from being reused too. `<video>` and `<audio>` draw only a text
/// link, so they do not.
fn inline_draws_live_media(inline: &Inline) -> bool {
    inline.objects.iter().any(|object| matches!(object.kind, InlineObjectKind::Image | InlineObjectKind::Picture))
}

/// [`draws_live_media`] for an allowed-HTML subtree.
fn html_draws_live_media(node: &HtmlNode) -> bool {
    if matches!(node.tag, HtmlTag::Img | HtmlTag::Picture | HtmlTag::Video | HtmlTag::Audio) {
        return true;
    }
    node.children.iter().any(|child| match child {
        HtmlChild::Node(child) => html_draws_live_media(child),
        HtmlChild::Text(_) => false,
    })
}

/// The source hash an article's memo key needs when a parse reuses revision 0.
///
/// [`render`]'s per-block keys hash the source for the same reason: a document
/// parsed by [`lmdv_md::parse`] is revision 0 whatever its text. A document
/// with a real revision needs none, so a large source stays unhashed on every
/// frame the article is reused.
fn revision_source_hash(doc: &MdDoc) -> u64 {
    if doc.revision != 0 {
        return 0;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    doc.source.as_ref().hash(&mut hasher);
    hasher.finish()
}

/// A version of the paint-only highlights, for the article's memo key.
fn highlights_hash(highlights: &[Highlight]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    highlights.hash(&mut hasher);
    hasher.finish()
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

/// The gesture a drag-to-scroll press is in (D-06/R-28).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PanPhase {
    /// No press of ours.
    #[default]
    Idle,
    /// The press may become a pan; nothing has moved yet, so a click stays a
    /// click.
    Armed,
    /// Past the 3-point threshold: the article follows the pointer.
    Dragging,
}

/// Middle-drag anywhere and left-drag on the article's empty margin, with the
/// 3-point threshold, a grab cursor and edge autoscroll (D-06/R-28).
///
/// The scroll viewport's and the article column's `on_drag` feed this. Rune's
/// own middle-click autoscroll never starts because both opt in to the middle
/// button. A primary press on the rendered text is excluded by its position, so
/// text selection keeps the drag; a primary press on the column's side padding
/// or the margin around it pans, as the original's `#content-pane` does.
#[derive(Debug, Default)]
pub(crate) struct ScrollPan {
    phase: PanPhase,
    /// The viewport's offset when the press went down.
    start_offset: f32,
    /// The pan's current target, in points.
    offset: f32,
    /// The pointer's last window point.
    at: [f32; 2],
    /// When the autoscroll last stepped.
    tick: f64,
    /// The scroll viewport's unscrolled rect, from the last frame.
    viewport: Cell<Option<[f32; 4]>>,
    /// The reading column's unscrolled rect and side padding, from the last
    /// frame: the empty margin is outside its text band.
    column: Cell<Option<[f32; 4]>>,
    /// The rendered document's unscrolled rect: its top and bottom padding are
    /// empty margin too.
    doc: Cell<Option<[f32; 4]>>,
    pad_x: Cell<f32>,
}

impl ScrollPan {
    /// Remembers the last frame's geometry: the viewport, the column and the
    /// column's side padding.
    pub(crate) fn record_geometry(
        &self,
        viewport: Option<[f32; 4]>,
        column: Option<[f32; 4]>,
        doc: Option<[f32; 4]>,
        pad_x: f32,
    ) {
        self.viewport.set(viewport);
        self.column.set(column);
        self.doc.set(doc);
        self.pad_x.set(pad_x.max(0.0));
    }

    /// The cursor the article shows: an open hand on an armed press, a closed
    /// one while the page follows the pointer.
    pub(crate) fn cursor(&self) -> Cursor {
        match self.phase {
            PanPhase::Idle => Cursor::Default,
            PanPhase::Armed => Cursor::Grab,
            PanPhase::Dragging => Cursor::Grabbing,
        }
    }

    /// Whether a press of `button` at `point` may start a pan.
    ///
    /// The middle button may anywhere. The primary one only on the empty
    /// margin: the column's side padding, or the space beside the column. The
    /// text band itself keeps selection.
    fn may_start(&self, button: PointerButton, point: [f32; 2], offset: f32) -> bool {
        match button {
            PointerButton::Middle => true,
            PointerButton::Primary => {
                let Some(column) = self.column.get() else { return false };
                let pad = self.pad_x.get();
                let inner_left = column[0] + pad;
                let inner_right = column[0] + column[2] - pad;
                if point[0] < inner_left || point[0] > inner_right {
                    return true;
                }
                // Above the first block or below the last one the column's top
                // and bottom padding is margin as well.
                self.doc.get().is_some_and(|doc| {
                    let top = doc[1] - offset;
                    point[1] < top || point[1] > top + doc[3]
                })
            }
            _ => false,
        }
    }

    /// The Down half: arms a pan when the press may start one. Returns whether
    /// the state changed, so the frame redraws the grab cursor.
    fn press(&mut self, button: PointerButton, point: [f32; 2], offset: f32, now: f64) -> bool {
        self.clear();
        if !self.may_start(button, point, offset) {
            return false;
        }
        self.phase = PanPhase::Armed;
        self.start_offset = offset.max(0.0);
        self.offset = self.start_offset;
        self.at = point;
        self.tick = now;
        true
    }

    /// The Move half: crosses the threshold and returns the offset the article
    /// should follow to, or `None` while the gesture is a still click.
    fn moved(&mut self, dx: f32, dy: f32, window: [f32; 2], scroll: ScrollSnapshot, now: f64) -> Option<f32> {
        if self.phase == PanPhase::Idle {
            return None;
        }
        self.at = window;
        if self.phase == PanPhase::Armed {
            if dx.hypot(dy) < PAN_THRESHOLD {
                return None;
            }
            self.phase = PanPhase::Dragging;
        }
        self.tick = now;
        self.offset = (self.start_offset - dy).clamp(0.0, scroll.max.max(0.0));
        Some(self.offset)
    }

    /// One autoscroll step while the pointer rests in an edge band; the target
    /// and the time the next step is due.
    fn autoscroll(&mut self, scroll: ScrollSnapshot, now: f64) -> Option<(f32, f64)> {
        if self.phase != PanPhase::Dragging {
            return None;
        }
        let viewport = self.viewport.get()?;
        let (top, bottom) = (viewport[1], viewport[1] + viewport[3]);
        let y = self.at[1];
        let speed = if y < top + AUTOSCROLL_BAND {
            -AUTOSCROLL_MAX_SPEED * ((top + AUTOSCROLL_BAND - y) / AUTOSCROLL_BAND).clamp(0.0, 1.0)
        } else if y > bottom - AUTOSCROLL_BAND {
            AUTOSCROLL_MAX_SPEED * ((y - (bottom - AUTOSCROLL_BAND)) / AUTOSCROLL_BAND).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if speed == 0.0 {
            return None;
        }
        let dt = (now - self.tick).clamp(0.0, 0.05);
        self.tick = now;
        self.offset = (self.offset + speed * dt as f32).clamp(0.0, scroll.max.max(0.0));
        Some((self.offset, now + 1.0 / 60.0))
    }

    /// Ends the gesture; true when one was in hand.
    fn release(&mut self) -> bool {
        let active = self.phase != PanPhase::Idle;
        self.clear();
        active
    }

    fn clear(&mut self) {
        self.phase = PanPhase::Idle;
    }
}

impl WindowState {
    /// D-06/R-28: the article's press, move and release. The viewport's
    /// `on_drag` calls this; the middle button never reaches Rune's own
    /// autoscroll because the viewport opts in to it.
    pub(crate) fn article_pan(&mut self, drag: &Drag, tx: &mut Tx) -> bool {
        let scroll = self.reading.scroll.get();
        let now = tx.clock();
        match drag.phase {
            Phase::Down => self.reader_pan.press(tx.button(), drag.window_point(), scroll.offset, now),
            Phase::Move => {
                let started = self.reader_pan.phase != PanPhase::Dragging;
                match self.reader_pan.moved(drag.dx, drag.dy, drag.window_point(), scroll, now) {
                    Some(target) => {
                        if started {
                            self.reading_keys.release_pin();
                            self.cancel_landing(now, tx);
                        } else {
                            // The page follows the hand; only the start of the
                            // drag changes what the view draws (the cursor).
                            tx.live_only();
                        }
                        tx.scroll_to_y(ARTICLE_SCROLL_KEY, target);
                        started
                    }
                    None => false,
                }
            }
            Phase::Up => self.reader_pan.release(),
        }
    }

    /// D-06: keeps the article scrolling while a pan holds the pointer in an
    /// edge band, one frame at a time.
    pub(crate) fn poll_scroll_pan(&mut self, now: f64, tx: &mut Tx) -> bool {
        let scroll = self.reading.scroll.get();
        let Some((target, wake)) = self.reader_pan.autoscroll(scroll, now) else { return false };
        tx.scroll_to_y(ARTICLE_SCROLL_KEY, target);
        tx.wake_at(wake);
        true
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
    /// E-06: the app's rendered-diagram cache, when the preview should reuse
    /// unchanged diagrams instead of rebuilding them.
    mermaid: Option<&'a dyn lmdv_render::MermaidCache>,
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
    /// D-06: the pointer shape the article shows (an armed or held pan).
    cursor: Cursor,
    /// The app state the article's handlers capture, when it may be reused
    /// ([`ReaderView::memo`]).
    memo: Option<u64>,
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
            mermaid: None,
            highlights: Vec::new(),
            scroll,
            signal: None,
            width: READER_MAX_WIDTH,
            max_width: Some(READER_MAX_WIDTH),
            scale: 1.0,
            background: Color::TRANSPARENT,
            scroll_margin: 0.0,
            cursor: Cursor::Default,
            memo: None,
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

    /// The rendered-diagram cache (E-06): an unchanged diagram is reused from
    /// `cache` instead of being laid out again on every preview refresh.
    pub fn mermaid(mut self, cache: &'a dyn lmdv_render::MermaidCache) -> Self {
        self.mermaid = Some(cache);
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

    /// D-06: the pointer shape the article shows while a pan is armed or held.
    pub fn article_cursor(mut self, cursor: Cursor) -> Self {
        self.cursor = cursor;
        self
    }

    /// The report signal the heading pin watches (O-04); `None` leaves it unwired.
    pub fn scroll_signal(mut self, cell: Rc<Cell<ScrollSignal>>) -> Self {
        self.signal = Some(cell);
        self
    }

    /// Reuses the article across frames that would draw the same thing.
    ///
    /// The window's view runs whenever a scroll moves the outline's spy or the
    /// reading anchor, so every frame of a wheel burst would otherwise build
    /// the whole article again: 14 ms of a 50 ms frame on a 950 KiB document.
    /// Rune's `memo` hands back the subtree it built for the same key instead,
    /// keeping its node identity, so selection, hover and the viewport's
    /// scroll offset carry on untouched.
    ///
    /// The key is the document revision (with the source hashed while a parse
    /// reuses revision 0), the theme's fingerprint, the column, the scale, the
    /// highlights and `state`. `state` must cover every app value a handler in
    /// the article captured or read while building: the tab, the mode, the
    /// open `<details>` cards and the table bar's copy flash.
    ///
    /// A document that draws a picture or a diagram is never kept
    /// ([`draws_live_media`]): its article has to be built again when the
    /// picture decodes or the appearance's diagram style changes.
    pub fn memo(mut self, state: u64) -> Self {
        self.memo = Some(state);
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
        // A document that draws a picture or a diagram is never kept (see `draws_live_media`).
        let memo = self.memo.filter(|_| !draws_live_media(self.doc));
        let highlights_version = memo.is_some().then(|| highlights_hash(&self.highlights));
        let opts = RenderOptions {
            width: content_width,
            style: self.style,
            math: self.math,
            images: self.images,
            wires: self.wires,
            mermaid: self.mermaid,
            details: self.details,
            selectable: true,
            sticky_tables: true,
            sticky_inset: 0.0,
            scale: self.scale,
            highlights: self.highlights,
        };
        let article = match memo {
            Some(state) => {
                let version = (
                    self.doc.revision,
                    revision_source_hash(self.doc),
                    self.theme.fingerprint(),
                    content_width.to_bits(),
                    self.scale.to_bits(),
                    highlights_version.unwrap_or(0),
                    state,
                );
                // The id is the renderer's own root key, so the reused
                // subtree keeps it: Rune renames a memoised root to the memo's
                // id, and one window draws one article.
                rune::memo(lmdv_render::DOC_KEY, version, || render(self.doc, self.theme, &opts))
            }
            _ => render(self.doc, self.theme, &opts),
        };
        let mut column = col().key(ARTICLE_KEY).w_full().px(pad_x).pt(pad_top).pb(READER_PAD_BOTTOM).child(article);
        if let Some(max_width) = self.max_width {
            column = column.max_w(max_width);
        }
        // D-06/R-28: the column's own side padding is empty margin, but its
        // focusable selection group stops a press before the viewport's drag
        // handler, so the column takes the same gesture. The text band itself
        // still refuses to pan, so selection is untouched.
        column =
            column.pointer_buttons([PointerButton::Middle]).drag_threshold(PAN_THRESHOLD).cursor(self.cursor).on_drag(
                |state: &mut WindowState, drag: &mut Drag, tx: &mut Tx| {
                    state.article_pan(drag, tx);
                },
            );
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
            // D-06/R-28: the middle button pans anywhere and never starts Rune's
            // own middle-click autoscroll; the primary button pans on the empty
            // margin only (the text keeps selection). A press becomes a pan
            // after 3 points, so a click stays a click.
            .pointer_buttons([PointerButton::Middle])
            .drag_threshold(PAN_THRESHOLD)
            .cursor(self.cursor)
            .on_drag(|state: &mut WindowState, drag: &mut Drag, tx: &mut Tx| {
                state.article_pan(drag, tx);
            })
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

    use rune::{Cursor, GesturePhase, MagnifyEvent, PointerButton, Runtime, Tx};
    use tempfile::TempDir;

    use super::*;
    use crate::test_support;
    use crate::window::WindowState;

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
    fn the_reading_column_pads_48_32_at_1280_and_20_at_480() {
        // R-02: 48/32/96; at pane width ≤640 the sides fall to 20, and at
        // ≤420 to 14 with a 24 top.
        assert_eq!((READER_PAD_X, READER_PAD_TOP, READER_PAD_BOTTOM), (48.0, 32.0, 96.0));
        assert_eq!(column_padding(1280.0), (48.0, 32.0));
        assert_eq!(column_padding(760.0), (48.0, 32.0), "the column itself keeps the wide steps");
        assert_eq!(column_padding(641.0), (48.0, 32.0));
        assert_eq!(column_padding(640.0), (20.0, 32.0), "≤640: sides 20, top still 32");
        assert_eq!(column_padding(480.0), (20.0, 32.0));
        assert_eq!(column_padding(421.0), (20.0, 32.0));
        assert_eq!(column_padding(420.0), (14.0, 24.0), "≤420: sides 14 and top 24");
        assert_eq!(column_padding(320.0), (14.0, 24.0));
    }

    #[test]
    fn the_rendered_reading_column_measures_48_32_at_1280_and_20_at_480() {
        // R-02's acceptance: the drawn article, not just the constants. The
        // scene keeps both panes closed, so the pane is the window's content.
        fn measure(width: f32) -> (f32, f32, f32, f32) {
            let temp = tempfile::TempDir::new().unwrap();
            let mut state = crate::window::WindowState::new(crate::test_support::shared(&temp));
            crate::view::scene_reader(&mut state);
            let mut runtime = rune::Runtime::new(state, crate::view::view, (width, 900.0), rune::Env::new(false, 1.0));
            runtime.frame(0.0);
            let elements = runtime.elements();
            let by = |key: &str| elements.iter().find(|element| element.key.as_deref() == Some(key)).expect(key);
            let (article, scroll, heading) = (by("article"), by("article-scroll"), by("h0"));
            (heading.rect[0] - article.rect[0], heading.rect[1] - scroll.rect[1], article.rect[2], heading.rect[2])
        }

        let (side, top, column, content) = measure(1280.0);
        assert_eq!((side, top), (READER_PAD_X, READER_PAD_TOP), "1280: 48 side and 32 top");
        assert_eq!(column, 760.0, "the default Normal column is capped at 760");
        assert_eq!(content, 664.0, "760 less both side paddings");

        let (side, top, column, content) = measure(480.0);
        assert_eq!(side, 20.0, "≤640: the sides fall to 20");
        assert_eq!(top, READER_PAD_TOP, "the top stays 32 above 420");
        assert!(column <= 640.0, "the pane itself is narrow: {column}");
        assert_eq!(content, column - 2.0 * side, "the content follows the narrow padding");
    }

    /// F-21: the article's `on_magnify` wire (at the end of [`ReaderView::build`])
    /// really steps the reader size from a native, phased trackpad pinch, not
    /// just from `reader_magnify` called directly. The side panes stay put.
    #[test]
    fn a_native_pinch_over_the_article_steps_only_the_reader_size() {
        let temp = TempDir::new().unwrap();
        let mut state = WindowState::new(test_support::shared(&temp));
        crate::view::scene_reader(&mut state);
        let size = state.shared().prefs().reader.size;
        let side = state.shared().prefs().reader.side_size;
        let mut runtime = Runtime::new(state, crate::view::view, (1280.0, 900.0), rune::Env::new(false, 1.0));
        runtime.frame(0.0);

        // A phased pinch: Began locks the article's handler; Moved deltas
        // accumulate to one step (0.2 + 0.2 > 0.35).
        runtime.magnify_event(MagnifyEvent::new(0.2).at(640.0, 450.0).phase(GesturePhase::Began));
        runtime.magnify_event(MagnifyEvent::new(0.2).at(640.0, 450.0).phase(GesturePhase::Moved));
        runtime.magnify_event(MagnifyEvent::new(0.0).at(640.0, 450.0).phase(GesturePhase::Ended));
        runtime.frame(1.0 / 60.0);

        assert_ne!(runtime.state.shared().prefs().reader.size, size, "the native pinch grew the reader one step");
        assert_eq!(runtime.state.shared().prefs().reader.side_size, side, "the panels were untouched");
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
    fn only_pictures_and_diagrams_keep_the_article_from_being_reused() {
        assert!(!draws_live_media(&lmdv_md::parse(FIXTURE)), "text, lists, quotes, code and tables are still");
        assert!(
            !draws_live_media(&lmdv_md::parse("$$x^2$$\n\nSome math.\n")),
            "a formula is drawn in the building frame"
        );

        assert!(draws_live_media(&lmdv_md::parse("![alt](shot.png)\n")), "a block picture");
        assert!(draws_live_media(&lmdv_md::parse("A paragraph with ![alt](shot.png) inline.\n")), "an inline picture");
        assert!(draws_live_media(&lmdv_md::parse("> ![alt](shot.png)\n")), "a picture in a quote");
        assert!(draws_live_media(&lmdv_md::parse("- ![alt](shot.png)\n")), "a picture in a list item");
        assert!(
            draws_live_media(&lmdv_md::parse("| a |\n| --- |\n| ![alt](shot.png) |\n")),
            "a picture in a table cell"
        );
        assert!(draws_live_media(&lmdv_md::parse("<img src=\"shot.png\" alt=\"alt\">\n")), "an HTML picture");
        let picture = r#"<picture><source srcset="shot.webp"><img src="shot.png" alt="alt"></picture>"#;
        assert!(
            draws_live_media(&lmdv_md::parse(&format!("A paragraph with {picture} inline.\n"))),
            "an inline <picture> decodes through the image resolver"
        );
        assert!(
            draws_live_media(&lmdv_md::parse(&format!("| a |\n| --- |\n| {picture} |\n"))),
            "an inline <picture> in a table cell"
        );
        assert!(draws_live_media(&lmdv_md::parse("```mermaid\ngraph TD\nA-->B\n```\n")), "a diagram");

        let footnoted = lmdv_md::parse("A note.[^1]\n\n[^1]: ![alt](shot.png)\n");
        assert!(draws_live_media(&footnoted), "a picture in a footnote definition");
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

    // ---- D-06/R-28: drag-to-scroll ------------------------------------------

    fn snapshot(offset: f32, max: f32, viewport: f32) -> ScrollSnapshot {
        ScrollSnapshot { offset, max, viewport }
    }

    #[test]
    fn the_middle_button_may_pan_anywhere_and_the_primary_only_on_the_margin() {
        let pan = ScrollPan::default();
        // A 760-point column at x 20 in an 800-point viewport, padded 48.
        pan.record_geometry(
            Some([0.0, 40.0, 800.0, 600.0]),
            Some([20.0, 40.0, 760.0, 2000.0]),
            Some([68.0, 100.0, 664.0, 1000.0]),
            48.0,
        );
        assert!(pan.may_start(PointerButton::Middle, [400.0, 300.0], 0.0), "the middle button pans over the text");
        assert!(!pan.may_start(PointerButton::Primary, [400.0, 300.0], 0.0), "the text keeps selection");
        assert!(pan.may_start(PointerButton::Primary, [30.0, 300.0], 0.0), "the column's side padding is empty margin");
        assert!(pan.may_start(PointerButton::Primary, [750.0, 300.0], 0.0), "so is the space beside the column");
        assert!(!pan.may_start(PointerButton::Primary, [80.0, 300.0], 0.0), "just inside the text band does not pan");
        assert!(pan.may_start(PointerButton::Primary, [400.0, 60.0], 0.0), "above the first block is margin");
        assert!(pan.may_start(PointerButton::Primary, [400.0, 1200.0], 0.0), "below the last block is margin");
        assert!(!pan.may_start(PointerButton::Primary, [400.0, 300.0], 200.0), "a scrolled page's text still selects");
        assert!(!pan.may_start(PointerButton::Back, [30.0, 300.0], 0.0), "only the middle and primary buttons pan");
    }

    #[test]
    fn a_pan_starts_after_three_points_and_clamps() {
        let mut pan = ScrollPan::default();
        pan.record_geometry(
            Some([0.0, 40.0, 800.0, 600.0]),
            Some([20.0, 40.0, 760.0, 2000.0]),
            Some([68.0, 100.0, 664.0, 1000.0]),
            48.0,
        );
        let at = [30.0, 300.0];
        assert!(pan.press(PointerButton::Middle, at, 300.0, 1.0), "a press arms the pan");
        assert_eq!(pan.cursor(), Cursor::Grab, "the press shows an open hand");
        let scroll = snapshot(300.0, 1000.0, 600.0);
        assert_eq!(pan.moved(2.0, -2.0, at, scroll, 1.0), None, "2 points is still a click");
        assert_eq!(pan.moved(0.0, -80.0, [30.0, 220.0], scroll, 1.1), Some(380.0));
        assert_eq!(pan.cursor(), Cursor::Grabbing, "the held pan shows a closed hand");
        assert_eq!(pan.moved(0.0, -2000.0, [30.0, -1700.0], scroll, 1.2), Some(1000.0), "the offset clamps at the end");
        assert!(pan.release(), "the release ends the gesture");
        assert_eq!(pan.cursor(), Cursor::Default);
        assert!(!pan.release(), "a second release is nothing");
    }

    #[test]
    fn the_edge_bands_autoscroll_toward_the_pointer() {
        let mut pan = ScrollPan::default();
        pan.record_geometry(
            Some([0.0, 100.0, 800.0, 600.0]),
            Some([20.0, 100.0, 760.0, 2000.0]),
            Some([68.0, 160.0, 664.0, 1000.0]),
            48.0,
        );
        pan.press(PointerButton::Middle, [400.0, 400.0], 300.0, 1.0);
        pan.moved(0.0, -30.0, [400.0, 695.0], snapshot(300.0, 1000.0, 600.0), 1.0);
        let scroll = snapshot(300.0, 1000.0, 600.0);
        let (target, wake) = pan.autoscroll(scroll, 1.1).expect("the bottom band scrolls down");
        assert!(target > 300.0, "toward the end: {target}");
        assert!((wake - (1.1 + 1.0 / 60.0)).abs() < 1e-6);

        pan.moved(0.0, -30.0, [400.0, 300.0], scroll, 1.1);
        assert_eq!(pan.autoscroll(scroll, 1.2), None, "the middle of the page is still");

        pan.moved(0.0, -30.0, [400.0, 105.0], scroll, 1.2);
        let (up, _) = pan.autoscroll(snapshot(300.0, 1000.0, 600.0), 1.3).expect("the top band scrolls up");
        assert!(up < 300.0, "toward the start: {up}");
    }

    /// A reader runtime laid out twice, so the toolbar and article have sizes.
    fn reader_runtime(temp: &TempDir, width: f32) -> Runtime<WindowState> {
        let mut state = WindowState::new(test_support::shared(temp));
        crate::view::scene_reader(&mut state);
        let mut runtime = Runtime::new(state, crate::view::view, (width, 760.0), rune::Env::new(true, 1.0));
        runtime.frame(0.0);
        runtime.frame(0.0);
        runtime.frame(0.0);
        runtime
    }

    fn rect(runtime: &Runtime<WindowState>, key: &str) -> [f32; 4] {
        runtime
            .elements()
            .into_iter()
            .find(|element| element.key.as_deref() == Some(key))
            .map(|element| element.rect)
            .expect(key)
    }

    fn offset(runtime: &Runtime<WindowState>) -> f32 {
        runtime.state.reading.scroll.get().offset
    }

    #[test]
    fn a_middle_drag_over_the_text_pans_the_article() {
        let temp = TempDir::new().unwrap();
        let mut runtime = reader_runtime(&temp, 1100.0);
        let scroll = rect(&runtime, ARTICLE_SCROLL_KEY);
        let (x, y) = (scroll[0] + scroll[2] * 0.5, scroll[1] + scroll[3] * 0.5);
        runtime.pointer_move(x, y);
        runtime.frame(0.0);
        runtime.button_down(x, y, PointerButton::Middle);
        runtime.frame(0.0);
        assert_eq!(runtime.state.reader_pan.cursor(), Cursor::Grab);
        runtime.pointer_move(x, y - 140.0);
        runtime.frame(0.0);
        runtime.pointer_move(x, y - 160.0);
        runtime.frame(0.0);
        runtime.frame(0.0);
        runtime.button_up(x, y - 160.0, PointerButton::Middle);
        runtime.frame(0.0);
        assert!(offset(&runtime) > 80.0, "the middle drag moved the article: {}", offset(&runtime));
        assert_eq!(runtime.state.reader_pan.cursor(), Cursor::Default);
    }

    #[test]
    fn a_left_drag_pans_only_on_the_empty_margin() {
        let temp = TempDir::new().unwrap();
        let mut runtime = reader_runtime(&temp, 1100.0);
        let scroll = rect(&runtime, ARTICLE_SCROLL_KEY);
        let y = scroll[1] + scroll[3] * 0.5;
        // The column is centred at 170 points, 760 wide, padded 48: 30 is margin.
        let margin = scroll[0] + 30.0;
        runtime.pointer_move(margin, y);
        runtime.frame(0.0);
        runtime.pointer_down(margin, y);
        runtime.frame(0.0);
        runtime.pointer_move(margin, y - 90.0);
        runtime.frame(0.0);
        runtime.pointer_move(margin, y - 120.0);
        runtime.frame(0.0);
        runtime.frame(0.0);
        runtime.pointer_up(margin, y - 120.0);
        runtime.frame(0.0);
        assert!(offset(&runtime) > 60.0, "the margin drag moved the article: {}", offset(&runtime));

        // The column's own side padding (760 wide at x 170, padded 48) is
        // margin too, and its focusable group would otherwise swallow the press.
        let mut runtime = reader_runtime(&temp, 1100.0);
        let scroll = rect(&runtime, ARTICLE_SCROLL_KEY);
        let x = scroll[0] + 170.0;
        let y = scroll[1] + scroll[3] * 0.5;
        runtime.pointer_move(x, y);
        runtime.frame(0.0);
        runtime.pointer_down(x, y);
        runtime.frame(0.0);
        runtime.pointer_move(x, y - 90.0);
        runtime.frame(0.0);
        runtime.pointer_move(x, y - 120.0);
        runtime.frame(0.0);
        runtime.frame(0.0);
        runtime.pointer_up(x, y - 120.0);
        runtime.frame(0.0);
        assert!(offset(&runtime) > 60.0, "the column's padding pans: {}", offset(&runtime));

        // From the text, the same gesture is a selection, not a pan.
        let mut runtime = reader_runtime(&temp, 1100.0);
        let scroll = rect(&runtime, ARTICLE_SCROLL_KEY);
        let x = scroll[0] + scroll[2] * 0.5;
        let y = scroll[1] + scroll[3] * 0.5;
        runtime.pointer_move(x, y);
        runtime.frame(0.0);
        runtime.pointer_down(x, y);
        runtime.frame(0.0);
        runtime.pointer_move(x, y - 90.0);
        runtime.frame(0.0);
        runtime.pointer_up(x, y - 90.0);
        runtime.frame(0.0);
        assert_eq!(offset(&runtime), 0.0, "a text drag stays selection");
    }

    #[test]
    fn a_two_point_move_is_still_a_click() {
        let temp = TempDir::new().unwrap();
        let mut runtime = reader_runtime(&temp, 1100.0);
        let scroll = rect(&runtime, ARTICLE_SCROLL_KEY);
        let x = scroll[0] + 30.0;
        let y = scroll[1] + scroll[3] * 0.5;
        runtime.pointer_move(x, y);
        runtime.frame(0.0);
        runtime.pointer_down(x, y);
        runtime.frame(0.0);
        runtime.pointer_move(x, y - 2.0);
        runtime.frame(0.0);
        runtime.pointer_move(x, y - 2.5);
        runtime.frame(0.0);
        assert_eq!(offset(&runtime), 0.0, "under three points nothing moves");
        runtime.pointer_move(x, y - 40.0);
        runtime.frame(0.0);
        runtime.frame(0.0);
        assert!(offset(&runtime) > 20.0, "past the threshold the article follows: {}", offset(&runtime));
        runtime.pointer_up(x, y - 40.0);
        runtime.frame(0.0);
    }

    #[test]
    fn holding_the_pointer_in_an_edge_band_keeps_scrolling() {
        let temp = TempDir::new().unwrap();
        let mut runtime = reader_runtime(&temp, 1100.0);
        runtime.poll = Some(|state: &mut WindowState, now, tx: &mut Tx| state.poll(now, tx));
        let scroll = rect(&runtime, ARTICLE_SCROLL_KEY);
        let margin = scroll[0] + 30.0;
        let middle = scroll[1] + scroll[3] * 0.5;
        // First get the page away from the top.
        runtime.pointer_move(margin, middle);
        runtime.frame(0.0);
        runtime.pointer_down(margin, middle);
        runtime.frame(0.0);
        runtime.pointer_move(margin, middle - 300.0);
        runtime.frame(0.0);
        runtime.frame(0.0);
        runtime.pointer_up(margin, middle - 300.0);
        runtime.frame(0.0);
        let start = offset(&runtime);
        assert!(start > 200.0, "the first drag scrolled: {start}");

        // A held press near the bottom edge scrolls on by itself.
        runtime.pointer_move(margin, middle);
        runtime.frame(0.0);
        runtime.pointer_down(margin, middle);
        runtime.frame(0.0);
        runtime.pointer_move(margin, scroll[1] + scroll[3] - 6.0);
        runtime.frame(0.0);
        runtime.frame(0.0);
        let before = offset(&runtime);
        for _ in 0..12 {
            runtime.frame(0.05);
        }
        let after = offset(&runtime);
        assert!(after > before + 20.0, "the edge autoscroll continued: {before} -> {after}");
        runtime.pointer_up(margin, scroll[1] + scroll[3] - 6.0);
        runtime.frame(0.0);
    }
}
