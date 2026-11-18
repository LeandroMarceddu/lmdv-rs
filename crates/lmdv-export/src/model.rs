//! The format-neutral export model and the public options/traits.
//!
//! [`ExportDoc`] is built once from an [`lmdv_md::MdDoc`] and then serialized by
//! the HTML writer (and, later, the PDF and DOCX writers). It carries no Rune
//! elements, no layout and no I/O: every asset is already embedded bytes, every
//! link is already resolved, and every run is already split at its style and
//! link boundaries.
//!
//! The model deliberately keeps *semantic* structure (headings, lists, tables,
//! callouts, footnotes, code) rather than a cloned DOM, so the same document can
//! be printed, turned into OOXML, or written as HTML without re-parsing.

use std::ops::Range;
use std::path::PathBuf;

use lmdv_md::{Align, CalloutKind, HtmlNode};
use thiserror::Error;

/// Everything the exporter needs besides the document and its assets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    /// The document title. `None` derives one from the source file name, the
    /// first level-1 heading, or the localized “Pasted text” fallback.
    pub title: Option<String>,
    /// The BCP 47 tag for `<html lang>` and the document labels. The
    /// application passes the UI language (X-07); empty means `en`.
    pub lang: String,
    /// Add a table-of-contents navigation when the document has no explicit
    /// `[TOC]` marker. An explicit marker is always rendered.
    pub include_toc: bool,
    /// Embed formulas as PNG images (2× raster). `false` leaves the TeX source
    /// in `<code class="math-tex">`.
    pub math: bool,
    /// Include the YAML front matter as a collapsed `<details>` card. The
    /// original export omits it, so this is off by default.
    pub front_matter: bool,
    /// The Markdown file this document came from, when it has one.
    ///
    /// It is used for the title fallback and to turn relative local links into
    /// `file:///` URLs. The exporter never touches the filesystem, so the value
    /// is only treated as a lexical path; pass an absolute path when possible.
    pub source_path: Option<PathBuf>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            title: None,
            lang: "en".to_owned(),
            include_toc: false,
            math: true,
            front_matter: false,
            source_path: None,
        }
    }
}

/// Asset bytes supplied by the application, with their MIME type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportedAsset {
    /// The image MIME type (`image/png`, `image/jpeg`, …). Non-image types are
    /// rejected by the exporter.
    pub mime: &'static str,
    /// The encoded file bytes.
    pub bytes: Vec<u8>,
}

impl ExportedAsset {
    /// An asset with `mime` and `bytes`.
    pub fn new(mime: &'static str, bytes: Vec<u8>) -> Self {
        Self { mime, bytes }
    }
}

/// Resolves document images and Mermaid diagrams to bytes.
///
/// The implementation belongs to the application: it applies the document
/// folder and the path policy, decodes the picture and returns the encoded
/// file. The exporter calls [`AssetResolver::image`] only for local, `file:` and
/// `data:` sources; a network URL is never fetched and is shown as alt text.
/// Returning `None` means the picture is missing and the export keeps its alt
/// text, so the output is still self-contained.
pub trait AssetResolver {
    /// The picture for an image source, with the alt text as written.
    fn image(&self, src: &str, alt: &str) -> Option<ExportedAsset>;
    /// A PNG or SVG for a Mermaid source. `None` keeps the diagram source as a
    /// code block.
    fn diagram(&self, source: &str) -> Option<ExportedAsset>;
}

/// A resolver with no assets: images keep their alt text and diagrams their
/// source. Useful for tests and for exports without picture access.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoAssets;

impl AssetResolver for NoAssets {
    fn image(&self, _src: &str, _alt: &str) -> Option<ExportedAsset> {
        None
    }

    fn diagram(&self, _source: &str) -> Option<ExportedAsset> {
        None
    }
}

/// An asset's stable id inside [`ExportDoc::assets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub u32);

