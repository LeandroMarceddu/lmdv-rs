//! Rune renderer for the Rune-free [`lmdv_md::MdDoc`] model.
//!
//! [`render`] is a pure function of the document, the reading theme and the
//! caller-supplied resolvers: it touches no window, tab or app state. The app
//! wraps the result in its article viewport; exports can reuse the same tree or
//! read [`lmdv_md`] directly.
//!
//! What the tree carries:
//!
//! * every top-level block gets its `block_spans[i].key` (`b{index}`);
//! * headings also get `h{heading_index}` so an outline jump targets the
//!   heading itself, not the block wrapper (ARCHITECTURE_DETAIL §5.3);
//! * footnote entries are `fn-{label}`; inline references are links to
//!   `fn:{label}`, so the app scrolls the matching entry;
//! * in-document links are their destination as written (`#id`, `./file.md`);
//!   the app resolves fragments through [`lmdv_md::MdDoc::resolve_heading`];
//! * table/front-matter/callout/footnote/TOC parts carry stable sub-keys.
//!
//! Interaction stays in the app. Links become clickable only through
//! [`RenderOptions::link_wire`], a `Rc<dyn Fn(El) -> El>` the app builds once
//! from `El::on_link` for its own state type (Rune's handler is generic over
//! the app state, so a plain `Fn(&str)` cannot be attached here). Task
//! checkboxes take [`RenderOptions::task_control_wire`] on the real checkbox
//! control ([`RenderOptions::task_wire`] remains for row-level use), and the
//! `<details>` cards take [`RenderOptions::details`] plus the same link wire
//! with a `details:` key.
//! A [`RenderWires::block`] wire wraps every top-level block with the
//! [`BlockRef`] the app needs to turn a preview double-click or a right-click
//! into a source line (E-04/E-35): the block's key and source lines, plus the
//! inline text exactly as drawn, split per text element so a clicked word maps
//! back to its own source occurrence.
//!
//! Parity rows: M-01–M-34 and R-02/R-25/R-26 (reading surface and copy).
//!
//! # Highlights
//!
//! Find matches, the current match and the `--line` line are painted through
//! [`RenderOptions::highlights`]: each [`Highlight`] is a source byte range with
//! a [`HighlightKind`], mapped to the drawn text through the block's inline
//! [`lmdv_md::SourceMap`] and painted as a paint-only [`rune::TextHighlight`].
//! Nothing is reshaped, selected or copied differently, so a highlight can move
//! every frame without moving a line. A source range that crosses blocks is
//! split into one highlight per block; overlapping ranges paint the strongest
//! kind once ([`HighlightKind::Current`] over [`HighlightKind::Find`] over
//! [`HighlightKind::Line`]).
//!
//! # What stays outside the renderer
//!
//! * **Click-to-source and selection-to-source** — the mapping data is in
//!   `MdDoc`; the app wires it through [`RenderWires::block`] (P3-04c).
//! * **Table action bar and copy-as-table** — the renderer hands every table's
//!   key, source range and drawn cells to [`RenderWires::table`]; the app draws
//!   the hover/focus bar, the full-window preview and the TSV copy above this
//!   tree (M-06, X-16). The table itself, its sticky header and alignment are
//!   here.
//! * **Full-window media preview** — the renderer hands every drawn picture
//!   and supported diagram to [`RenderWires::media`] as a [`MediaRef`]; the app
//!   adds the open affordance, the modal stage and its pan/zoom (V-01–V-10).
//!   The media's own markup stays this renderer's.
//! * **Media playback and subtitle tracks** — Rune has no Linux media backend;
//!   video/audio are placeholder cards with `media:<src>` (M-27/M-28 gap).
//! * **Link policy and opening** — the tree only reports destinations; the app
//!   resolves relative paths, fragments and refused network paths (M-30/M-31).
//! * **`<details>` bodies after a blank line** — `lmdv-md` ends the HTML block at
//!   the blank line, so those blocks are ordinary document blocks outside the
//!   card; the HTML inside one block is a real card (M-22 partial).

#![forbid(unsafe_code)]

