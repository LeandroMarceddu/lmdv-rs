//! Deterministic OOXML (`.docx`) serialization of an [`ExportDoc`].
//!
//! The writer is deliberately small: it maps the export model to Word
//! paragraphs, real `Heading1`–`Heading6` styles, list numbering, GFM tables,
//! shaded callout/code cells, `wp:inline` pictures and external hyperlinks.
//! Nothing here reads a file, fetches a network resource or understands HTML:
//! the model already carries resolved bytes and `file:///` links.
//!
//! Layout is print-safe and fixed, never the reader's preferences (ADR-015,
//! X-10): A4 portrait, 2.54 cm margins, Calibri 11 pt body, 9.5 pt mono code.
//! The original exported through Word's HTML filter, which only understands
//! simple CSS; here the same choices become real Word styles so the navigation
//! pane, table of contents and list numbering work.
//!
//! Deliberate limits (see the worker report and `docs/OPEN_ITEMS.md`):
//!
//! - Math is the embedded 2× PNG with the TeX in `wp:docPr` `descr`/`title`;
//!   OMML is not generated (OI-066).
//! - Footnotes are a real `word/footnotes.xml` part: one `w:footnote` per
//!   referenced definition with the number marker, its blocks and a backlink
//!   to a bookmark around the first reference. The body only carries
//!   `w:footnoteReference` runs, not an end-of-document list. A footnote that
//!   references another keeps the inner number as superscript text, because
//!   Word has no nested notes.
//! - SVG pictures are rasterized to PNG on the export worker (OI-076), with
//!   text shaped by Typst's bundled faces. AVIF still needs a decoder that is
//!   not in the dependency tree, so it degrades to its alt text.
//! - The table of contents is a real Word `TOC` field (OI-078): the entry
//!   paragraphs are its cached result, and `w:updateFields` lets Word rebuild
//!   them with page numbers when the document opens.
//! - The package is deflated (OI-079): XML and relationship parts use the
//!   fixed-level raw deflate stream that is now in the dependency tree, while
//!   already-compressed pictures stay stored.
//! - Warnings are not appended to the document: [`export_docx`] takes an
//!   immutable [`ExportDoc`] and returns bytes only.

use std::collections::HashMap;

use lmdv_i18n::{Key, text_in};
use lmdv_md::{Align, CalloutKind, HtmlChild, HtmlNode, HtmlTag};

use crate::labels::ExportLabels;
use crate::model::{
    AssetId, ExportAsset, ExportBlock, ExportCallout, ExportDoc, ExportError, ExportFootnote, ExportImage, ExportLink,
    ExportList, ExportRun, ExportTable, ExportTocEntry, RunStyle,
};

mod package;
mod raster;

/// The table-of-contents label, matching the HTML exporter.
const TOC_LABEL: Key = lmdv_i18n::key("render.toc");

/// The XML namespace prefixes every document part needs.
const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const WP_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const PIC_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const REL_STYLES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
const REL_NUMBERING: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
const REL_SETTINGS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings";
const REL_FOOTNOTES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const REL_HYPERLINK: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

/// English Metric Units per CSS pixel at 96 dpi (the asset model's unit).
const EMU_PER_PIXEL: u64 = 9_525;
/// English Metric Units per centimetre.
const EMU_PER_CM: u64 = 360_000;
/// The largest picture width an inline drawing may take: 16 cm.
const MAX_MEDIA_EMU: u64 = 16 * EMU_PER_CM;
/// A picture with no knowable dimensions keeps a 4:3 box.
const FALLBACK_MEDIA: (u64, u64) = (640, 480);
/// The fixed metadata timestamp that makes the package byte-identical.
const FIXED_TIMESTAMP: &str = "2000-01-01T00:00:00Z";

/// Twips of indentation per list/quote level (half an inch).
const INDENT: u32 = 720;
/// Twips a block quote adds on top of its parent.
const QUOTE_INDENT: u32 = 360;

const XML_DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// Word merges two adjacent `w:tbl` elements into one table, so a small empty
/// paragraph separates consecutive tables (and closes a table at the end of
/// the body or of a callout cell).
const TABLE_SEPARATOR: &str = "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" \
    w:line=\"120\" w:lineRule=\"auto\"/></w:pPr></w:p>";

/// The opening of a real Word `TOC` field: the instruction covers heading
/// levels 1–6, links the entries and honours outline levels. The entry
/// paragraphs that follow are its cached result. `w:dirty` makes Word refresh
/// the cached result even when the user declines the open-time update.
const TOC_FIELD_START: &str = concat!(
    "<w:r><w:fldChar w:fldCharType=\"begin\" w:dirty=\"true\"/></w:r>",
    "<w:r><w:instrText xml:space=\"preserve\"> TOC \\o \"1-6\" \\h \\z \\u </w:instrText></w:r>",
    "<w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>",
);
/// The closing `w:fldChar` of the TOC field, in the last cached entry.
const TOC_FIELD_END: &str = "<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>";

/// The character style a `w:footnoteReference`/`w:footnoteRef` run uses; the
/// style itself carries the superscript (Word's own template does the same).
const FOOTNOTE_REFERENCE_STYLE: &str = "FootnoteReference";
/// The paragraph style of a footnote's text in `word/footnotes.xml`.
const FOOTNOTE_TEXT_STYLE: &str = "FootnoteText";

/// The two pseudo-footnotes Word always expects first in the part: the reading
/// rule and its continuation on the next page. `w:id` -1 and 0 are reserved.
const FOOTNOTE_SEPARATOR: &str = "<w:footnote w:type=\"separator\" w:id=\"-1\"><w:p>\
    <w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
    <w:r><w:separator/></w:r></w:p></w:footnote>";
const FOOTNOTE_CONTINUATION: &str = "<w:footnote w:type=\"continuationSeparator\" w:id=\"0\"><w:p>\
    <w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
    <w:r><w:continuationSeparator/></w:r></w:p></w:footnote>";

const QUOTE_BORDER: &str = "<w:pBdr><w:left w:val=\"single\" w:sz=\"12\" w:space=\"8\" w:color=\"D1D9E0\"/></w:pBdr>";
const HEADING_RULE: &str = "<w:pBdr><w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"2\" w:color=\"D1D9E0\"/></w:pBdr>";
const RULE_BORDER: &str = "<w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"D1D9E0\"/></w:pBdr>";

const GRID_BORDERS: &str = "<w:tblBorders>\
    <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D1D9E0\"/>\
    <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D1D9E0\"/>\
    <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D1D9E0\"/>\
    <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D1D9E0\"/>\
    <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D1D9E0\"/>\
    <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D1D9E0\"/>\
    </w:tblBorders>";

const GRID_LOOK: &str = "<w:tblLook w:val=\"04A0\" w:firstRow=\"1\" w:lastRow=\"0\" \
    w:firstColumn=\"1\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/>";
const PLAIN_LOOK: &str = "<w:tblLook w:val=\"0000\" w:firstRow=\"0\" w:lastRow=\"0\" \
    w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"0\"/>";

const HEADER_FILL: &str = "F6F8FA";
const BAND_FILL: &str = "FBFCFD";
const CODE_FILL: &str = "F6F8FA";
const BODY_COLOR: &str = "1F2328";
const MUTED_COLOR: &str = "59636E";

/// Page geometry for the DOCX section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocxPage {
    /// Page width in twentieths of a point (twips).
    pub width_twips: u32,
    /// Page height in twips.
    pub height_twips: u32,
    /// Every margin in twips.
    pub margin_twips: u32,
}

impl DocxPage {
    /// A4 portrait (21 × 29.7 cm) with Word's normal 2.54 cm margins.
    pub const A4: Self = Self { width_twips: 11_906, height_twips: 16_838, margin_twips: 1_440 };
}

impl Default for DocxPage {
    fn default() -> Self {
        Self::A4
    }
}

/// Print-safe Word document settings.
///
/// The defaults are what the original produced through its DOCX stylesheet:
/// Calibri 11 pt body (`22` half-points), Consolas 9.5 pt code, A4 portrait.
/// The reader's theme, size and column width never reach these values
/// (ADR-015, X-10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocxOptions {
    /// The body font family. Word falls back when it is not installed.
    pub body_font: String,
    /// The body size in half-points (`22` = 11 pt).
    pub body_size_half_points: u32,
    /// The monospace family used by inline code and code blocks.
    pub code_font: String,
    /// Page size and margins.
    pub page: DocxPage,
}

impl Default for DocxOptions {
    fn default() -> Self {
        Self {
            body_font: "Calibri".to_owned(),
            body_size_half_points: 22,
            code_font: "Consolas".to_owned(),
            page: DocxPage::A4,
        }
    }
}

/// Writes `doc` as one `.docx` package.
///
/// The package is deterministic: equal documents produce byte-identical
/// archives because the entry order, the metadata timestamps and the ZIP
/// timestamps are fixed. Missing pictures, unrenderable formulas and diagrams
/// without a raster fall back to text, exactly as the HTML writer does.
///
/// The in-memory package writer cannot fail for parts under the ZIP32 limit;
/// the `Result` mirrors [`ExportDoc::to_html`] and leaves room for a future
/// checked step (OMML, for example).
pub fn export_docx(doc: &ExportDoc, opts: &DocxOptions) -> Result<Vec<u8>, ExportError> {
    Ok(render(doc, opts))
}

impl ExportDoc {
    /// Serializes this document as a `.docx` package; see [`export_docx`].
    pub fn to_docx(&self, opts: &DocxOptions) -> Result<Vec<u8>, ExportError> {
        export_docx(self, opts)
    }
}

/// Renders the whole package.
pub(crate) fn render(doc: &ExportDoc, opts: &DocxOptions) -> Vec<u8> {
    let mut writer = Writer::new(doc, opts);
    let mut body = String::new();
    writer.title(&mut body);
    writer.blocks(&mut body, &doc.blocks, Ctx::default());
    if body.is_empty() {
        body.push_str("<w:p/>");
    }
    if body.ends_with("</w:tbl>") {
        body.push_str(TABLE_SEPARATOR);
    }
    // The notes part is written after the body: it registers the media and
    // hyperlink relationships of the note text and needs the body's footnote
    // bookmarks (`footnote_anchors`) for its backlinks.
    let footnotes = (!doc.footnotes.is_empty()).then(|| writer.footnotes_xml());
    let document = writer.document_xml(&body);
    let styles = writer.styles_xml();
    let numbering = writer.numbering_xml();
    let rels = writer.rels_xml();
    // A footnotes part needs its own relationship file only when its text
    // embeds a picture/formula or links somewhere.
    let footnote_rels = (!writer.footnote_rels.is_empty()).then(|| writer.footnote_rels_xml());
    let core = writer.core_xml();
    let has_toc = doc.blocks.iter().any(|block| matches!(block, ExportBlock::Toc { entries } if !entries.is_empty()));

    let mut parts = vec![
        package::Part::text("[Content_Types].xml", package::content_types(!doc.footnotes.is_empty())),
        package::Part::text("_rels/.rels", package::root_rels()),
        package::Part::text("word/document.xml", document),
        package::Part::text("word/styles.xml", styles),
        package::Part::text("word/numbering.xml", numbering),
        package::Part::text("word/settings.xml", package::settings(has_toc)),
    ];
    if let Some(footnotes) = footnotes {
        parts.push(package::Part::text("word/footnotes.xml", footnotes));
    }
    parts.push(package::Part::text("word/_rels/document.xml.rels", rels));
    if let Some(footnote_rels) = footnote_rels {
        parts.push(package::Part::text("word/_rels/footnotes.xml.rels", footnote_rels));
    }
    parts.push(package::Part::text("docProps/core.xml", core));
    parts.push(package::Part::text("docProps/app.xml", package::app()));
    for media in &writer.media {
        if let Some(asset) = doc.asset(media.asset) {
            let bytes = media.raster.clone().unwrap_or_else(|| asset.bytes.clone());
            parts.push(package::Part { name: format!("word/media/{}", media.file_name), bytes });
        }
    }
    package::zip(&parts)
}

// ---- model helpers -------------------------------------------------------

/// One relationship in a part's `.rels` file.
struct Rel {
    id: String,
    kind: &'static str,
    target: String,
    external: bool,
}

/// Which OOXML part a relationship belongs to.
///
/// Every part has its own relationships file, so a picture, formula or
/// hyperlink used inside a footnote must be declared in
/// `word/_rels/footnotes.xml.rels`; reusing a document id there would point at
/// a relationship Word cannot resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Document,
    Footnotes,
}

impl Part {
    /// The prefix of this part's relationship-deduplication keys.
    fn prefix(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Footnotes => "footnotes",
        }
    }
}