impl AssetId {
    /// The position in [`ExportDoc::assets`].
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// One embedded asset: a picture, a formula raster or a diagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportAsset {
    /// The id runs and blocks refer to.
    pub id: AssetId,
    /// The MIME type. Only image types are accepted.
    pub mime: &'static str,
    /// The encoded file bytes.
    pub bytes: Vec<u8>,
    /// The preferred display width in CSS pixels, when known (math).
    pub width: Option<u32>,
    /// The preferred display height in CSS pixels, when known (math).
    pub height: Option<u32>,
    /// How far the picture descends below the text baseline, in CSS pixels
    /// (math only). `None` leaves the default baseline alignment.
    pub baseline: Option<u32>,
}

impl ExportAsset {
    /// The `data:` URI the HTML writer embeds.
    pub fn data_uri(&self) -> String {
        use base64::Engine as _;
        format!("data:{};base64,{}", self.mime, base64::engine::general_purpose::STANDARD.encode(&self.bytes))
    }
}

/// A non-fatal problem the caller may show after an export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportWarningKind {
    /// A formula could not be rendered; the TeX source was kept.
    MathFallback,
    /// An image had no readable local bytes.
    ImageMissing,
    /// An image source was refused (network, unsupported type, unsafe SVG).
    ImageRefused,
    /// A diagram had no rendered picture; its source was kept.
    DiagramFallback,
    /// A link destination was refused (script/data URL or unknown scheme).
    LinkRefused,
}

/// One non-fatal export problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportWarning {
    /// What went wrong.
    pub kind: ExportWarningKind,
    /// A short, user-readable description.
    pub message: String,
    /// The source or TeX that caused it, when there is one.
    pub source: Option<String>,
}

/// Why an export failed outright.
///
/// Missing pictures and unrenderable formulas are not errors: they fall back to
/// alt text and TeX so a self-contained file is always produced. Only an
/// internal encoding or typesetting failure aborts the export.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ExportError {
    /// A rasterized formula could not be encoded as PNG.
    #[error("could not encode a math image as PNG: {0}")]
    MathImage(String),
    /// The PDF typesetter could not lay out or write the document.
    ///
    /// This is an engine failure (an invalid option, a font that is not a
    /// font, or a bug in the generated Typst source), not a content problem:
    /// missing pictures and formulas still fall back to text.
    #[error("could not write the PDF: {0}")]
    Pdf(String),
}

/// The styles one export run carries.
///
/// The flags map to the original viewer's inline styles. Links are separate
/// ([`ExportRun::Text`]) because they need an href and a title.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RunStyle {
    /// `**strong**`
    pub strong: bool,
    /// `*emphasis*`
    pub em: bool,
    /// `~~strikethrough~~`
    pub strike: bool,
    /// Inline code.
    pub code: bool,
    /// `<mark>`
    pub mark: bool,
    /// `<sub>`
    pub sub: bool,
    /// `<sup>`
    pub sup: bool,
    /// `<kbd>`
    pub kbd: bool,
    /// `<u>` / `<ins>`
    pub underline: bool,
}

impl RunStyle {
    /// Whether the style draws nothing.
    pub fn is_plain(self) -> bool {
        self == Self::default()
    }
}

/// A resolved link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportLink {
    /// The destination, already made `file:///` for relative local links.
    pub href: String,
    /// The optional title.
    pub title: Option<String>,
}

/// One inline run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportRun {
    /// Styled text.
    Text {
        /// The text, without Markdown markers or object placeholders.
        text: String,
        /// The styles over the text.
        style: RunStyle,
        /// The link covering the text, when there is one.
        link: Option<ExportLink>,
    },
    /// A formula, with its raster asset when math is enabled and succeeded.
    Math {
        /// The TeX source, without delimiters.
        tex: String,
        /// Whether it is display math.
        display: bool,
        /// The embedded PNG, if any.
        asset: Option<AssetId>,
        /// A link covering the formula, when there is one.
        link: Option<ExportLink>,
    },
    /// An inline picture.
    Image {
        /// The picture and its source.
        image: ExportImage,
        /// A link covering the picture, when there is one.
        link: Option<ExportLink>,
    },
    /// A hard line break.
    Break,
    /// A footnote reference, numbered by first reference.
    FootnoteRef {
        /// The definition label.
        label: String,
        /// The 1-based number.
        number: usize,
        /// Which reference to this definition it is (0 is the first).
        occurrence: usize,
    },
    /// A preserved, already-sanitized HTML subtree (a details card or media).
    Html(HtmlNode),
}