mod block;
mod html;
mod inline;
mod math;
mod parts;
mod table;
mod theme;

#[cfg(test)]
mod tests;

use std::rc::Rc;

use lmdv_md::MdDoc;
use rune::El;
use rune_kit::markdown::MarkdownStyle;

pub use math::{MathResolver, NoMath};
pub use rune::text::InlineImage;
pub use table::table_refs;
pub use theme::{CalloutPaint, DocLabels, DocTheme};

/// The article root's key. The app may wrap it in its own scroll viewport.
pub const DOC_KEY: &str = "doc";
/// The front-matter card's key.
pub const FRONT_MATTER_KEY: &str = "front-matter";
/// The generated footnotes section's key.
pub const FOOTNOTES_KEY: &str = "footnotes";
/// The generated table-of-contents navigation's key.
pub const TOC_KEY: &str = "toc";
/// How far a reveal keeps a match from a horizontal scroller's edges, in points.
///
/// The original's `revealSideways` keeps a 40 px margin before centring the
/// match in a wide table or code block (R-19); Rune reveals with the scroller's
/// own `scroll_margin`, so the renderer sets this on those viewports.
pub const HORIZONTAL_REVEAL_MARGIN: f32 = 40.0;

/// The key of the heading whose index is `index` in `MdDoc::headings`.
pub fn heading_key(index: usize) -> String {
    format!("h{index}")
}

/// The key of a footnote entry, from its label.
pub fn footnote_key(label: &str) -> String {
    format!("fn-{label}")
}

/// The link destination an inline footnote reference reports.
pub fn footnote_link(label: &str) -> String {
    format!("fn:{label}")
}

/// The link destination that toggles a `<details>`/front-matter card.
pub fn details_toggle(key: &str) -> String {
    format!("details:{key}")
}

/// The paint key of the highlight at `index` passed to
/// [`RenderOptions::highlights`].
///
/// A key survives memo reuse and reaches `Tx::scroll_into_view` and
/// `Tx::scroll_center`, so the app can reveal the current find match (OI-081)
/// without measuring text itself.
pub fn highlight_key(index: usize) -> String {
    format!("hl-{index}")
}

/// The link destination that copies a code block's source.
pub fn copy_code_link(key: &str) -> String {
    format!("copy-code:{key}")
}

/// The link destination that asks the OS to open a media file.
pub fn media_link(src: &str) -> String {
    format!("media:{src}")
}

/// Hands the renderer a picture for an image source.
///
/// The app resolves the path against the document folder and its path policy
/// (M-24–M-31) and returns a decoded [`rune::Image`] from its image cache.
/// `None` means missing (M-25): the renderer draws the alt text and the source
/// in the broken-image style.
pub trait ImageResolver {
    /// The picture for `src`, with `alt` and `title` as written.
    fn resolve(&self, src: &str, alt: &str, title: &str) -> Option<rune::Image>;
}

/// An [`ImageResolver`] that has no pictures: every image is its placeholder.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoImages;

impl ImageResolver for NoImages {
    fn resolve(&self, _src: &str, _alt: &str, _title: &str) -> Option<rune::Image> {
        None
    }
}

/// One task checkbox the app may make live (M-07).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRef {
    /// The top-level block key.
    pub block: String,
    /// The item's index inside its list.
    pub item: usize,
    /// The state the source shows.
    pub checked: bool,
    /// The item's own text, trimmed and capped at 120 characters (M-07).
    pub label: String,
    /// The three source bytes of the `[ ]`/`[x]` marker, from the inline source
    /// map. Nested lists repeat `item`, so this is what lets the app locate the
    /// marker without guessing (OI-092); empty when the parser did not map it.
    pub source: std::ops::Range<usize>,
}

/// What a [`Highlight`] paints, and how strongly it wins an overlap.
///
/// [`Current`](Self::Current) is the match the app is on, [`Find`](Self::Find)
/// the other matches, [`Landing`](Self::Landing) the heading a jump just landed
/// on, and [`Line`](Self::Line) the `--line` source line. In one overlapping
/// region the strongest kind paints once, so a current match inside a find match
/// reads as one current highlight, never a muddled blend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HighlightKind {
    /// A neutral source-line tint (`--line`, scroll-spy).
    Line,
    /// The heading a jump just landed on (O-10's landing flash).
    Landing,
    /// A find match.
    Find,
    /// The current find match: the strongest kind.
    Current,
}