/// One numbering instance. Every list gets its own `w:num`, so a nested list
/// restarts and a `start` other than 1 only affects its own list.
struct NumberDef {
    id: u32,
    abstract_id: u32,
    level: u8,
    start: Option<u64>,
}

/// One embedded picture in `word/media/`.
///
/// The bytes are shared by every part; the relationship id is not: a picture
/// first used in the body and then in a footnote needs one `Rel` in each part's
/// `.rels` file.
struct Media {
    file_name: String,
    asset: AssetId,
    /// A converted raster (an SVG rendered to PNG); `None` means the asset's
    /// own bytes.
    raster: Option<Vec<u8>>,
    /// The converted raster's pixel size, for the drawing extent.
    size: Option<(u64, u64)>,
    document_rel: Option<String>,
    footnote_rel: Option<String>,
}

impl Media {
    /// The relationship id of this media in `part`, when already registered.
    fn rel(&self, part: Part) -> Option<&String> {
        match part {
            Part::Document => self.document_rel.as_ref(),
            Part::Footnotes => self.footnote_rel.as_ref(),
        }
    }

    /// Records the relationship id of this media in `part`.
    fn set_rel(&mut self, part: Part, id: String) {
        match part {
            Part::Document => self.document_rel = Some(id),
            Part::Footnotes => self.footnote_rel = Some(id),
        }
    }
}

/// The traversal context: quote depth and list depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Ctx {
    /// Twips added by enclosing block quotes.
    base: u32,
    /// List nesting depth (`0` outside a list).
    depth: u8,
    /// Whether the context is inside a block quote.
    quote: bool,
    /// Whether the context is the `word/footnotes.xml` part. Referencing a
    /// footnote from a footnote keeps the inner number as superscript text:
    /// Word has no nested notes.
    in_note: bool,
}

impl Ctx {
    /// The left indent for a block in this context.
    fn indent(self) -> u32 {
        self.base + u32::from(self.depth) * INDENT
    }

    /// The context inside a block quote.
    fn quoted(self) -> Self {
        Self { base: self.base + QUOTE_INDENT, quote: true, ..self }
    }

    /// The context of a list item at `self.depth`.
    fn item(self) -> Self {
        Self { depth: self.depth.saturating_add(1), ..self }
    }
}

/// The paragraph properties the writer emits, in OOXML schema order.
#[derive(Clone, Copy, Default)]
struct Ppr {
    style: Option<&'static str>,
    keep_next: bool,
    keep_lines: bool,
    numbering: Option<(u8, u32)>,
    border: Option<&'static str>,
    before: Option<u32>,
    after: Option<u32>,
    line: Option<u32>,
    indent: Option<u32>,
    hanging: Option<u32>,
    jc: Option<&'static str>,
    outline: Option<u8>,
    color: Option<&'static str>,
}

impl Ppr {
    fn render(self) -> String {
        let mut p = String::new();
        if let Some(style) = self.style {
            p.push_str("<w:pStyle w:val=\"");
            p.push_str(style);
            p.push_str("\"/>");
        }
        if self.keep_next {
            p.push_str("<w:keepNext/>");
        }
        if self.keep_lines {
            p.push_str("<w:keepLines/>");
        }
        if let Some((level, id)) = self.numbering {
            p.push_str("<w:numPr><w:ilvl w:val=\"");
            p.push_str(&level.min(8).to_string());
            p.push_str("\"/><w:numId w:val=\"");
            p.push_str(&id.to_string());
            p.push_str("\"/></w:numPr>");
        }
        if let Some(border) = self.border {
            p.push_str(border);
        }
        if self.before.is_some() || self.after.is_some() || self.line.is_some() {
            p.push_str("<w:spacing");
            if let Some(before) = self.before {
                p.push_str(&format!(" w:before=\"{before}\""));
            }
            if let Some(after) = self.after {
                p.push_str(&format!(" w:after=\"{after}\""));
            }
            if let Some(line) = self.line {
                p.push_str(&format!(" w:line=\"{line}\" w:lineRule=\"auto\""));
            }
            p.push_str("/>");
        }
        if let Some(indent) = self.indent {
            p.push_str(&format!("<w:ind w:left=\"{indent}\""));
            if let Some(hanging) = self.hanging {
                p.push_str(&format!(" w:hanging=\"{hanging}\""));
            }
            p.push_str("/>");
        } else if let Some(hanging) = self.hanging {
            p.push_str(&format!("<w:ind w:hanging=\"{hanging}\"/>"));
        }
        if let Some(jc) = self.jc {
            p.push_str(&format!("<w:jc w:val=\"{jc}\"/>"));
        }
        if let Some(outline) = self.outline {
            p.push_str(&format!("<w:outlineLvl w:val=\"{outline}\"/>"));
        }
        if let Some(color) = self.color {
            p.push_str("<w:rPr><w:color w:val=\"");
            p.push_str(color);
            p.push_str("\"/></w:rPr>");
        }
        p
    }
}

/// A paragraph with its rendered run XML.
fn paragraph(ppr: Ppr, content: &str) -> String {
    let props = ppr.render();
    if props.is_empty() {
        format!("<w:p>{content}</w:p>")
    } else {
        format!("<w:p><w:pPr>{props}</w:pPr>{content}</w:p>")
    }
}

/// The paragraph properties for body text in `ctx`.
fn body_ppr(ctx: Ctx) -> Ppr {
    let indent = ctx.indent();
    Ppr {
        style: if ctx.in_note {
            // Notes are their own part and use Word's footnote text style for
            // every paragraph the body layout produces (code cells and lists
            // keep their own specific styles).
            Some(FOOTNOTE_TEXT_STYLE)
        } else if ctx.quote {
            Some("Quote")
        } else if ctx.depth > 0 {
            Some("ListParagraph")
        } else {
            None
        },
        indent: (indent > 0).then_some(indent),
        color: ctx.quote.then_some(MUTED_COLOR),
        ..Ppr::default()
    }
}

/// A table row ready to serialize.
struct TableRow {
    header: bool,
    cells: Vec<TableCell>,
}

/// One table cell with its already-rendered paragraphs.
struct TableCell {
    header: bool,
    paragraphs: String,
}

/// Serializes a table. `width` and `indent` are twips.
fn table_xml(width: u32, indent: u32, borders: &str, look: &str, rows: &[TableRow], banding: bool) -> String {
    let columns = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0).max(1);
    let base = (width / columns as u32).max(1);
    let mut widths = vec![base; columns];
    if columns > 1 {
        widths[columns - 1] = width.saturating_sub(base * (columns as u32 - 1)).max(1);
    }
    let mut out = String::from("<w:tbl><w:tblPr>");
    out.push_str(&format!("<w:tblW w:w=\"{width}\" w:type=\"dxa\"/>"));
    if indent > 0 {
        out.push_str(&format!("<w:tblInd w:w=\"{indent}\" w:type=\"dxa\"/>"));
    }
    out.push_str(borders);
    out.push_str("<w:tblLayout w:type=\"fixed\"/>");
    out.push_str("<w:tblCellMar><w:top w:w=\"60\" w:type=\"dxa\"/><w:left w:w=\"120\" w:type=\"dxa\"/>");
    out.push_str("<w:bottom w:w=\"60\" w:type=\"dxa\"/><w:right w:w=\"120\" w:type=\"dxa\"/></w:tblCellMar>");
    out.push_str(look);
    out.push_str("</w:tblPr><w:tblGrid>");
    for column_width in &widths {
        out.push_str(&format!("<w:gridCol w:w=\"{column_width}\"/>"));
    }
    out.push_str("</w:tblGrid>");
    let mut body_row = 0usize;
    for row in rows {
        out.push_str("<w:tr><w:trPr><w:cantSplit/>");
        if row.header {
            out.push_str("<w:tblHeader/>");
        }
        out.push_str("</w:trPr>");
        for (column, cell) in row.cells.iter().enumerate() {
            let cell_width = widths.get(column).copied().unwrap_or(base);
            let fill = if cell.header {
                Some(HEADER_FILL)
            } else if banding && body_row % 2 == 1 {
                Some(BAND_FILL)
            } else {
                None
            };
            out.push_str("<w:tc><w:tcPr>");
            out.push_str(&format!("<w:tcW w:w=\"{cell_width}\" w:type=\"dxa\"/>"));
            if let Some(fill) = fill {
                out.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{fill}\"/>"));
            }
            out.push_str("<w:vAlign w:val=\"top\"/></w:tcPr>");
            if cell.paragraphs.is_empty() {
                out.push_str("<w:p/>");
            } else {
                out.push_str(&cell.paragraphs);
            }
            out.push_str("</w:tc>");
        }
        out.push_str("</w:tr>");
        if !row.header {
            body_row += 1;
        }
    }
    out.push_str("</w:tbl>");
    out
}

/// A one-cell table, used for code blocks and callouts.
fn one_cell_table(width: u32, indent: u32, borders: &str, fill: &str, cell: &str) -> String {
    let mut out = String::from("<w:tbl><w:tblPr>");
    out.push_str(&format!("<w:tblW w:w=\"{width}\" w:type=\"dxa\"/>"));
    if indent > 0 {
        out.push_str(&format!("<w:tblInd w:w=\"{indent}\" w:type=\"dxa\"/>"));
    }
    out.push_str(borders);
    out.push_str("<w:tblLayout w:type=\"fixed\"/>");
    out.push_str("<w:tblCellMar><w:top w:w=\"100\" w:type=\"dxa\"/><w:left w:w=\"140\" w:type=\"dxa\"/>");
    out.push_str("<w:bottom w:w=\"100\" w:type=\"dxa\"/><w:right w:w=\"140\" w:type=\"dxa\"/></w:tblCellMar>");
    out.push_str(PLAIN_LOOK);
    out.push_str(&format!(
        "</w:tblPr><w:tblGrid><w:gridCol w:w=\"{width}\"/></w:tblGrid>\
         <w:tr><w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>\
         <w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{fill}\"/>\
         <w:vAlign w:val=\"top\"/></w:tcPr>{cell}</w:tc></w:tr></w:tbl>"
    ));
    out
}

// ---- writer --------------------------------------------------------------

struct Writer<'a> {
    doc: &'a ExportDoc,
    opts: &'a DocxOptions,
    /// `word/_rels/document.xml.rels`.
    rels: Vec<Rel>,
    /// `word/_rels/footnotes.xml.rels`; empty unless the notes part uses a
    /// picture or an external link, in which case the part is written too.
    footnote_rels: Vec<Rel>,
    /// Relationship deduplication keys, scoped by part.
    rel_keys: HashMap<String, String>,
    /// The part currently being rendered, which selects the relationship file.
    part: Part,
    numbers: Vec<NumberDef>,
    media: Vec<Media>,
    media_by_asset: Vec<Option<usize>>,
    next_doc_pr: u32,
    next_bookmark_id: u32,
    bookmarks: HashMap<String, String>,
    /// The bookmark around the first reference to each footnote id, so the
    /// note's backlink can jump there.
    footnote_anchors: HashMap<usize, String>,
    toc_label: String,
    labels: ExportLabels,
    content_width: u32,
}

impl<'a> Writer<'a> {
    fn new(doc: &'a ExportDoc, opts: &'a DocxOptions) -> Self {
        let locale = lmdv_i18n::negotiate(std::iter::once(doc.lang.as_str()));
        let width = opts.page.width_twips.max(1_440);
        let height = opts.page.height_twips.max(1_440);
        let margin = opts.page.margin_twips.min(width / 3).min(height / 3);
        let mut rels = vec![
            Rel { id: "rId1".to_owned(), kind: REL_STYLES, target: "styles.xml".to_owned(), external: false },
            Rel { id: "rId2".to_owned(), kind: REL_NUMBERING, target: "numbering.xml".to_owned(), external: false },
            Rel { id: "rId3".to_owned(), kind: REL_SETTINGS, target: "settings.xml".to_owned(), external: false },
        ];
        if !doc.footnotes.is_empty() {
            rels.push(Rel {
                id: "rId4".to_owned(),
                kind: REL_FOOTNOTES,
                target: "footnotes.xml".to_owned(),
                external: false,
            });
        }
        Self {
            doc,
            opts,
            rels,
            footnote_rels: Vec::new(),
            rel_keys: HashMap::new(),
            part: Part::Document,
            numbers: Vec::new(),
            media: Vec::new(),
            media_by_asset: vec![None; doc.assets.len()],
            next_doc_pr: 1,
            next_bookmark_id: 1,
            bookmarks: collect_bookmarks(doc),
            footnote_anchors: HashMap::new(),
            toc_label: text_in(locale, TOC_LABEL).to_owned(),
            labels: ExportLabels::for_lang(&doc.lang),
            content_width: width.saturating_sub(margin.saturating_mul(2)).max(1_440),
        }
    }

    fn body_size(&self) -> u32 {
        self.opts.body_size_half_points.clamp(2, 400)
    }