/// A picture with its resolved asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportImage {
    /// The source as written (or the chosen light `<picture>` candidate).
    pub src: String,
    /// The alt text.
    pub alt: String,
    /// The optional title.
    pub title: Option<String>,
    /// The embedded bytes, or `None` when the picture is missing.
    pub asset: Option<AssetId>,
}

/// One entry of a table-of-contents navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportTocEntry {
    /// The heading id without `#`.
    pub id: String,
    /// The heading's plain text.
    pub text: String,
    /// The heading level (1..=6).
    pub level: u8,
    /// The entries nested below this one.
    pub children: Vec<ExportTocEntry>,
}

/// A table with its column alignments and cells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportTable {
    /// The column alignments.
    pub align: Vec<Align>,
    /// The header cells.
    pub head: Vec<Vec<ExportRun>>,
    /// The body rows, each with one run list per cell.
    pub rows: Vec<Vec<Vec<ExportRun>>>,
}

/// A list item, with its task state and accessible label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportItem {
    /// `Some(checked)` for a task item.
    pub task: Option<bool>,
    /// The item's own text, trimmed and capped at 120 characters.
    pub label: String,
    /// The item's blocks.
    pub blocks: Vec<ExportBlock>,
}

/// A bullet, ordered or task list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportList {
    /// Whether the list is ordered.
    pub ordered: bool,
    /// The first number of an ordered list.
    pub start: u64,
    /// Whether the list is tight (no blank lines between blocks).
    pub tight: bool,
    /// The items.
    pub items: Vec<ExportItem>,
}

/// A GitHub alert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportCallout {
    /// The alert kind.
    pub kind: CalloutKind,
    /// A custom title, or `None` for the kind's default.
    pub title: Option<String>,
}

/// A footnote definition that at least one reference points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportFootnote {
    /// The label written between `[^` and `]`.
    pub label: String,
    /// The 1-based number.
    pub number: usize,
    /// The definition's blocks.
    pub blocks: Vec<ExportBlock>,
}

/// The leading YAML front matter, when the export includes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportFrontMatter {
    /// The YAML between the fences, verbatim.
    pub raw: String,
    /// Whether the flat subset parsed.
    pub valid: bool,
    /// Flat entries; lists are flattened to their values.
    pub entries: Vec<(String, Vec<String>)>,
}

/// One block of the export model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportBlock {
    /// A heading with its stable GitHub id.
    Heading {
        /// The level, 1..=6.
        level: u8,
        /// The id without `#`.
        id: String,
        /// The heading content.
        runs: Vec<ExportRun>,
    },
    /// A paragraph.
    Para(Vec<ExportRun>),
    /// A list.
    List(ExportList),
    /// A block quote, optionally a GitHub alert.
    Quote {
        /// The alert kind, when the quote is one.
        callout: Option<ExportCallout>,
        /// The quote body.
        blocks: Vec<ExportBlock>,
    },
    /// A fenced or indented code block.
    Code {
        /// The first word of the fence info string.
        lang: Option<String>,
        /// The code, without the fence and without one trailing newline.
        text: String,
    },
    /// A GFM table.
    Table(ExportTable),
    /// Display math.
    Math {
        /// The TeX source.
        tex: String,
        /// Always `true` for this variant; kept for the DOCX/PDF writers.
        display: bool,
        /// The embedded PNG, if any.
        asset: Option<AssetId>,
    },
    /// A block-level picture.
    Image(ExportImage),
    /// A Mermaid diagram, rendered to a picture when the resolver can.
    Diagram {
        /// The diagram source.
        source: String,
        /// The rendered picture, if any.
        asset: Option<AssetId>,
    },
    /// A thematic break.
    Rule,
    /// A table of contents.
    Toc {
        /// The entries, nested.
        entries: Vec<ExportTocEntry>,
    },
    /// The generated footnote section.
    Footnotes,
    /// An already-sanitized HTML subtree from the allowed subset.
    Html(HtmlNode),
}