impl HighlightKind {
    /// How strongly this kind paints over another overlapping kind.
    pub(crate) fn priority(self) -> u8 {
        match self {
            Self::Line => 0,
            Self::Landing => 1,
            Self::Find => 2,
            Self::Current => 3,
        }
    }
}

/// One source byte range to paint behind the drawn text.
///
/// The range is in [`lmdv_md::MdDoc::source`], end exclusive. It is mapped
/// through each block's inline source map, so a range that crosses blocks, or
/// covers Markdown markers (`**`, the URL of a link), paints only the text those
/// bytes produced. Zero-length ranges, ranges with no mapped text, and ranges
/// past the source are ignored. An inline formula or picture maps to the box it
/// draws, its TeX or syntax included; a link's destination paints the link's
/// label; a front-matter key or value paints its drawn text; and a heading's
/// bytes also paint the generated TOC label copied from them (OI-127).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Highlight {
    /// The source bytes to paint.
    pub range: std::ops::Range<usize>,
    /// What the range means.
    pub kind: HighlightKind,
}

impl Highlight {
    /// A highlight of `range` with `kind`.
    pub fn new(range: impl Into<std::ops::Range<usize>>, kind: HighlightKind) -> Self {
        Self { range: range.into(), kind }
    }
}

/// Wraps a text element that holds links; the app attaches `on_link`.
///
/// Rune's `El::on_link` is generic over the app state, so the renderer cannot
/// name `Fn(&str)` directly. Build one wire per window, not per frame, from a
/// stateless closure; the renderer disables per-block memoisation while a wire
/// is set:
///
/// ```ignore
/// let wire: lmdv_render::LinkWire = Rc::new(|e| {
///     e.on_link(|state: &mut WindowState, url: &str, tx: &mut rune::Tx| {
///         state.open_link(url, tx);
///     })
/// });
/// let opts = RenderOptions::new(width, &style, &math, &images).link_wire(wire);
/// ```
pub type LinkWire = Rc<dyn Fn(El) -> El>;
/// Wraps a task checkbox row; the app attaches row-level behavior.
pub type TaskWire = Rc<dyn Fn(El, &TaskRef) -> El>;
/// Wraps the real task checkbox control (not its row); the app attaches its
/// press and Space/Enter handlers here, so a tree has one checkbox per item
/// and no transparent overlay is needed (OI-091).
pub type TaskControlWire = Rc<dyn Fn(El, &TaskRef) -> El>;
/// Wraps a top-level block with the source-mapping [`BlockRef`] the app's
/// double-click and context-menu handlers read (E-04, E-35).
pub type BlockWire = Rc<dyn Fn(El, &BlockRef) -> El>;
/// Wraps one drawn GFM table with the [`TableRef`] the app's action bar and
/// preview read (M-06, X-16, V-11–V-18); the element is the table as drawn,
/// including its horizontal scroll viewport when it is wider than the column.
pub type TableWire = Rc<dyn Fn(El, &TableRef) -> El>;
/// Wraps one drawn image or Mermaid diagram with the [`MediaRef`] the app's
/// lightbox reads (V-01–V-10); the element is the media as drawn, so the wire
/// only adds the app's open handler.
pub type MediaWire = Rc<dyn Fn(El, &MediaRef) -> El>;

/// What kind of media a [`MediaRef`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    /// A block image, resolved through [`ImageResolver`].
    Image,
    /// A supported Mermaid diagram.
    Diagram,
}