    fn code_size(&self) -> u32 {
        (self.body_size() * 7 / 8).clamp(2, 400)
    }

    // ---- document -------------------------------------------------------

    fn title(&mut self, out: &mut String) {
        let title = self.doc.title.trim();
        if title.is_empty() || self.title_matches_first_heading(title) {
            return;
        }
        let text = title.to_owned();
        let mut content = String::new();
        self.text_run(&mut content, &text, RunStyle::default(), false, false);
        out.push_str(&paragraph(Ppr { style: Some("Title"), ..Ppr::default() }, &content));
    }

    /// Whether the body already starts with an h1 carrying the document title;
    /// exporting would then show the same line twice.
    fn title_matches_first_heading(&self, title: &str) -> bool {
        let Some(ExportBlock::Heading { level: 1, runs, .. }) = self.doc.blocks.first() else {
            return false;
        };
        let text = plain_text(runs);
        text.trim().eq_ignore_ascii_case(title)
    }

    fn blocks(&mut self, out: &mut String, blocks: &[ExportBlock], ctx: Ctx) {
        let mut previous_table = false;
        for block in blocks {
            let mut rendered = String::new();
            self.block(&mut rendered, block, ctx);
            if rendered.is_empty() {
                continue;
            }
            let is_table = rendered.starts_with("<w:tbl>") && rendered.ends_with("</w:tbl>");
            if previous_table && is_table {
                out.push_str(TABLE_SEPARATOR);
            }
            out.push_str(&rendered);
            previous_table = is_table;
        }
    }

    fn block(&mut self, out: &mut String, block: &ExportBlock, ctx: Ctx) {
        match block {
            ExportBlock::Heading { level, id, runs } => self.heading(out, *level, id, runs, ctx),
            ExportBlock::Para(runs) => self.paragraph_runs(out, runs, ctx),
            ExportBlock::List(list) => self.list(out, list, ctx),
            ExportBlock::Quote { callout: None, blocks } => {
                let inner = ctx.quoted();
                self.blocks(out, blocks, inner);
            }
            ExportBlock::Quote { callout: Some(callout), blocks } => {
                self.callout(out, callout, blocks, ctx);
            }
            ExportBlock::Code { lang, text } => self.code(out, lang.as_deref(), text, ctx),
            ExportBlock::Table(table) => self.table(out, table, ctx),
            ExportBlock::Math { tex, asset, .. } => self.math_block(out, tex, *asset, ctx),
            ExportBlock::Image(image) => self.image_block(out, image, ctx),
            ExportBlock::Diagram { source, asset } => self.diagram(out, source, *asset, ctx),
            ExportBlock::Rule => out.push_str(&paragraph(
                Ppr { border: Some(RULE_BORDER), before: Some(120), after: Some(240), ..Ppr::default() },
                "",
            )),
            ExportBlock::Toc { entries } => self.toc(out, entries, ctx),
            // Footnotes live in `word/footnotes.xml`; the marker only says where
            // the generated list would go in other formats.
            ExportBlock::Footnotes => {}
            ExportBlock::Html(node) => self.html_block(out, node, ctx),
        }
    }

    fn heading(&mut self, out: &mut String, level: u8, id: &str, runs: &[ExportRun], ctx: Ctx) {
        let level = level.clamp(1, 6);
        let mut content = String::new();
        let bookmark = self.bookmarks.get(id).cloned();
        let bookmark_id = bookmark.as_ref().map(|_| {
            let id = self.next_bookmark_id;
            self.next_bookmark_id += 1;
            id
        });
        if let (Some(name), Some(id)) = (&bookmark, bookmark_id) {
            content.push_str(&format!("<w:bookmarkStart w:id=\"{id}\" w:name=\""));
            escape_attr(&mut content, name);
            content.push_str("\"/>");
        }
        self.runs(&mut content, runs, ctx);
        if let Some(id) = bookmark_id {
            content.push_str(&format!("<w:bookmarkEnd w:id=\"{id}\"/>"));
        }
        out.push_str(&paragraph(Ppr { style: Some(heading_style(level)), ..Ppr::default() }, &content));
    }

    fn paragraph_runs(&mut self, out: &mut String, runs: &[ExportRun], ctx: Ctx) {
        if runs.is_empty() {
            return;
        }
        let mut content = String::new();
        self.runs(&mut content, runs, ctx);
        if content.is_empty() {
            return;
        }
        out.push_str(&paragraph(body_ppr(ctx), &content));
    }

    // ---- lists ----------------------------------------------------------