/// A fully resolved document, ready for a format writer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportDoc {
    /// The `<title>` / metadata title.
    pub title: String,
    /// The document language (BCP 47); it also selects the localized document
    /// labels.
    pub lang: String,
    /// The body blocks, in document order.
    pub blocks: Vec<ExportBlock>,
    /// The source byte range of each entry in [`ExportDoc::blocks`], parallel
    /// to it.
    ///
    /// `None` marks a block with no source of its own (the TOC an export may
    /// insert) or a model that was built without a parsed document. The ranges
    /// are what [`blocks_for_source`] maps a selection through; they are
    /// metadata only and no writer serializes them.
    pub block_sources: Vec<Option<Range<usize>>>,
    /// The referenced footnote definitions, numbered by first reference.
    pub footnotes: Vec<ExportFootnote>,
    /// Embedded pictures, formulas and diagrams, in first-use order.
    pub assets: Vec<ExportAsset>,
    /// The front matter, when the export includes it.
    pub front_matter: Option<ExportFrontMatter>,
    /// The Markdown file this document came from, when known.
    pub source_path: Option<PathBuf>,
    /// Non-fatal problems encountered while building the document.
    pub warnings: Vec<ExportWarning>,
}

impl ExportDoc {
    /// Builds the export model from a parsed document.
    pub fn from_md(
        doc: &lmdv_md::MdDoc,
        opts: &ExportOptions,
        assets: &dyn AssetResolver,
    ) -> Result<Self, ExportError> {
        crate::build::build(doc, opts, assets)
    }

    /// The asset with `id`.
    pub fn asset(&self, id: AssetId) -> Option<&ExportAsset> {
        self.assets.get(id.index()).filter(|asset| asset.id == id)
    }

    /// The source byte range of the top-level block at `index`, when the model
    /// came from [`ExportDoc::from_md`] and the block has one.
    pub fn block_source(&self, index: usize) -> Option<&Range<usize>> {
        self.block_sources.get(index)?.as_ref()
    }

    /// The TSV of the table block at `index` (X-16).
    ///
    /// `None` when the block is not a table. `visible` masks body rows exactly
    /// like [`crate::table_tsv`].
    pub fn table_tsv(&self, index: usize, visible: Option<&[bool]>) -> Option<String> {
        match self.blocks.get(index)? {
            ExportBlock::Table(table) => Some(crate::tsv::table_tsv(table, visible)),
            _ => None,
        }
    }

    /// Serializes this document as one self-contained HTML file.
    pub fn to_html(&self) -> Result<String, ExportError> {
        Ok(crate::html::render(self))
    }
}

/// Maps a source byte range to the top-level export blocks it touches.
///
/// The result is the contiguous block range whose source spans intersect
/// `bytes`, ready to pass to [`crate::export_rich_fragment`] (X-03). A
/// selection that touches no block — an empty range, the front matter, or only
/// the separators between blocks — returns `0..0`, which renders as an empty
/// fragment. A model built without source spans (a hand-built [`ExportDoc`])
/// also returns `0..0`; the application must then map the reader's selection
/// to block keys itself.
///
/// The range is half-open like a selection: a block selected up to its last
/// byte is included, and an end exactly at the next block's start does not
/// include it.
pub fn blocks_for_source(doc: &ExportDoc, bytes: Range<usize>) -> Range<usize> {
    if bytes.is_empty() {
        return 0..0;
    }
    let mut first = None;
    let mut last = 0;
    for (index, source) in doc.block_sources.iter().enumerate() {
        let Some(source) = source else { continue };
        if !source.is_empty() && source.end > bytes.start && source.start < bytes.end {
            first.get_or_insert(index);
            last = index;
        }
    }
    first.map_or(0..0, |first| first..last + 1)
}