/// One drawn image or Mermaid diagram, for the app's full-window preview
/// (V-01–V-10).
///
/// Every block image the resolver answered and every supported Mermaid diagram
/// gets one. A missing/broken image and a diagram left as source never do, so
/// the app's open handler exists only where there is something to show. The
/// element handed to the wire is exactly the article's, which keeps a tree
/// without a wire byte-identical to one whose wire returns its element.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaRef {
    /// The top-level block's stable key (`b{index}`).
    pub block: String,
    /// The top-level block's index in [`lmdv_md::MdDoc::blocks`].
    pub index: usize,
    /// Whether this is a raster picture or a vector diagram.
    pub kind: MediaKind,
    /// The picture's resolved source URL, or the diagram's Mermaid source.
    pub source: String,
    /// The image's alt text as written; empty for a diagram.
    pub alt: String,
    /// The content's natural width: image pixels, or the diagram's laid-out
    /// points at the article width.
    pub width: f32,
    /// The content's natural height.
    pub height: f32,
    /// The block's first source line, 0-based.
    pub line: usize,
    /// The line after the block's last source line, 0-based.
    pub end_line: usize,
}

/// One GFM table cell as the renderer drew it, for the app's preview (V-17).
///
/// [`text`](Self::text) is what the cell draws; inline markup disappears, a
/// formula keeps its TeX, and objects that draw no text contribute nothing.
/// [`links`](Self::links) name the drawn runs that are links and their
/// destinations, so the preview can follow them; [`task`](Self::task) records a
/// task checkbox, which the preview shows as an inert glyph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableCell {
    /// The cell's drawn text, whitespace collapsed.
    pub text: String,
    /// The link runs inside [`text`](Self::text), in byte offsets.
    pub links: Vec<TableLink>,
    /// A task checkbox in the cell and its state (`true` checked).
    pub task: Option<bool>,
}

impl TableCell {
    /// A plain cell with no links or task checkbox.
    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), links: Vec::new(), task: None }
    }
}

/// One link drawn inside a [`TableCell`] (V-17).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableLink {
    /// The link's byte range in [`TableCell::text`].
    pub range: std::ops::Range<usize>,
    /// The destination as written.
    pub dest: String,
}

/// One GFM table as the renderer drew it, for the app's action bar, preview and
/// spreadsheet copy (M-06, X-16).
///
/// Every table the article draws gets one, including tables nested in lists and
/// quotes; [`block`](Self::block) and [`index`](Self::index) name the top-level
/// block it sits in, and [`source`](Self::source) is the table's own source
/// range, so two tables in one quote stay distinct. The cells are the drawn
/// cells, one per column in reading order; each row is padded to
/// [`columns`](Self::columns) with empty cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRef {
    /// The top-level block's stable key (`b{index}`).
    pub block: String,
    /// The top-level block's index in [`lmdv_md::MdDoc::blocks`].
    pub index: usize,
    /// The table's own source byte range, end exclusive.
    pub source: std::ops::Range<usize>,
    /// The table header row's 0-based source line.
    pub line: usize,
    /// The line after the table's last source line, 0-based.
    pub end_line: usize,
    /// One alignment per column, as the delimiter row wrote it.
    pub align: Vec<lmdv_md::Align>,
    /// The header cells, one per column (empty cells when the table has no
    /// header row).
    pub head: Vec<TableCell>,
    /// The body rows' cells, one entry per cell.
    pub rows: Vec<Vec<TableCell>>,
    /// The column count (header and body rows agree).
    pub columns: usize,
}

/// One drawn inline text inside a top-level block, for preview→source mapping.
///
/// The app reads the text a double-click selected from
/// `Tx::text_selection()` (`SelectedText::key`, `range`, `len`, `text`) and
/// matches it to one of these to find the word's offset in [`BlockRef::text`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewText {
    /// The text element's own key (`text`, `h3`), when the renderer gave it one.
    pub key: Option<String>,
    /// The text's byte range in [`BlockRef::text`].
    pub range: std::ops::Range<usize>,
    /// The keys the renderer created between the block and this text, outermost
    /// first: a list item's `i{k}`, a table row's `row-{i}`/`head-row` and a
    /// cell's `c{column}`. They match components of `SelectedText::path()`.
    pub path: Vec<String>,
}