    fn list(&mut self, out: &mut String, list: &ExportList, ctx: Ctx) {
        let level = ctx.depth.min(8);
        let num_id = self.new_number(list.ordered, list.start, level);
        let indent = ctx.base + u32::from(level + 1) * INDENT;
        let after = if list.tight { 20 } else { 120 };
        for item in &list.items {
            let body_start = match item.blocks.first() {
                Some(ExportBlock::Para(runs)) => {
                    self.list_para(out, runs, item, level, indent, num_id, after, ctx);
                    1
                }
                _ => {
                    // The marker must exist even when the item starts with a
                    // code block, table or nested list.
                    self.list_para(out, &[], item, level, indent, num_id, after, ctx);
                    0
                }
            };
            if body_start < item.blocks.len() {
                let mut rendered = String::new();
                self.blocks(&mut rendered, &item.blocks[body_start..], ctx.item());
                out.push_str(&rendered);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn list_para(
        &mut self,
        out: &mut String,
        runs: &[ExportRun],
        item: &crate::model::ExportItem,
        level: u8,
        indent: u32,
        num_id: u32,
        after: u32,
        ctx: Ctx,
    ) {
        let mut content = String::new();
        if let Some(checked) = item.task {
            let glyph = if checked { "\u{2611}" } else { "\u{2610}" };
            self.text_run(&mut content, glyph, RunStyle::default(), false, false);
            self.text_run(&mut content, " ", RunStyle::default(), false, false);
        }
        self.runs(&mut content, runs, ctx);
        let ppr = Ppr {
            style: Some(if ctx.quote { "QuoteListParagraph" } else { "ListParagraph" }),
            numbering: item.task.is_none().then_some((level, num_id)),
            border: ctx.quote.then_some(QUOTE_BORDER),
            after: Some(after),
            indent: Some(indent),
            hanging: item.task.is_none().then_some(360),
            color: ctx.quote.then_some(MUTED_COLOR),
            ..Ppr::default()
        };
        out.push_str(&paragraph(ppr, &content));
    }

    fn new_number(&mut self, ordered: bool, start: u64, level: u8) -> u32 {
        let id = self.numbers.len() as u32 + 1;
        let abstract_id = if ordered { 1 } else { 0 };
        let start = (ordered && start != 1).then_some(start.max(1));
        self.numbers.push(NumberDef { id, abstract_id, level, start });
        id
    }

    // ---- quotes and callouts --------------------------------------------

    fn callout(&mut self, out: &mut String, callout: &ExportCallout, blocks: &[ExportBlock], ctx: Ctx) {
        let color = callout_color(callout.kind);
        let fill = callout_fill(callout.kind);
        let title = callout.title.clone().unwrap_or_else(|| self.labels.callout(callout.kind).to_owned());
        let mut cell = String::new();
        let mut heading = String::new();
        self.styled_run(
            &mut heading,
            &format!("{} {title}", callout_symbol(callout.kind)),
            &format!("<w:b/><w:color w:val=\"{color}\"/>"),
            false,
        );
        cell.push_str(&paragraph(Ppr { keep_next: true, after: Some(60), ..Ppr::default() }, &heading));
        let inner = Ctx { in_note: ctx.in_note, ..Ctx::default() };
        self.blocks(&mut cell, blocks, inner);
        if cell.ends_with("</w:tbl>") {
            cell.push_str(TABLE_SEPARATOR);
        }
        let width = self.content_width.saturating_sub(ctx.indent()).max(1_440);
        let borders = format!(
            "<w:tblBorders><w:top w:val=\"nil\"/><w:left w:val=\"single\" w:sz=\"24\" w:space=\"0\" \
             w:color=\"{color}\"/><w:bottom w:val=\"nil\"/><w:right w:val=\"nil\"/>\
             <w:insideH w:val=\"nil\"/><w:insideV w:val=\"nil\"/></w:tblBorders>"
        );
        out.push_str(&one_cell_table(width, ctx.indent(), &borders, fill, &cell));
    }

    // ---- code -----------------------------------------------------------

    fn code(&mut self, out: &mut String, lang: Option<&str>, text: &str, ctx: Ctx) {
        let mut cell = String::new();
        if let Some(lang) = lang.map(str::trim).filter(|lang| !lang.is_empty()) {
            let mut content = String::new();
            self.styled_run(
                &mut content,
                lang,
                &format!(
                    "<w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\" w:cs=\"{0}\"/>\
                     <w:color w:val=\"6E6E69\"/><w:sz w:val=\"16\"/><w:szCs w:val=\"16\"/>",
                    escape_attr_to_string(&self.opts.code_font)
                ),
                false,
            );
            cell.push_str(&paragraph(
                Ppr { style: Some("CodeBlock"), after: Some(40), line: Some(240), ..Ppr::default() },
                &content,
            ));
        }
        let mut lines: Vec<&str> = text.split('\n').collect();
        if lines.len() > 1 && lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push("");
        }
        for line in lines {
            let mut content = String::new();
            if !line.is_empty() {
                self.code_run(&mut content, line);
            }
            cell.push_str(&paragraph(
                Ppr { style: Some("CodeBlock"), after: Some(0), line: Some(240), ..Ppr::default() },
                &content,
            ));
        }
        let width = self.content_width.saturating_sub(ctx.indent()).max(1_440);
        out.push_str(&one_cell_table(width, ctx.indent(), GRID_BORDERS, CODE_FILL, &cell));
    }

    fn code_run(&self, out: &mut String, line: &str) {
        out.push_str("<w:r><w:rPr><w:rFonts w:ascii=\"");
        escape_attr(out, &self.opts.code_font);
        out.push_str("\" w:hAnsi=\"");
        escape_attr(out, &self.opts.code_font);
        out.push_str("\" w:cs=\"");
        escape_attr(out, &self.opts.code_font);
        out.push_str(&format!(
            "\"/><w:sz w:val=\"{}\"/><w:szCs w:val=\"{}\"/></w:rPr>",
            self.code_size(),
            self.code_size()
        ));
        write_run_content(out, line);
        out.push_str("</w:r>");
    }

    // ---- tables ---------------------------------------------------------

    fn table(&mut self, out: &mut String, table: &ExportTable, ctx: Ctx) {
        if table.head.is_empty() && table.rows.is_empty() {
            return;
        }
        let mut rows = Vec::new();
        if !table.head.is_empty() {
            let mut cells = Vec::with_capacity(table.head.len());
            for (column, runs) in table.head.iter().enumerate() {
                let mut content = String::new();
                self.runs_bold(&mut content, runs, ctx, true);
                cells.push(TableCell {
                    header: true,
                    paragraphs: paragraph(
                        Ppr {
                            after: Some(0),
                            jc: align_name(table.align.get(column).copied().unwrap_or_default()),
                            ..Ppr::default()
                        },
                        &content,
                    ),
                });
            }
            rows.push(TableRow { header: true, cells });
        }
        for row in &table.rows {
            let mut cells = Vec::with_capacity(row.len());
            for (column, runs) in row.iter().enumerate() {
                let mut content = String::new();
                self.runs(&mut content, runs, ctx);
                cells.push(TableCell {
                    header: false,
                    paragraphs: paragraph(
                        Ppr {
                            after: Some(0),
                            jc: align_name(table.align.get(column).copied().unwrap_or_default()),
                            ..Ppr::default()
                        },
                        &content,
                    ),
                });
            }
            rows.push(TableRow { header: false, cells });
        }
        let width = self.content_width.saturating_sub(ctx.indent()).max(1_440);
        out.push_str(&table_xml(width, ctx.indent(), GRID_BORDERS, GRID_LOOK, &rows, true));
    }

    // ---- pictures and formulas ------------------------------------------

    fn image_block(&mut self, out: &mut String, image: &ExportImage, ctx: Ctx) {
        let alt = if !image.alt.is_empty() {
            image.alt.clone()
        } else if !image.src.is_empty() {
            image.src.clone()
        } else {
            self.labels.image.clone()
        };
        let mut content = String::new();
        let shown =
            image.asset.is_some_and(|asset| self.image_drawing(&mut content, asset, &alt, image.title.as_deref()));
        if shown {
            out.push_str(&paragraph(body_ppr(ctx).centered(), &content));
            if let Some(title) = &image.title {
                let mut caption = String::new();
                self.styled_run(
                    &mut caption,
                    title,
                    &format!("<w:i/><w:color w:val=\"{MUTED_COLOR}\"/><w:sz w:val=\"18\"/><w:szCs w:val=\"18\"/>"),
                    false,
                );
                out.push_str(&paragraph(Ppr { after: Some(160), jc: Some("center"), ..Ppr::default() }, &caption));
            }
        } else {
            self.missing_image(out, &alt, ctx);
        }
    }

    fn missing_image(&mut self, out: &mut String, alt: &str, ctx: Ctx) {
        let mut content = String::new();
        self.styled_run(&mut content, alt, &format!("<w:i/><w:color w:val=\"{MUTED_COLOR}\"/>"), false);
        out.push_str(&paragraph(body_ppr(ctx), &content));
    }

    fn math_block(&mut self, out: &mut String, tex: &str, asset: Option<AssetId>, ctx: Ctx) {
        let mut content = String::new();
        let shown = asset.is_some_and(|asset| self.image_drawing(&mut content, asset, tex, Some(tex)));
        if shown {
            out.push_str(&paragraph(body_ppr(ctx).centered(), &content));
            return;
        }
        let mut content = String::new();
        self.tex_run(&mut content, tex);
        out.push_str(&paragraph(
            Ppr { after: Some(160), jc: Some("center"), indent: Some(ctx.indent()), ..Ppr::default() },
            &content,
        ));
    }

    fn diagram(&mut self, out: &mut String, source: &str, asset: Option<AssetId>, ctx: Ctx) {
        let mut content = String::new();
        let alt = self.labels.mermaid.clone();
        let shown = asset.is_some_and(|asset| self.image_drawing(&mut content, asset, &alt, None));
        if shown {
            out.push_str(&paragraph(body_ppr(ctx).centered(), &content));
        } else {
            self.code(out, Some("mermaid"), source, ctx);
        }
    }

    /// Emits one `wp:inline` drawing run. Returns `false` when the asset cannot
    /// be embedded (a missing asset, an empty file, AVIF, …).
    fn image_drawing(&mut self, out: &mut String, asset: AssetId, alt: &str, title: Option<&str>) -> bool {
        let part = self.part;
        let Some(media) = self.media_for(asset) else { return false };
        let Some(source) = self.doc.asset(asset) else { return false };
        let (cx, cy) = image_extent(source, self.media.get(media).and_then(|media| media.size));
        let rel_id = self.media_rel(media, part);
        let id = self.next_doc_pr;
        self.next_doc_pr += 1;
        let name = format!("Picture {id}");
        out.push_str("<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">");
        out.push_str(&format!("<wp:extent cx=\"{cx}\" cy=\"{cy}\"/>"));
        out.push_str("<wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>");
        out.push_str(&format!("<wp:docPr id=\"{id}\" name=\""));
        escape_attr(out, &name);
        out.push_str("\" descr=\"");
        escape_attr(out, alt);
        out.push('"');
        if let Some(title) = title.filter(|title| !title.is_empty()) {
            out.push_str(" title=\"");
            escape_attr(out, title);
            out.push('"');
        }
        out.push_str("/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect=\"1\"/>");
        out.push_str("</wp:cNvGraphicFramePr><a:graphic><a:graphicData uri=\"");
        out.push_str(PIC_NS);
        out.push_str("\"><pic:pic><pic:nvPicPr><pic:cNvPr id=\"");
        out.push_str(&id.to_string());
        out.push_str("\" name=\"");
        escape_attr(out, &name);
        out.push_str("\" descr=\"");
        escape_attr(out, alt);
        out.push_str("\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"");
        out.push_str(&rel_id);
        out.push_str("\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr>");
        out.push_str(&format!("<a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>"));
        out.push_str("<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>");
        out.push_str("</a:graphicData></a:graphic></wp:inline></w:drawing></w:r>");
        true
    }

    /// Registers an asset as a media part and returns its index.
    ///
    /// An SVG is rasterized to PNG here (see [`raster`]); Word's plain
    /// `a:blip` has no portable SVG support. AVIF has no decoder in the
    /// exporter's dependency tree, so it still falls back to alt text.
    fn media_for(&mut self, asset: AssetId) -> Option<usize> {
        let index = asset.index();
        if let Some(slot) = self.media_by_asset.get(index).copied().flatten() {
            return Some(slot);
        }
        let (extension, raster, size) = {
            let source = self.doc.asset(asset)?;
            if source.bytes.is_empty() {
                return None;
            }
            if source.mime == "image/svg+xml" {
                let (png, (width, height)) = raster::svg_to_png(&source.bytes)?;
                ("png", Some(png), Some((u64::from(width), u64::from(height))))
            } else {
                (media_extension(source.mime)?, None, None)
            }
        };
        let file_name = format!("image{}.{}", self.media.len() + 1, extension);
        let media = Media { file_name, asset, raster, size, document_rel: None, footnote_rel: None };
        self.media.push(media);
        let media_index = self.media.len() - 1;
        if let Some(slot) = self.media_by_asset.get_mut(index) {
            *slot = Some(media_index);
        }
        Some(media_index)
    }

    /// The relationship id of `media` in `part`, adding the relationship on
    /// first use. Each part has its own id sequence and its own `.rels` file.
    fn media_rel(&mut self, index: usize, part: Part) -> String {
        if let Some(id) = self.media.get(index).and_then(|media| media.rel(part)) {
            return id.clone();
        }
        let id = self.next_rel_id(part);
        let target = format!("media/{}", self.media[index].file_name);
        self.part_rels_mut(part).push(Rel { id: id.clone(), kind: REL_IMAGE, target, external: false });
        if let Some(media) = self.media.get_mut(index) {
            media.set_rel(part, id.clone());
        }
        id
    }

    // ---- toc and footnotes ----------------------------------------------

    fn toc(&mut self, out: &mut String, entries: &[ExportTocEntry], ctx: Ctx) {
        if entries.is_empty() {
            return;
        }
        let mut heading = String::new();
        let label = self.toc_label.clone();
        self.styled_run(&mut heading, &label, "<w:b/>", false);
        out.push_str(&paragraph(
            Ppr { style: Some("TOCHeading"), keep_next: true, after: Some(120), ..Ppr::default() },
            &heading,
        ));
        // The entry paragraphs are the field's cached result: a reader that
        // does not update fields still sees a usable, linked table of contents.
        // Word rebuilds it (with page numbers) on open because `settings.xml`
        // sets `w:updateFields`, or on F9.
        let mut paragraphs = Vec::new();
        self.toc_level(&mut paragraphs, entries, ctx);
        let Some(first) = paragraphs.first_mut() else { return };
        insert_after_paragraph_properties(first, TOC_FIELD_START);
        if let Some(last) = paragraphs.last_mut() {
            insert_before_paragraph_end(last, TOC_FIELD_END);
        }
        for paragraph in paragraphs {
            out.push_str(&paragraph);
        }
    }

    fn toc_level(&mut self, out: &mut Vec<String>, entries: &[ExportTocEntry], ctx: Ctx) {
        for entry in entries {
            let mut content = String::new();
            let opened = self.bookmarks.get(&entry.id).cloned().is_some_and(|anchor| {
                content.push_str("<w:hyperlink w:anchor=\"");
                escape_attr(&mut content, &anchor);
                content.push_str("\" w:history=\"1\">");
                true
            });
            let text = entry.text.clone();
            self.text_run(&mut content, &text, RunStyle::default(), false, false);
            if opened {
                content.push_str("</w:hyperlink>");
            }
            let level = entry.level.clamp(1, 6);
            out.push(paragraph(
                Ppr {
                    style: Some(toc_style(level)),
                    after: Some(20),
                    indent: Some(ctx.indent() + 220 * (u32::from(level) - 1)),
                    ..Ppr::default()
                },
                &content,
            ));
            self.toc_level(out, &entry.children, ctx);
        }
    }

    // ---- footnotes -------------------------------------------------------

    /// The `word/footnotes.xml` part: the two Word pseudo-notes (separator and
    /// continuation separator, ids -1 and 0) followed by one `w:footnote` per
    /// referenced definition, in the model's first-reference order.
    ///
    /// The part is its own OPC part, so its pictures and external links use
    /// [`Writer::footnote_rels`] rather than the document's relationships.
    fn footnotes_xml(&mut self) -> String {
        let mut xml = String::from(XML_DECLARATION);
        xml.push_str(&format!(
            "<w:footnotes xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\" xmlns:wp=\"{WP_NS}\" \
             xmlns:a=\"{A_NS}\" xmlns:pic=\"{PIC_NS}\">"
        ));
        xml.push_str(FOOTNOTE_SEPARATOR);
        xml.push_str(FOOTNOTE_CONTINUATION);
        let doc = self.doc;
        self.part = Part::Footnotes;
        for (index, footnote) in doc.footnotes.iter().enumerate() {
            let id = index + 1;
            let mut body = self.footnote_body(footnote);
            // A trailing text/list/quote/html block can carry the backlink on
            // its last line; a table, code block or picture gets its own
            // footnote-text paragraph instead.
            let inline = matches!(
                footnote.blocks.last(),
                None | Some(
                    ExportBlock::Para(_) | ExportBlock::List(_) | ExportBlock::Quote { .. } | ExportBlock::Html(_)
                )
            );
            self.footnote_backlink(&mut body, id, footnote.number, inline);
            xml.push_str(&format!("<w:footnote w:id=\"{id}\">{body}</w:footnote>"));
        }
        self.part = Part::Document;
        xml.push_str("</w:footnotes>\n");
        xml
    }

    /// One note's content: the `w:footnoteRef` number marker, the definition's
    /// first paragraph on the same line, then its remaining blocks (paragraphs,
    /// lists, code, pictures and formulas render exactly as in the body).
    fn footnote_body(&mut self, footnote: &ExportFootnote) -> String {
        let ctx = Ctx { in_note: true, ..Ctx::default() };
        let mut content = String::new();
        content.push_str("<w:r><w:rPr><w:rStyle w:val=\"");
        content.push_str(FOOTNOTE_REFERENCE_STYLE);
        content.push_str("\"/><w:vertAlign w:val=\"superscript\"/></w:rPr><w:footnoteRef/></w:r>");
        // A space run separates the marker from the text (Word's own layout).
        content.push_str("<w:r><w:t xml:space=\"preserve\"> </w:t></w:r>");
        let mut rest = 0;
        if let Some(ExportBlock::Para(runs)) = footnote.blocks.first() {
            self.runs(&mut content, runs, ctx);
            rest = 1;
        }
        let mut out = paragraph(Ppr { style: Some(FOOTNOTE_TEXT_STYLE), ..Ppr::default() }, &content);
        self.blocks(&mut out, &footnote.blocks[rest..], ctx);
        out
    }

    /// Appends the localized backlink to the note's last paragraph (or its own
    /// paragraph when the note ends with a table). The link targets the
    /// bookmark [`Writer::footnote_reference`] put around the first reference.
    fn footnote_backlink(&mut self, out: &mut String, id: usize, number: usize, inline: bool) {
        let Some(anchor) = self.footnote_anchors.get(&id).cloned() else { return };
        let label = lmdv_i18n::render(&self.labels.footnote_back, &[("n", &number)]);
        let mut link = String::new();
        link.push_str("<w:hyperlink w:anchor=\"");
        escape_attr(&mut link, &anchor);
        link.push_str("\" w:tooltip=\"");
        escape_attr(&mut link, &label);
        link.push_str("\" w:history=\"1\">");
        self.styled_run(&mut link, " \u{21A9}", &format!("<w:color w:val=\"{MUTED_COLOR}\"/>"), false);
        link.push_str("</w:hyperlink>");
        if inline && out.ends_with("</w:p>") {
            let cut = out.len() - "</w:p>".len();
            out.insert_str(cut, &link);
        } else {
            out.push_str(&paragraph(Ppr { style: Some(FOOTNOTE_TEXT_STYLE), ..Ppr::default() }, &link));
        }
    }

    // ---- HTML subset -----------------------------------------------------

    fn html_block(&mut self, out: &mut String, node: &HtmlNode, ctx: Ctx) {
        match node.tag {
            HtmlTag::Fragment | HtmlTag::Div | HtmlTag::Tbody | HtmlTag::Thead | HtmlTag::Tfoot => {
                self.html_children_blocks(out, node, ctx);
            }
            HtmlTag::P => {
                let mut content = String::new();
                self.html_children_inline(&mut content, node, HtmlFmt::default(), ctx);
                if !content.is_empty() {
                    out.push_str(&paragraph(body_ppr(ctx), &content));
                }
            }
            HtmlTag::Summary => {
                let mut content = String::new();
                self.html_children_inline(&mut content, node, HtmlFmt { bold: true, ..HtmlFmt::default() }, ctx);
                if !content.is_empty() {
                    out.push_str(&paragraph(body_ppr(ctx), &content));
                }
            }
            // Word has no collapsible card in the plain OOXML subset: the
            // summary becomes a bold line and the body follows as ordinary
            // blocks (the details are kept, the toggle is lost).
            HtmlTag::Details => {
                let summary = node.children.iter().find_map(|child| match child {
                    HtmlChild::Node(child) if child.tag == HtmlTag::Summary => Some(child.text()),
                    _ => None,
                });
                if let Some(summary) = summary.map(|summary| summary.trim().to_owned()).filter(|s| !s.is_empty()) {
                    let mut content = String::new();
                    self.styled_run(&mut content, &summary, "<w:b/>", false);
                    out.push_str(&paragraph(body_ppr(ctx), &content));
                }
                for child in &node.children {
                    match child {
                        HtmlChild::Node(child) if child.tag == HtmlTag::Summary => {}
                        HtmlChild::Node(child) => self.html_block(out, child, ctx),
                        HtmlChild::Text(text) => {
                            let text = text.trim();
                            if !text.is_empty() {
                                let mut content = String::new();
                                self.text_run(&mut content, text, RunStyle::default(), false, false);
                                out.push_str(&paragraph(body_ppr(ctx), &content));
                            }
                        }
                    }
                }
            }
            HtmlTag::Blockquote => {
                let inner = ctx.quoted();
                self.html_children_blocks(out, node, inner);
            }
            HtmlTag::Pre => {
                let text = node.text();
                let text = text.strip_suffix('\n').unwrap_or(&text);
                self.code(out, None, text, ctx);
            }
            HtmlTag::Table => self.html_table(out, node, ctx),
            HtmlTag::Ul | HtmlTag::Ol => {
                for child in &node.children {
                    if let HtmlChild::Node(child) = child
                        && child.tag == HtmlTag::Li
                    {
                        self.html_list_item(out, child, node.tag == HtmlTag::Ol, ctx);
                    }
                }
            }
            HtmlTag::Li => self.html_list_item(out, node, false, ctx),
            HtmlTag::Img | HtmlTag::Picture => {
                let mut content = String::new();
                self.html_inline(&mut content, node, HtmlFmt::default(), ctx);
                if !content.is_empty() {
                    out.push_str(&paragraph(body_ppr(ctx).centered(), &content));
                }
            }
            HtmlTag::Video | HtmlTag::Audio => {
                let mut content = String::new();
                self.html_media_run(&mut content, node);
                if !content.is_empty() {
                    out.push_str(&paragraph(body_ppr(ctx), &content));
                }
            }
            _ => {
                let mut content = String::new();
                self.html_inline(&mut content, node, HtmlFmt::default(), ctx);
                if !content.is_empty() {
                    out.push_str(&paragraph(body_ppr(ctx), &content));
                }
            }
        }
    }

    fn html_children_blocks(&mut self, out: &mut String, node: &HtmlNode, ctx: Ctx) {
        for child in &node.children {
            match child {
                HtmlChild::Text(text) => {
                    let text = text.trim();
                    if !text.is_empty() {
                        let mut content = String::new();
                        self.text_run(&mut content, text, RunStyle::default(), false, false);
                        out.push_str(&paragraph(body_ppr(ctx), &content));
                    }
                }
                HtmlChild::Node(child) => self.html_block(out, child, ctx),
            }
        }
    }

    fn html_list_item(&mut self, out: &mut String, node: &HtmlNode, ordered: bool, ctx: Ctx) {
        let mut content = String::new();
        self.styled_run(&mut content, if ordered { "1. " } else { "\u{2022} " }, "", false);
        self.html_children_inline(&mut content, node, HtmlFmt::default(), ctx);
        out.push_str(&paragraph(Ppr { indent: Some(ctx.indent() + 360), ..body_ppr(ctx) }, &content));
    }

    fn html_table(&mut self, out: &mut String, node: &HtmlNode, ctx: Ctx) {
        let mut rows = Vec::new();
        let mut source = Vec::new();
        collect_html_rows(node, false, &mut source);
        for (header, row) in source {
            let mut cells = Vec::new();
            for child in &row.children {
                let HtmlChild::Node(cell) = child else { continue };
                if !matches!(cell.tag, HtmlTag::Td | HtmlTag::Th) {
                    continue;
                }
                let align = match cell.attr("align").map(str::to_ascii_lowercase).as_deref() {
                    Some("center") => Align::Center,
                    Some("right") => Align::Right,
                    Some("left") => Align::Left,
                    _ => Align::None,
                };
                let cell_header = header || cell.tag == HtmlTag::Th;
                let mut content = String::new();
                let fmt = HtmlFmt { bold: cell_header, ..HtmlFmt::default() };
                self.html_children_inline(&mut content, cell, fmt, ctx);
                cells.push(TableCell {
                    header: cell_header,
                    paragraphs: paragraph(Ppr { after: Some(0), jc: align_name(align), ..Ppr::default() }, &content),
                });
            }
            if !cells.is_empty() {
                rows.push(TableRow { header, cells });
            }
        }
        if rows.is_empty() {
            let text = node.text();
            let text = text.trim();
            if !text.is_empty() {
                let mut content = String::new();
                self.text_run(&mut content, text, RunStyle::default(), false, false);
                out.push_str(&paragraph(body_ppr(ctx), &content));
            }
            return;
        }
        let width = self.content_width.saturating_sub(ctx.indent()).max(1_440);
        out.push_str(&table_xml(width, ctx.indent(), GRID_BORDERS, GRID_LOOK, &rows, true));
    }

    fn html_inline(&mut self, out: &mut String, node: &HtmlNode, fmt: HtmlFmt, ctx: Ctx) {
        match node.tag {
            HtmlTag::Img => self.html_img_run(out, node),
            HtmlTag::Picture => {
                if let Some(img) = node.children.iter().find_map(|child| match child {
                    HtmlChild::Node(child) if child.tag == HtmlTag::Img => Some(child),
                    _ => None,
                }) {
                    self.html_img_run(out, img);
                } else {
                    self.html_children_inline(out, node, fmt, ctx);
                }
            }
            HtmlTag::Video | HtmlTag::Audio => self.html_media_run(out, node),
            HtmlTag::Source | HtmlTag::Track => {}
            HtmlTag::Br => out.push_str("<w:r><w:br/></w:r>"),
            HtmlTag::A => {
                let opened = node.attr("href").is_some_and(|href| self.open_href(out, href));
                self.html_children_inline(out, node, fmt, ctx);
                if opened {
                    out.push_str("</w:hyperlink>");
                }
            }
            HtmlTag::Strong | HtmlTag::B => {
                self.html_children_inline(out, node, HtmlFmt { bold: true, ..fmt }, ctx);
            }
            HtmlTag::Em | HtmlTag::I => {
                self.html_children_inline(out, node, HtmlFmt { italic: true, ..fmt }, ctx);
            }
            HtmlTag::S | HtmlTag::Del => {
                self.html_children_inline(out, node, HtmlFmt { strike: true, ..fmt }, ctx);
            }
            HtmlTag::U | HtmlTag::Ins => {
                self.html_children_inline(out, node, HtmlFmt { underline: true, ..fmt }, ctx);
            }
            HtmlTag::Code | HtmlTag::Kbd => {
                self.html_children_inline(out, node, HtmlFmt { code: true, ..fmt }, ctx);
            }
            HtmlTag::Mark => {
                self.html_children_inline(out, node, HtmlFmt { mark: true, ..fmt }, ctx);
            }
            HtmlTag::Sub => {
                self.html_children_inline(out, node, HtmlFmt { sub: true, ..fmt }, ctx);
            }
            HtmlTag::Sup => {
                self.html_children_inline(out, node, HtmlFmt { sup: true, ..fmt }, ctx);
            }
            _ => self.html_children_inline(out, node, fmt, ctx),
        }
    }

    fn html_children_inline(&mut self, out: &mut String, node: &HtmlNode, fmt: HtmlFmt, ctx: Ctx) {
        for child in &node.children {
            match child {
                HtmlChild::Text(text) => {
                    if !text.is_empty() {
                        self.text_run(out, text, fmt.style(), false, fmt.bold);
                    }
                }
                HtmlChild::Node(child) => {
                    // A nested bold/italic wrapper keeps the outer flags.
                    let nested = merge_fmt(fmt, child.tag);
                    self.html_inline(out, child, nested, ctx);
                }
            }
        }
    }

    fn html_img_run(&mut self, out: &mut String, node: &HtmlNode) {
        let alt = node.attr("alt").unwrap_or_default();
        let Some(src) = node.attr("src").filter(|src| !src.is_empty()) else {
            if !alt.is_empty() {
                self.text_run(out, alt, RunStyle::default(), false, false);
            }
            return;
        };
        if let Some(id) = src.strip_prefix("lmdv-asset:").and_then(|id| id.parse::<u32>().ok())
            && self.image_drawing(out, AssetId(id), alt, node.attr("title"))
        {
            return;
        }
        let fallback = if !alt.is_empty() { alt } else { src };
        self.text_run(out, fallback, RunStyle::default(), false, false);
    }

    fn html_media_run(&mut self, out: &mut String, node: &HtmlNode) {
        let tag = node.tag.name();
        let src = node.attr("src").or_else(|| node.attr("data-lmdv-remote"));
        let Some(src) = src.filter(|src| !src.is_empty()) else {
            self.text_run(out, tag, RunStyle::default(), false, false);
            return;
        };
        let label = file_name(src).unwrap_or_else(|| tag.to_owned());
        let opened = self.open_href(out, src);
        self.text_run(out, &label, RunStyle::default(), opened, false);
        if opened {
            out.push_str("</w:hyperlink>");
        }
    }

    // ---- inline ----------------------------------------------------------

    fn runs(&mut self, out: &mut String, runs: &[ExportRun], ctx: Ctx) {
        self.runs_bold(out, runs, ctx, false);
    }

    fn runs_bold(&mut self, out: &mut String, runs: &[ExportRun], ctx: Ctx, bold: bool) {
        for run in runs {
            self.run(out, run, ctx, bold);
        }
    }

    fn run(&mut self, out: &mut String, run: &ExportRun, ctx: Ctx, bold: bool) {
        match run {
            ExportRun::Text { text, style, link } => {
                if text.is_empty() {
                    return;
                }
                let opened = self.link_open(out, link.as_ref());
                self.text_run(out, text, *style, opened, bold);
                self.link_close(out, opened);
            }
            ExportRun::Math { tex, asset, link, .. } => {
                let opened = self.link_open(out, link.as_ref());
                let mut content = String::new();
                let shown = asset.is_some_and(|asset| self.image_drawing(&mut content, asset, tex, Some(tex)));
                if shown {
                    out.push_str(&content);
                } else {
                    self.tex_run(out, tex);
                }
                self.link_close(out, opened);
            }
            ExportRun::Image { image, link } => {
                let opened = self.link_open(out, link.as_ref());
                let alt = if !image.alt.is_empty() {
                    image.alt.clone()
                } else if !image.src.is_empty() {
                    image.src.clone()
                } else {
                    self.labels.image.clone()
                };
                let shown =
                    image.asset.is_some_and(|asset| self.image_drawing(out, asset, &alt, image.title.as_deref()));
                if !shown {
                    self.text_run(out, &alt, RunStyle::default(), opened, false);
                }
                self.link_close(out, opened);
            }
            ExportRun::Break => out.push_str("<w:r><w:br/></w:r>"),
            ExportRun::FootnoteRef { label, number, occurrence } => {
                self.footnote_reference(out, label, *number, *occurrence, ctx);
            }
            ExportRun::Html(node) => self.html_inline(out, node, HtmlFmt::default(), ctx),
        }
    }

    /// A footnote reference in the body: a `w:footnoteReference` run pointing at
    /// the note's id, wrapped in a bookmark on the first reference so the note's
    /// backlink has a target.
    ///
    /// Inside `word/footnotes.xml` (a note referencing another note) the number
    /// stays visible superscript text: Word has no nested footnotes.
    fn footnote_reference(&mut self, out: &mut String, label: &str, number: usize, occurrence: usize, ctx: Ctx) {
        if ctx.in_note {
            self.styled_run(out, &number.to_string(), "<w:vertAlign w:val=\"superscript\"/>", false);
            return;
        }
        let Some(id) = self.footnote_id(label, number) else {
            // A hand-built model can reference a definition it does not carry;
            // keep the visible number instead of emitting a dangling id.
            self.styled_run(out, &number.to_string(), "<w:vertAlign w:val=\"superscript\"/>", false);
            return;
        };
        let first = occurrence == 0;
        let anchor = if first {
            Some(self.footnote_anchors.entry(id).or_insert_with(|| format!("_lmdvfn{id}")).clone())
        } else {
            self.footnote_anchors.get(&id).cloned()
        };
        let bookmark_id = first.then(|| {
            let bookmark_id = self.next_bookmark_id;
            self.next_bookmark_id += 1;
            bookmark_id
        });
        if let (Some(bookmark_id), Some(name)) = (bookmark_id, anchor.as_ref()) {
            out.push_str(&format!("<w:bookmarkStart w:id=\"{bookmark_id}\" w:name=\""));
            escape_attr(out, name);
            out.push_str("\"/>");
        }
        out.push_str("<w:r><w:rPr><w:rStyle w:val=\"");
        out.push_str(FOOTNOTE_REFERENCE_STYLE);
        out.push_str("\"/><w:vertAlign w:val=\"superscript\"/></w:rPr><w:footnoteReference w:id=\"");
        out.push_str(&id.to_string());
        out.push_str("\"/></w:r>");
        if let Some(bookmark_id) = bookmark_id {
            out.push_str(&format!("<w:bookmarkEnd w:id=\"{bookmark_id}\"/>"));
        }
    }

    /// The 1-based `w:id` of a footnote, by its position in
    /// [`ExportDoc::footnotes`]. The model numbers definitions by first
    /// reference, so id and number agree; deriving the id from the position
    /// still keeps the part valid for a hand-built model with repeated or
    /// non-sequential numbers.
    fn footnote_id(&self, label: &str, number: usize) -> Option<usize> {
        self.doc
            .footnotes
            .iter()
            .position(|footnote| footnote.label == label)
            .or_else(|| self.doc.footnotes.iter().position(|footnote| footnote.number == number))
            .map(|index| index + 1)
    }

    fn link_open(&mut self, out: &mut String, link: Option<&ExportLink>) -> bool {
        match link {
            Some(link) => self.open_href(out, &link.href),
            None => false,
        }
    }

    fn link_close(&self, out: &mut String, opened: bool) {
        if opened {
            out.push_str("</w:hyperlink>");
        }
    }

    /// Opens a hyperlink: an internal bookmark (`w:anchor`) or an external
    /// relationship (`r:id`). Returns whether a tag was opened.
    fn open_href(&mut self, out: &mut String, href: &str) -> bool {
        if let Some(fragment) = href.strip_prefix('#') {
            let Some(anchor) = self.bookmarks.get(fragment).cloned() else {
                return false;
            };
            out.push_str("<w:hyperlink w:anchor=\"");
            escape_attr(out, &anchor);
            out.push_str("\" w:history=\"1\">");
            return true;
        }
        if !href_is_safe(href) {
            return false;
        }
        let rel_id = self.rel_for_link(href, self.part);
        out.push_str("<w:hyperlink r:id=\"");
        out.push_str(&rel_id);
        out.push_str("\" w:history=\"1\">");
        true
    }

    fn rel_for_link(&mut self, href: &str, part: Part) -> String {
        let key = format!("{}|L|{href}", part.prefix());
        if let Some(id) = self.rel_keys.get(&key) {
            return id.clone();
        }
        let id = self.next_rel_id(part);
        self.part_rels_mut(part).push(Rel {
            id: id.clone(),
            kind: REL_HYPERLINK,
            target: href.to_owned(),
            external: true,
        });
        self.rel_keys.insert(key, id.clone());
        id
    }

    fn part_rels(&self, part: Part) -> &[Rel] {
        match part {
            Part::Document => &self.rels,
            Part::Footnotes => &self.footnote_rels,
        }
    }

    fn part_rels_mut(&mut self, part: Part) -> &mut Vec<Rel> {
        match part {
            Part::Document => &mut self.rels,
            Part::Footnotes => &mut self.footnote_rels,
        }
    }

    fn next_rel_id(&self, part: Part) -> String {
        format!("rId{}", self.part_rels(part).len() + 1)
    }

    fn text_run(&mut self, out: &mut String, text: &str, style: RunStyle, linked: bool, bold: bool) {
        let mut props = String::new();
        if linked {
            props.push_str("<w:rStyle w:val=\"Hyperlink\"/>");
        }
        if style.code || style.kbd {
            props.push_str("<w:rFonts w:ascii=\"");
            escape_attr(&mut props, &self.opts.code_font);
            props.push_str("\" w:hAnsi=\"");
            escape_attr(&mut props, &self.opts.code_font);
            props.push_str("\" w:cs=\"");
            escape_attr(&mut props, &self.opts.code_font);
            props.push_str("\"/>");
        }
        if style.strong || bold {
            props.push_str("<w:b/>");
        }
        if style.em {
            props.push_str("<w:i/>");
        }
        if style.strike {
            props.push_str("<w:strike/>");
        }
        if style.code || style.kbd {
            let size = self.code_size();
            props.push_str(&format!("<w:color w:val=\"24292F\"/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/>"));
        }
        if style.mark {
            props.push_str("<w:highlight w:val=\"yellow\"/>");
        }
        if style.underline {
            props.push_str("<w:u w:val=\"single\"/>");
        }
        if style.kbd {
            props.push_str("<w:bdr w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D1D9E0\"/>");
        }
        if style.code || style.kbd {
            props.push_str("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F6F8FA\"/>");
        }
        if style.sup {
            props.push_str("<w:vertAlign w:val=\"superscript\"/>");
        } else if style.sub {
            props.push_str("<w:vertAlign w:val=\"subscript\"/>");
        }
        out.push_str("<w:r>");
        if !props.is_empty() {
            out.push_str("<w:rPr>");
            out.push_str(&props);
            out.push_str("</w:rPr>");
        }
        write_run_content(out, text);
        out.push_str("</w:r>");
    }

    /// A run with hand-written properties (callout titles, captions, labels).
    fn styled_run(&mut self, out: &mut String, text: &str, props: &str, linked: bool) {
        out.push_str("<w:r>");
        let mut all = String::new();
        if linked {
            all.push_str("<w:rStyle w:val=\"Hyperlink\"/>");
        }
        all.push_str(props);
        if !all.is_empty() {
            out.push_str("<w:rPr>");
            out.push_str(&all);
            out.push_str("</w:rPr>");
        }
        write_run_content(out, text);
        out.push_str("</w:r>");
    }

    fn tex_run(&self, out: &mut String, tex: &str) {
        out.push_str("<w:r><w:rPr><w:rFonts w:ascii=\"");
        escape_attr(out, &self.opts.code_font);
        out.push_str("\" w:hAnsi=\"");
        escape_attr(out, &self.opts.code_font);
        out.push_str("\" w:cs=\"");
        escape_attr(out, &self.opts.code_font);
        out.push_str(&format!(
            "\"/><w:sz w:val=\"{}\"/><w:szCs w:val=\"{}\"/>\
             <w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{CODE_FILL}\"/></w:rPr>",
            self.code_size(),
            self.code_size()
        ));
        write_run_content(out, tex);
        out.push_str("</w:r>");
    }

    // ---- package parts ---------------------------------------------------

    fn document_xml(&self, body: &str) -> String {
        let width = self.opts.page.width_twips.max(1_440);
        let height = self.opts.page.height_twips.max(1_440);
        let margin = self.opts.page.margin_twips.min(width / 3).min(height / 3);
        // `w:footnotePr` precedes `w:pgSz` in the `sectPr` schema order. The
        // notes part numbers its entries 1..N, so the section says decimal and
        // continuous instead of inheriting a different Word default.
        let footnote_pr = if self.doc.footnotes.is_empty() {
            ""
        } else {
            "<w:footnotePr><w:numFmt w:val=\"decimal\"/><w:numStart w:val=\"1\"/>\
             <w:numRestart w:val=\"continuous\"/></w:footnotePr>"
        };
        format!(
            "{XML_DECLARATION}<w:document xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\" \
             xmlns:wp=\"{WP_NS}\" xmlns:a=\"{A_NS}\" xmlns:pic=\"{PIC_NS}\">\
             <w:body>{body}<w:sectPr>{footnote_pr}<w:pgSz w:w=\"{width}\" w:h=\"{height}\"/>\
             <w:pgMar w:top=\"{margin}\" w:right=\"{margin}\" w:bottom=\"{margin}\" \
             w:left=\"{margin}\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/>\
             </w:sectPr></w:body></w:document>\n"
        )
    }

    fn styles_xml(&self) -> String {
        let body = self.body_size();
        let code = self.code_size();
        let body_font = escape_attr_to_string(&self.opts.body_font);
        let code_font = escape_attr_to_string(&self.opts.code_font);
        let mut xml = String::from(XML_DECLARATION);
        xml.push_str(&format!("<w:styles xmlns:w=\"{W_NS}\">"));
        xml.push_str("<w:docDefaults><w:rPrDefault><w:rPr>");
        xml.push_str(&format!(
            "<w:rFonts w:ascii=\"{body_font}\" w:hAnsi=\"{body_font}\" w:eastAsia=\"{body_font}\" \
             w:cs=\"{body_font}\"/><w:color w:val=\"{BODY_COLOR}\"/>\
             <w:sz w:val=\"{body}\"/><w:szCs w:val=\"{body}\"/>\
             <w:lang w:val=\"en-US\" w:eastAsia=\"en-US\" w:bidi=\"ar-SA\"/>"
        ));
        xml.push_str("</w:rPr></w:rPrDefault><w:pPrDefault><w:pPr>");
        xml.push_str("<w:spacing w:after=\"160\" w:line=\"408\" w:lineRule=\"auto\"/>");
        xml.push_str("</w:pPr></w:pPrDefault></w:docDefaults>");

        xml.push_str("<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\">");
        xml.push_str("<w:name w:val=\"Normal\"/><w:qFormat/></w:style>");

        xml.push_str("<w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\">");
        xml.push_str("<w:name w:val=\"Default Paragraph Font\"/><w:uiPriority w:val=\"1\"/>");
        xml.push_str("<w:semiHidden/><w:unhideWhenUsed/></w:style>");

        xml.push_str("<w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\">");
        xml.push_str("<w:name w:val=\"Normal Table\"/><w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/>");
        xml.push_str("<w:tblPr><w:tblInd w:w=\"0\" w:type=\"dxa\"/><w:tblCellMar>");
        xml.push_str("<w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"108\" w:type=\"dxa\"/>");
        xml.push_str("<w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/>");
        xml.push_str("</w:tblCellMar></w:tblPr></w:style>");

        xml.push_str("<w:style w:type=\"paragraph\" w:styleId=\"Title\">");
        xml.push_str("<w:name w:val=\"Title\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/>");
        xml.push_str("<w:uiPriority w:val=\"10\"/><w:qFormat/><w:pPr><w:keepNext/>");
        xml.push_str(HEADING_RULE);
        xml.push_str("<w:spacing w:before=\"0\" w:after=\"240\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>");
        xml.push_str("<w:rPr><w:b/><w:sz w:val=\"56\"/><w:szCs w:val=\"56\"/></w:rPr></w:style>");

        for level in 1..=6u8 {
            let (before, after, size, color) = heading_metrics(level);
            let rule = if level <= 2 { HEADING_RULE } else { "" };
            xml.push_str(&format!(
                "<w:style w:type=\"paragraph\" w:styleId=\"{style}\"><w:name w:val=\"heading {level}\"/>\
                 <w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:uiPriority w:val=\"9\"/>\
                 <w:qFormat/><w:pPr><w:keepNext/><w:keepLines/>{rule}\
                 <w:spacing w:before=\"{before}\" w:after=\"{after}\" w:line=\"240\" w:lineRule=\"auto\"/>\
                 <w:outlineLvl w:val=\"{outline}\"/></w:pPr>\
                 <w:rPr><w:b/><w:color w:val=\"{color}\"/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/>\
                 </w:rPr></w:style>",
                style = heading_style(level),
                outline = level - 1
            ));
        }

        xml.push_str("<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\">");
        xml.push_str("<w:name w:val=\"List Paragraph\"/><w:basedOn w:val=\"Normal\"/>");
        xml.push_str("<w:uiPriority w:val=\"34\"/><w:qFormat/><w:pPr>");
        xml.push_str("<w:spacing w:after=\"40\"/><w:ind w:left=\"720\"/><w:contextualSpacing/></w:pPr></w:style>");

        // OI-080: a list inside a block quote must not keep the body colour
        // its runs would inherit from List Paragraph. The paragraph mark was
        // already muted; this style mutes the runs too.
        xml.push_str("<w:style w:type=\"paragraph\" w:styleId=\"QuoteListParagraph\">");
        xml.push_str("<w:name w:val=\"Quoted List Paragraph\"/><w:basedOn w:val=\"ListParagraph\"/>");
        xml.push_str("<w:next w:val=\"Normal\"/><w:uiPriority w:val=\"34\"/><w:qFormat/>");
        xml.push_str(&format!("<w:rPr><w:color w:val=\"{MUTED_COLOR}\"/></w:rPr></w:style>"));

        // The TOC field's own heading and entry styles. The right tab with a
        // dot leader is what Word uses when it rebuilds the field with page
        // numbers; the cached entries have no page numbers of their own.
        xml.push_str("<w:style w:type=\"paragraph\" w:styleId=\"TOCHeading\">");
        xml.push_str("<w:name w:val=\"TOC Heading\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/>");
        xml.push_str("<w:uiPriority w:val=\"39\"/><w:qFormat/><w:pPr><w:keepNext/>");
        xml.push_str("<w:spacing w:before=\"240\" w:after=\"120\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>");
        xml.push_str("<w:rPr><w:b/><w:sz w:val=\"32\"/><w:szCs w:val=\"32\"/></w:rPr></w:style>");
        for level in 1..=6u8 {
            xml.push_str(&format!(
                "<w:style w:type=\"paragraph\" w:styleId=\"{style}\"><w:name w:val=\"toc {level}\"/>\
                 <w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:uiPriority w:val=\"39\"/>\
                 <w:unhideWhenUsed/><w:pPr><w:tabs><w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"{tab}\"/>\
                 </w:tabs><w:spacing w:before=\"0\" w:after=\"60\" w:line=\"240\" w:lineRule=\"auto\"/>\
                 <w:ind w:left=\"{indent}\"/></w:pPr></w:style>",
                style = toc_style(level),
                tab = self.content_width,
                indent = 220 * (u32::from(level) - 1),
            ));
        }

        xml.push_str("<w:style w:type=\"paragraph\" w:styleId=\"Quote\">");
        xml.push_str("<w:name w:val=\"Quote\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/>");
        xml.push_str("<w:uiPriority w:val=\"29\"/><w:qFormat/><w:pPr>");
        xml.push_str(QUOTE_BORDER);
        xml.push_str("<w:ind w:left=\"360\"/></w:pPr>");
        xml.push_str(&format!("<w:rPr><w:color w:val=\"{MUTED_COLOR}\"/></w:rPr></w:style>"));

        xml.push_str("<w:style w:type=\"paragraph\" w:styleId=\"CodeBlock\">");
        xml.push_str("<w:name w:val=\"Code Block\"/><w:basedOn w:val=\"Normal\"/><w:uiPriority w:val=\"99\"/>");
        xml.push_str(&format!(
            "<w:pPr><w:keepLines/><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/>\
             <w:jc w:val=\"left\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"{code_font}\" w:hAnsi=\"{code_font}\" \
             w:cs=\"{code_font}\"/><w:sz w:val=\"{code}\"/><w:szCs w:val=\"{code}\"/></w:rPr></w:style>"
        ));

        xml.push_str("<w:style w:type=\"character\" w:styleId=\"CodeChar\">");
        xml.push_str("<w:name w:val=\"Code Char\"/><w:basedOn w:val=\"DefaultParagraphFont\"/>");
        xml.push_str("<w:uiPriority w:val=\"1\"/><w:qFormat/>");
        xml.push_str(&format!(
            "<w:rPr><w:rFonts w:ascii=\"{code_font}\" w:hAnsi=\"{code_font}\" w:cs=\"{code_font}\"/>\
             <w:sz w:val=\"{code}\"/><w:szCs w:val=\"{code}\"/>\
             <w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F6F8FA\"/></w:rPr></w:style>"
        ));

        xml.push_str("<w:style w:type=\"character\" w:styleId=\"Hyperlink\">");
        xml.push_str("<w:name w:val=\"Hyperlink\"/><w:basedOn w:val=\"DefaultParagraphFont\"/>");
        xml.push_str("<w:uiPriority w:val=\"99\"/><w:unhideWhenUsed/>");
        xml.push_str("<w:rPr><w:color w:val=\"0969DA\"/><w:u w:val=\"single\"/></w:rPr></w:style>");

        // Footnotes: the note text is smaller than the body, and the reference
        // run (body marker and the note's own `w:footnoteRef`) is superscript.
        xml.push_str("<w:style w:type=\"paragraph\" w:styleId=\"FootnoteText\">");
        xml.push_str("<w:name w:val=\"footnote text\"/><w:basedOn w:val=\"Normal\"/>");
        xml.push_str("<w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/><w:qFormat/>");
        xml.push_str("<w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>");
        xml.push_str("<w:rPr><w:sz w:val=\"18\"/><w:szCs w:val=\"18\"/></w:rPr></w:style>");

        xml.push_str("<w:style w:type=\"character\" w:styleId=\"FootnoteReference\">");
        xml.push_str("<w:name w:val=\"footnote reference\"/><w:basedOn w:val=\"DefaultParagraphFont\"/>");
        xml.push_str("<w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/>");
        xml.push_str("<w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr></w:style>");

        xml.push_str("</w:styles>\n");
        xml
    }

    fn numbering_xml(&self) -> String {
        let mut xml = String::from(XML_DECLARATION);
        xml.push_str(&format!("<w:numbering xmlns:w=\"{W_NS}\">"));
        xml.push_str("<w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"hybridMultilevel\"/>");
        for level in 0..9u32 {
            let bullet = ["\u{2022}", "\u{25E6}", "\u{25AA}"][level as usize % 3];
            xml.push_str(&format!(
                "<w:lvl w:ilvl=\"{level}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/>\
                 <w:lvlText w:val=\"{bullet}\"/><w:lvlJc w:val=\"left\"/>\
                 <w:pPr><w:ind w:left=\"{left}\" w:hanging=\"360\"/></w:pPr>\
                 <w:rPr><w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\" w:hint=\"default\"/></w:rPr></w:lvl>",
                left = (level + 1) * INDENT
            ));
        }
        xml.push_str("</w:abstractNum>");
        xml.push_str("<w:abstractNum w:abstractNumId=\"1\"><w:multiLevelType w:val=\"hybridMultilevel\"/>");
        for level in 0..9u32 {
            xml.push_str(&format!(
                "<w:lvl w:ilvl=\"{level}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/>\
                 <w:lvlText w:val=\"%{number}.\"/><w:lvlJc w:val=\"left\"/>\
                 <w:pPr><w:ind w:left=\"{left}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
                number = level + 1,
                left = (level + 1) * INDENT
            ));
        }
        xml.push_str("</w:abstractNum>");
        for number in &self.numbers {
            xml.push_str(&format!(
                "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{}\"/>",
                number.id, number.abstract_id
            ));
            if let Some(start) = number.start {
                xml.push_str(&format!(
                    "<w:lvlOverride w:ilvl=\"{}\"><w:startOverride w:val=\"{start}\"/></w:lvlOverride>",
                    number.level
                ));
            }
            xml.push_str("</w:num>");
        }
        xml.push_str("</w:numbering>\n");
        xml
    }

    fn rels_xml(&self) -> String {
        relationships_xml(&self.rels)
    }

    fn footnote_rels_xml(&self) -> String {
        relationships_xml(&self.footnote_rels)
    }

    fn core_xml(&self) -> String {
        let mut xml = String::from(XML_DECLARATION);
        xml.push_str(
            "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
             xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
             xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" \
             xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">",
        );
        xml.push_str("<dc:title>");
        escape_text(&mut xml, &self.doc.title);
        xml.push_str("</dc:title><dc:language>");
        let lang = if self.doc.lang.trim().is_empty() { "en" } else { self.doc.lang.trim() };
        escape_text(&mut xml, lang);
        xml.push_str(&format!(
            "</dc:language><cp:lastModifiedBy>lmdv</cp:lastModifiedBy>\
             <dcterms:created xsi:type=\"dcterms:W3CDTF\">{FIXED_TIMESTAMP}</dcterms:created>\
             <dcterms:modified xsi:type=\"dcterms:W3CDTF\">{FIXED_TIMESTAMP}</dcterms:modified>\
             </cp:coreProperties>\n"
        ));
        xml
    }
}

impl Ppr {
    fn centered(mut self) -> Self {
        self.jc = Some("center");
        self
    }
}

// ---- static tables -------------------------------------------------------

/// One part's `.rels` file.
fn relationships_xml(rels: &[Rel]) -> String {
    let mut xml = String::from(XML_DECLARATION);
    xml.push_str("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n");
    for rel in rels {
        xml.push_str("<Relationship Id=\"");
        xml.push_str(&rel.id);
        xml.push_str("\" Type=\"");
        xml.push_str(rel.kind);
        xml.push_str("\" Target=\"");
        escape_attr(&mut xml, &rel.target);
        xml.push('"');
        if rel.external {
            xml.push_str(" TargetMode=\"External\"");
        }
        xml.push_str("/>\n");
    }
    xml.push_str("</Relationships>\n");
    xml
}

fn heading_style(level: u8) -> &'static str {
    match level.clamp(1, 6) {
        1 => "Heading1",
        2 => "Heading2",
        3 => "Heading3",
        4 => "Heading4",
        5 => "Heading5",
        _ => "Heading6",
    }
}

/// The built-in Word paragraph style of a cached TOC entry at `level`.
fn toc_style(level: u8) -> &'static str {
    match level.clamp(1, 6) {
        1 => "TOC1",
        2 => "TOC2",
        3 => "TOC3",
        4 => "TOC4",
        5 => "TOC5",
        _ => "TOC6",
    }
}

/// Inserts `runs` as the first content of a rendered paragraph, after its
/// `w:pPr` (the field opening must precede the cached entry's hyperlink).
fn insert_after_paragraph_properties(paragraph: &mut String, runs: &str) {
    let at = paragraph
        .find("</w:pPr>")
        .map(|at| at + "</w:pPr>".len())
        .or_else(|| paragraph.find('>').map(|at| at + 1))
        .unwrap_or(0);
    paragraph.insert_str(at, runs);
}

/// Inserts `runs` just before a rendered paragraph's closing tag.
fn insert_before_paragraph_end(paragraph: &mut String, runs: &str) {
    match paragraph.rfind("</w:p>") {
        Some(at) => paragraph.insert_str(at, runs),
        None => paragraph.push_str(runs),
    }
}

/// `(before, after, half-point size, color)` for a heading level.
fn heading_metrics(level: u8) -> (u32, u32, u32, &'static str) {
    match level.clamp(1, 6) {
        1 => (360, 120, 48, BODY_COLOR),
        2 => (320, 100, 36, BODY_COLOR),
        3 => (280, 80, 28, BODY_COLOR),
        4 => (240, 60, 24, BODY_COLOR),
        5 => (220, 50, 22, BODY_COLOR),
        _ => (200, 40, 20, MUTED_COLOR),
    }
}

fn align_name(align: Align) -> Option<&'static str> {
    match align {
        Align::None | Align::Left => None,
        Align::Center => Some("center"),
        Align::Right => Some("right"),
    }
}

fn callout_symbol(kind: CalloutKind) -> &'static str {
    match kind {
        CalloutKind::Note => "\u{2139}\u{FE0F}",
        CalloutKind::Tip => "\u{1F4A1}",
        CalloutKind::Important => "\u{2757}",
        CalloutKind::Warning => "\u{26A0}\u{FE0F}",
        CalloutKind::Caution => "\u{26D4}",
    }
}

fn callout_color(kind: CalloutKind) -> &'static str {
    match kind {
        CalloutKind::Note => "0969DA",
        CalloutKind::Tip => "1A7F37",
        CalloutKind::Important => "8250DF",
        CalloutKind::Warning => "9A6700",
        CalloutKind::Caution => "CF222E",
    }
}

fn callout_fill(kind: CalloutKind) -> &'static str {
    match kind {
        CalloutKind::Note => "F3F8FD",
        CalloutKind::Tip => "F4F9F5",
        CalloutKind::Important => "F9F6FD",
        CalloutKind::Warning => "FAF7F2",
        CalloutKind::Caution => "FDF4F5",
    }
}

fn media_extension(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" => "jpeg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/webp" => "webp",
        _ => return None,
    })
}

/// The drawing extent in EMU: the asset's logical size when known, else the
/// converted raster's pixel size, else the picture's intrinsic pixel size at
/// 96 dpi, capped at 16 cm wide.
fn image_extent(asset: &ExportAsset, raster: Option<(u64, u64)>) -> (u64, u64) {
    let (width, height) = match (asset.width, asset.height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => (u64::from(width), u64::from(height)),
        _ => raster.or_else(|| intrinsic_size(&asset.bytes)).unwrap_or(FALLBACK_MEDIA),
    };
    let mut cx = width.saturating_mul(EMU_PER_PIXEL).max(1);
    let mut cy = height.saturating_mul(EMU_PER_PIXEL).max(1);
    if cx > MAX_MEDIA_EMU {
        cy = (cy.saturating_mul(MAX_MEDIA_EMU) / cx).max(1);
        cx = MAX_MEDIA_EMU;
    }
    (cx, cy)
}