/// A top-level block as the renderer drew it (E-04/E-35, M-32).
///
/// The source lines come from [`lmdv_md::MdDoc::block_spans`]; the inline text
/// is the drawn text of the block in reading order, one text element after
/// another separated by a newline, so the app can count the occurrences of a
/// clicked word before its own offset exactly as the original does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRef {
    /// The block's stable key (`b{index}`).
    pub key: String,
    /// The block's first source line, 0-based.
    pub line: usize,
    /// The line after the block's last source line, 0-based.
    pub end_line: usize,
    /// The block is fenced code: search its source as written.
    pub code: bool,
    /// The drawn inline text of the block, in reading order.
    pub text: String,
    /// The text elements that make up [`BlockRef::text`].
    pub texts: Vec<PreviewText>,
}

/// The closures that connect the pure tree to app state.
///
/// All are optional: without them the document is read-only, which is exactly
/// what export and the headless gates need.
#[derive(Clone, Default)]
pub struct RenderWires {
    /// The link handler.
    pub link: Option<LinkWire>,
    /// The task-checkbox row handler.
    pub task: Option<TaskWire>,
    /// The real task-checkbox control handler (preferred over `task`).
    pub task_control: Option<TaskControlWire>,
    /// The block wrapper that maps a preview click to its source line.
    pub block: Option<BlockWire>,
    /// The table wrapper that hands the app each drawn table's data (X-16).
    pub table: Option<TableWire>,
    /// The media wrapper that hands the app each image or diagram (V-01).
    pub media: Option<MediaWire>,
}

impl std::fmt::Debug for RenderWires {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderWires")
            .field("link", &self.link.as_ref().map(|_| "…"))
            .field("task", &self.task.as_ref().map(|_| "…"))
            .field("task_control", &self.task_control.as_ref().map(|_| "…"))
            .field("block", &self.block.as_ref().map(|_| "…"))
            .field("table", &self.table.as_ref().map(|_| "…"))
            .field("media", &self.media.as_ref().map(|_| "…"))
            .finish()
    }
}

/// What one block build collects for a [`BlockWire`]: the drawn text with its
/// elements and the renderer-created key scope (list items, table cells).
#[derive(Debug, Default)]
pub(crate) struct PreviewBlock {
    pub(crate) text: String,
    pub(crate) texts: Vec<PreviewText>,
    pub(crate) scope: Vec<String>,
}

/// Which `<details>` cards are open, by key.
///
/// The front-matter card is [`FRONT_MATTER_KEY`]; an HTML `<details>` uses
/// `details:{block_key}:{hash}`, where the suffix identifies the element
/// within its block, so two sibling cards keep separate state.
pub trait DetailsState {
    /// Whether the card keyed `key` is open.
    fn is_open(&self, key: &str) -> bool;
}

/// Every card closed.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllClosed;

impl DetailsState for AllClosed {
    fn is_open(&self, _key: &str) -> bool {
        false
    }
}

/// Everything the renderer needs beyond the document and theme.
pub struct RenderOptions<'a> {
    /// The reading column's content width in points, used for wide tables and
    /// image scaling.
    pub width: f32,
    /// The kit Markdown style used for Mermaid diagrams (its fields are
    /// crate-private at this rev, so all other tokens come from [`DocTheme`]).
    pub style: &'a MarkdownStyle,
    /// Formula images; [`NoMath`] shows the TeX instead.
    pub math: &'a dyn MathResolver,
    /// Document images; [`NoImages`] shows the missing-image placeholder.
    pub images: &'a dyn ImageResolver,
    /// App handlers (links, task checkboxes, block source mapping).
    pub wires: RenderWires,
    /// Open `<details>` cards.
    pub details: Option<&'a dyn DetailsState>,
    /// Whether prose is selectable and copies as drawn (R-25, R-26).
    pub selectable: bool,
    /// Whether a GFM table's header row sticks to the top of the scroll viewport.
    pub sticky_tables: bool,
    /// The top inset a sticky table header keeps, in points.
    pub sticky_inset: f32,
    /// The display scale (`Env::scale`), used to choose a `srcset` candidate
    /// (M-26: a 2x candidate from 1.5x; otherwise the 1x one).
    pub scale: f32,
    /// Paint-only source ranges (find matches, the current match, `--line`).
    ///
    /// Empty by default, and empty means no highlight work at all. Set it with
    /// [`RenderOptions::highlights`]; the per-block memo version includes only
    /// the ranges that touch the block, so changing one match repaints the
    /// blocks it reaches and reuses the rest.
    pub highlights: Vec<Highlight>,
}