/// The picture's pixel size, read from its header (the image crate does not
/// decode the pixels).
fn intrinsic_size(bytes: &[u8]) -> Option<(u64, u64)> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let (width, height) = reader.into_dimensions().ok()?;
    (width > 0 && height > 0).then_some((u64::from(width), u64::from(height)))
}

/// Assigns one bookmark name per heading id, in document order. Word bookmark
/// names start with a letter or `_`, are at most 40 characters long and cannot
/// contain `-`, so GitHub's slug cannot be used verbatim.
fn collect_bookmarks(doc: &ExportDoc) -> HashMap<String, String> {
    let mut map = HashMap::new();
    walk_bookmarks(&doc.blocks, &mut map);
    for footnote in &doc.footnotes {
        walk_bookmarks(&footnote.blocks, &mut map);
    }
    map
}

fn walk_bookmarks(blocks: &[ExportBlock], map: &mut HashMap<String, String>) {
    for block in blocks {
        match block {
            ExportBlock::Heading { id, .. } => {
                if !id.is_empty() && !map.contains_key(id) {
                    map.insert(id.clone(), format!("_lmdv{}", map.len() + 1));
                }
            }
            ExportBlock::Quote { blocks, .. } => walk_bookmarks(blocks, map),
            ExportBlock::List(list) => {
                for item in &list.items {
                    walk_bookmarks(&item.blocks, map);
                }
            }
            _ => {}
        }
    }
}

/// The plain text of a run sequence (used for the title/heading comparison).
fn plain_text(runs: &[ExportRun]) -> String {
    let mut text = String::new();
    for run in runs {
        match run {
            ExportRun::Text { text: run, .. } => text.push_str(run),
            ExportRun::Math { tex, .. } => text.push_str(tex),
            ExportRun::Break => text.push(' '),
            _ => {}
        }
    }
    text
}

/// The bold/italic/… flags of a raw HTML inline subtree.
#[derive(Clone, Copy, Default)]
struct HtmlFmt {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    code: bool,
    mark: bool,
    sub: bool,
    sup: bool,
}

impl HtmlFmt {
    fn style(self) -> RunStyle {
        RunStyle {
            strong: self.bold,
            em: self.italic,
            strike: self.strike,
            underline: self.underline,
            code: self.code,
            mark: self.mark,
            sub: self.sub,
            sup: self.sup,
            kbd: false,
        }
    }
}

/// Carries the outer inline flags into a nested element.
fn merge_fmt(fmt: HtmlFmt, tag: HtmlTag) -> HtmlFmt {
    match tag {
        HtmlTag::Strong | HtmlTag::B => HtmlFmt { bold: true, ..fmt },
        HtmlTag::Em | HtmlTag::I => HtmlFmt { italic: true, ..fmt },
        HtmlTag::S | HtmlTag::Del => HtmlFmt { strike: true, ..fmt },
        HtmlTag::U | HtmlTag::Ins => HtmlFmt { underline: true, ..fmt },
        HtmlTag::Code | HtmlTag::Kbd => HtmlFmt { code: true, ..fmt },
        HtmlTag::Mark => HtmlFmt { mark: true, ..fmt },
        HtmlTag::Sub => HtmlFmt { sub: true, ..fmt },
        HtmlTag::Sup => HtmlFmt { sup: true, ..fmt },
        _ => fmt,
    }
}