impl<'a> RenderOptions<'a> {
    /// The usual reading options: selectable prose and sticky table headers.
    pub fn new(
        width: f32,
        style: &'a MarkdownStyle,
        math: &'a dyn MathResolver,
        images: &'a dyn ImageResolver,
    ) -> Self {
        Self {
            width: width.max(1.0),
            style,
            math,
            images,
            wires: RenderWires::default(),
            details: None,
            selectable: true,
            sticky_tables: true,
            sticky_inset: 0.0,
            scale: 1.0,
            highlights: Vec::new(),
        }
    }

    /// Attaches the app's link handler.
    pub fn link_wire(mut self, wire: LinkWire) -> Self {
        self.wires.link = Some(wire);
        self
    }

    /// Attaches the app's task-checkbox row handler.
    pub fn task_wire(mut self, wire: TaskWire) -> Self {
        self.wires.task = Some(wire);
        self
    }

    /// Attaches the app's handler to each real task checkbox control, so the
    /// box itself takes the press and the keyboard (OI-091).
    pub fn task_control_wire(mut self, wire: TaskControlWire) -> Self {
        self.wires.task_control = Some(wire);
        self
    }

    /// Attaches the app's handler to each GFM table, with the table's key,
    /// source range and drawn cells (M-06, X-16).
    pub fn table_wire(mut self, wire: TableWire) -> Self {
        self.wires.table = Some(wire);
        self
    }

    /// Attaches the app's handler to each drawn image and Mermaid diagram
    /// (V-01–V-10), with its block, source, alt and natural size.
    pub fn media_wire(mut self, wire: MediaWire) -> Self {
        self.wires.media = Some(wire);
        self
    }

    /// The `<details>` state to read; omitted means every card is closed.
    pub fn details(mut self, details: &'a dyn DetailsState) -> Self {
        self.details = Some(details);
        self
    }

    /// Replaces the paint-only highlights over source byte ranges.
    ///
    /// The usual call is `options.highlights([Highlight::new(range, kind)])`;
    /// an empty iterator turns highlighting off. Ranges are in document source
    /// bytes and may cross blocks. The highlight at index `i` is keyed
    /// [`highlight_key(i)`](highlight_key), which `Tx::scroll_into_view` accepts.
    pub fn highlights(mut self, highlights: impl IntoIterator<Item = Highlight>) -> Self {
        self.highlights = highlights.into_iter().collect();
        self
    }

    /// Whether the `<details>` card keyed `key` is open (closed without state).
    pub(crate) fn open_for(&self, key: &str) -> bool {
        self.details.is_some_and(|state| state.is_open(key))
    }
}

/// Renders `doc` as the reading column: front matter, every block in order, and
/// the generated footnotes.
///
/// Read-only trees are memoised per top-level block; the version covers the
/// document source, the theme fingerprint, the width bucket, the table options
/// and the highlights that touch the block, and wires disable memoisation so app
/// handlers never go stale.
pub fn render(doc: &MdDoc, theme: &DocTheme, opts: &RenderOptions<'_>) -> El {
    use std::hash::{Hash, Hasher};
    // One pass over the source per view build identifies the document to the
    // per-block memos, so a parse that reuses revision 0 for changed text of the
    // same length still invalidates every block.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    doc.source.as_ref().hash(&mut hasher);
    let source_hash = hasher.finish();
    let r = block::Renderer {
        doc,
        theme,
        opts,
        source_hash,
        theme_hash: theme.fingerprint(),
        current_block: std::cell::Cell::new(usize::MAX),
        list_depth: std::cell::Cell::new(0),
        preview: std::cell::RefCell::new(PreviewBlock::default()),
    };
    r.article()
}

/// A horizontal rule the width of the article, as the renderer draws one.
pub(crate) fn horizontal_rule(color: rune::Color) -> El {
    rune::rule(0.5, color).w_full().a11y_hidden()
}