/// Collects `(header, tr)` pairs from a raw HTML table subtree.
fn collect_html_rows<'n>(node: &'n HtmlNode, header: bool, rows: &mut Vec<(bool, &'n HtmlNode)>) {
    for child in &node.children {
        let HtmlChild::Node(child) = child else { continue };
        match child.tag {
            HtmlTag::Tr => rows.push((header, child)),
            HtmlTag::Thead => collect_html_rows(child, true, rows),
            HtmlTag::Tbody | HtmlTag::Tfoot => collect_html_rows(child, false, rows),
            _ => collect_html_rows(child, header, rows),
        }
    }
}

/// Whether a resolved href may be written as an external relationship.
fn href_is_safe(href: &str) -> bool {
    if href.is_empty() || href.chars().any(char::is_control) {
        return false;
    }
    match crate::link::scheme(href).map(str::to_ascii_lowercase).as_deref() {
        Some("http" | "https" | "mailto" | "file") => true,
        Some(_) => false,
        None => true,
    }
}

/// The last path segment of a media URL, for its link text.
fn file_name(src: &str) -> Option<String> {
    let path = src.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?.rsplit('\\').next()?;
    let name = percent_encoding::percent_decode_str(name).decode_utf8_lossy();
    let name = name.trim();
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_owned())
}

// ---- XML ----------------------------------------------------------------

/// Escapes text content and attribute values, dropping characters that XML 1.0
/// cannot represent.
fn escape_text(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other if xml_char(other) => out.push(other),
            _ => {}
        }
    }
}

/// Escapes an attribute value. XML normalizes literal tabs and newlines in
/// attributes anyway; replacing them here keeps the generated XML single-line
/// and makes the TeX alt text stable.
fn escape_attr(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' | '\r' | '\t' => out.push(' '),
            other if xml_char(other) => out.push(other),
            _ => {}
        }
    }
}

fn escape_attr_to_string(text: &str) -> String {
    let mut out = String::new();
    escape_attr(&mut out, text);
    out
}

/// XML 1.0 `Char`: tab, LF, CR and the printable ranges minus the
/// noncharacters at the end of the BMP.
fn xml_char(character: char) -> bool {
    matches!(
        character,
        '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    )
}

/// Writes a run's text as `w:t` nodes, with tabs and hard breaks as their own
/// elements. Every `w:t` preserves whitespace so code and multiple spaces
/// survive.
fn write_run_content(out: &mut String, text: &str) {
    let mut buffer = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\t' => {
                flush_text(out, &mut buffer);
                out.push_str("<w:tab/>");
            }
            '\n' => {
                flush_text(out, &mut buffer);
                out.push_str("<w:br/>");
            }
            '\r' => {
                flush_text(out, &mut buffer);
                if chars.peek() != Some(&'\n') {
                    out.push_str("<w:br/>");
                }
            }
            other if xml_char(other) => buffer.push(other),
            _ => {}
        }
    }
    flush_text(out, &mut buffer);
}

fn flush_text(out: &mut String, buffer: &mut String) {
    if buffer.is_empty() {
        return;
    }
    out.push_str("<w:t xml:space=\"preserve\">");
    escape_text(out, buffer);
    out.push_str("</w:t>");
    buffer.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExportOptions, NoAssets};

    #[test]
    fn escaped_text_drops_invalid_xml_characters() {
        let mut out = String::new();
        escape_text(&mut out, "a & b < c > d \"e\" 'f' \u{0}\u{1}\u{7}\u{B}\u{1F}");
        assert_eq!(out, "a &amp; b &lt; c &gt; d &quot;e&quot; &apos;f&apos; ");
        assert!(!xml_char('\u{1}'));
        assert!(xml_char('\u{9}'));
        assert!(xml_char('\u{FFFD}'));
        assert!(!xml_char('\u{FFFE}'));
        assert!(xml_char('\u{1F600}'));
    }

    #[test]
    fn attribute_values_are_single_line_and_escaped() {
        let mut out = String::new();
        escape_attr(&mut out, "a\tb\nc\r\"d\"");
        assert_eq!(out, "a b c &quot;d&quot;");
    }

    #[test]
    fn run_content_keeps_tabs_spaces_and_breaks() {
        let mut out = String::new();
        write_run_content(&mut out, "a\tb  c\r\nd\ne");
        assert_eq!(
            out,
            "<w:t xml:space=\"preserve\">a</w:t><w:tab/>\
             <w:t xml:space=\"preserve\">b  c</w:t><w:br/>\
             <w:t xml:space=\"preserve\">d</w:t><w:br/>\
             <w:t xml:space=\"preserve\">e</w:t>"
        );
        let mut only_spaces = String::new();
        write_run_content(&mut only_spaces, "   ");
        assert_eq!(only_spaces, "<w:t xml:space=\"preserve\">   </w:t>");
    }

    #[test]
    fn only_raster_types_are_embedded() {
        assert_eq!(media_extension("image/png"), Some("png"));
        assert_eq!(media_extension("image/jpeg"), Some("jpeg"));
        assert_eq!(media_extension("image/webp"), Some("webp"));
        // SVG is rasterized to PNG before this lookup (`media_for`); AVIF has
        // no decoder in the dependency tree, so both are not plain media types.
        assert_eq!(media_extension("image/svg+xml"), None);
        assert_eq!(media_extension("image/avif"), None);
        assert_eq!(media_extension("application/pdf"), None);
    }

    #[test]
    fn a_drawing_caps_at_sixteen_centimetres_and_keeps_the_aspect_ratio() {
        let asset = ExportAsset {
            id: AssetId(0),
            mime: "image/png",
            bytes: Vec::new(),
            width: Some(3200),
            height: Some(1000),
            baseline: None,
        };
        let (cx, cy) = image_extent(&asset, None);
        assert_eq!(cx, MAX_MEDIA_EMU);
        // 3200 x 9525 was clamped, so the height keeps the 3.2:1 ratio.
        assert!((cy as f64 - (MAX_MEDIA_EMU as f64 / 3.2)).abs() < 2.0, "{cy}");

        let small = ExportAsset { width: Some(100), height: Some(50), ..asset };
        assert_eq!(image_extent(&small, None), (100 * EMU_PER_PIXEL, 50 * EMU_PER_PIXEL));
    }

    #[test]
    fn a_png_header_gives_its_intrinsic_size() {
        // The fixture PNG is 320 x 140 (the reader never decodes pixels).
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/img/test.png");
        let bytes = std::fs::read(path).expect("the fixture PNG");
        let (width, height) = intrinsic_size(&bytes).expect("PNG dimensions");
        assert_eq!((width, height), (320, 140));
    }

    #[test]
    fn media_file_names_come_from_local_and_percent_encoded_urls() {
        assert_eq!(file_name("file:///home/jane/docs/clip.mp4").as_deref(), Some("clip.mp4"));
        assert_eq!(file_name("https://example.com/a%20b.mp3?x=1#t").as_deref(), Some("a b.mp3"));
        assert_eq!(file_name("clip.mp4").as_deref(), Some("clip.mp4"));
        assert_eq!(file_name("https://example.com/"), None);
    }

    #[test]
    fn hrefs_are_limited_to_document_and_web_targets() {
        assert!(href_is_safe("https://example.com"));
        assert!(href_is_safe("mailto:a@example.com"));
        assert!(href_is_safe("file:///home/jane/a.md"));
        assert!(!href_is_safe("javascript:alert(1)"));
        assert!(!href_is_safe("data:text/html,x"));
        assert!(!href_is_safe("obsidian://open"));
        assert!(!href_is_safe(""));
        assert!(!href_is_safe("file:///a\u{1}"));
    }

    #[test]
    fn every_heading_gets_a_word_safe_bookmark() {
        let doc = lmdv_md::parse("# One\n\n## Two\n\n### Two\n");
        let mut exported = ExportDoc::from_md(&doc, &ExportOptions::default(), &NoAssets).unwrap();
        exported.blocks.push(ExportBlock::Rule);
        let bookmarks = collect_bookmarks(&exported);
        let ids: Vec<&str> = doc.headings.iter().map(|heading| heading.id.as_str()).collect();
        for id in ids {
            let name = bookmarks.get(id).unwrap_or_else(|| panic!("bookmark for {id}"));
            assert!(name.starts_with("_lmdv"));
            assert!(name.len() <= 40);
            assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        }
        assert_eq!(bookmarks.len(), doc.headings.len());
    }

    #[test]
    fn callout_metrics_match_the_reference_palette() {
        assert_eq!(callout_symbol(CalloutKind::Note), "\u{2139}\u{FE0F}");
        assert_eq!(callout_symbol(CalloutKind::Caution), "\u{26D4}");
        assert_eq!(callout_fill(CalloutKind::Warning), "FAF7F2");
        assert_eq!(callout_color(CalloutKind::Caution), "CF222E");
        assert_eq!(heading_style(0), "Heading1");
        assert_eq!(heading_style(7), "Heading6");
        assert_eq!(align_name(Align::Left), None);
        assert_eq!(align_name(Align::Right), Some("right"));
    }

    #[test]
    fn the_default_page_is_a4_with_normal_margins() {
        let options = DocxOptions::default();
        assert_eq!(options.page, DocxPage::A4);
        assert_eq!(options.body_size_half_points, 22);
        assert_eq!(options.body_font, "Calibri");
        assert_eq!(options.page.width_twips - 2 * options.page.margin_twips, 9_026);
    }
}
