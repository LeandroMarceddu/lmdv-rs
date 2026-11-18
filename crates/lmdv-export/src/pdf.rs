//! Print-ready PDF serializer.
//!
//! The same [`ExportDoc`] the HTML and DOCX writers consume is turned into a
//! Typst source document and compiled in-process; the output is a tagged,
//! font-subset A4 PDF with a page model, running header/footer, repeated table
//! headers, real page breaks and embedded PNG formulas. A table wider than the
//! paper is measured at layout time and scaled down like the print stylesheet's
//! CSS zoom, with the original 0.3 floor and a break-anywhere fallback (X-12).
//! No webview, no external binary, no network.
//!
//! Raw HTML from the allowed subset keeps its structure: tables (with
//! `colspan`/`rowspan`), nested lists, `<details>` cards, quotes, code and
//! media placeholders reuse the same Typst builders as Markdown; tags outside
//! the subset keep their text.
//!
//! Reader theme, size, line height and column width cannot reach this module:
//! [`PdfOptions`] carries only print settings and the font bytes the
//! application supplies. ADR-015 makes this a second, independent layout
//! pipeline rather than a screenshot of the reader.
//!
//! # Fonts
//!
//! The exporter never reads a font file. [`PdfOptions::fonts`] is the
//! application's list of [`PdfFont`] entries; Typst parses them, uses the
//! named family for the body or code text, falls back to them for missing
//! glyphs, and subsets what the document actually uses. When the list is empty
//! the engine's bundled faces (Libertinus Serif, New Computer Modern, DejaVu
//! Sans Mono) keep the default usable and deterministic.
//!
//! # Determinism
//!
//! The document date is `none`, the PDF creation timestamp is omitted and the
//! document identifier is derived from the title. Two exports of the same
//! [`ExportDoc`] and [`PdfOptions`] are byte-identical.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Cursor;
use std::path::PathBuf;

use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::png::PngEncoder;
use lmdv_md::{Align, CalloutKind, HtmlChild, HtmlNode, HtmlTag};
use typst::diag::{FileError, FileResult, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Smart};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

use crate::labels::ExportLabels;
use crate::model::{
    AssetId, ExportAsset, ExportBlock, ExportDoc, ExportError, ExportImage, ExportItem, ExportLink, ExportList,
    ExportRun, ExportTable, ExportTocEntry, RunStyle,
};

/// Body ink, matching the light export article.
const TEXT: &str = "#1f2328";
/// Secondary ink for captions, quotes and the header/footer.
const MUTED: &str = "#59636e";
/// Table, code and callout borders.
const BORDER: &str = "#d1d9e0";
/// Code, table headers and tinted surfaces.
const SURFACE: &str = "#f6f8fa";
/// Zebra table rows.
const BAND: &str = "#fbfcfd";
/// `<mark>` background.
const MARK: &str = "#fff8c5";
/// The bundled body family when the caller supplies no fonts.
const FALLBACK_BODY: &str = "Libertinus Serif";
/// The bundled monospace family when the caller supplies no fonts.
const FALLBACK_CODE: &str = "DejaVu Sans Mono";
/// The Typst string used for the table zebra stripe.
const WHITE: &str = "#ffffff";

/// Page geometry in millimetres.
///
/// The default is the original print layout: A4 portrait with 20 mm on top,
/// 16 mm on the right, 18 mm at the bottom and 16 mm on the left (X-04).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfPage {
    /// Page width in millimetres.
    pub width_mm: f32,
    /// Page height in millimetres.
    pub height_mm: f32,
    /// Top margin in millimetres; the running header sits here.
    pub top_mm: f32,
    /// Right margin in millimetres.
    pub right_mm: f32,
    /// Bottom margin in millimetres; the page number sits here.
    pub bottom_mm: f32,
    /// Left margin in millimetres.
    pub left_mm: f32,
}

impl PdfPage {
    /// A4 portrait (210 × 297 mm) with the original print margins.
    pub const A4: Self =
        Self { width_mm: 210.0, height_mm: 297.0, top_mm: 20.0, right_mm: 16.0, bottom_mm: 18.0, left_mm: 16.0 };

    /// The printable width in points (1 pt = 1/72 inch).
    fn content_width_pt(self) -> f32 {
        (self.width_mm - self.left_mm - self.right_mm).max(1.0) * 72.0 / 25.4
    }
}

impl Default for PdfPage {
    fn default() -> Self {
        Self::A4
    }
}

/// One font the application supplies.
///
/// `family` is the name Typst should use in `font:`; the bytes are parsed by
/// the engine, so the label is only a hint for the caller's convenience. A
/// font collection (`.ttc`) contributes all of its faces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfFont {
    /// The family name the caller wants to refer to, e.g. `Liberation Sans`.
    pub family: String,
    /// The encoded font file.
    pub bytes: Vec<u8>,
}

impl PdfFont {
    /// A font with `family` and its encoded `bytes`.
    pub fn new(family: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self { family: family.into(), bytes: bytes.into() }
    }
}

/// Print settings for [`export_pdf`].
///
/// Every value is a print value; none is copied from the reader. The defaults
/// are the original export layout: A4 with 20/16/18/16 mm margins, 12 pt sans
/// body text, 9.5 pt monospace code and a light, white-page theme.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfOptions {
    /// The localized date line in the running header, formatted by the app.
    ///
    /// The header is `doc.title · date`. `None` or an empty string omits the
    /// date; an empty title and date omit the header entirely.
    pub date: Option<String>,
    /// Start a new page before every level-1 heading after the first (X-06).
    pub break_on_h1: bool,
    /// The body family. Empty picks the first [`PdfFont`], else the bundled
    /// serif face.
    pub body_font: String,
    /// The monospace family. Empty picks the first supplied family whose name
    /// looks monospaced, else the bundled DejaVu Sans Mono.
    pub code_font: String,
    /// Body size in points. Code is 0.8× this size.
    pub body_size_pt: f32,
    /// Page size and margins.
    pub page: PdfPage,
    /// Font files the application supplies. The exporter never reads one.
    pub fonts: Vec<PdfFont>,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            date: None,
            break_on_h1: false,
            body_font: String::new(),
            code_font: String::new(),
            body_size_pt: 12.0,
            page: PdfPage::A4,
            fonts: Vec::new(),
        }
    }
}

impl PdfOptions {
    /// The body family this option set resolves to.
    pub fn resolved_body_font(&self) -> &str {
        if !self.body_font.trim().is_empty() {
            return self.body_font.trim();
        }
        self.fonts.first().map(|font| font.family.trim()).filter(|family| !family.is_empty()).unwrap_or(FALLBACK_BODY)
    }

    /// The monospace family this option set resolves to.
    pub fn resolved_code_font(&self) -> &str {
        if !self.code_font.trim().is_empty() {
            return self.code_font.trim();
        }
        self.fonts
            .iter()
            .map(|font| font.family.trim())
            .find(|family| family.to_ascii_lowercase().contains("mono") && !family.is_empty())
            .unwrap_or(FALLBACK_CODE)
    }
}

/// Writes `doc` as a print-ready A4 PDF.
///
/// Missing pictures and formulas fall back to alt text and TeX, exactly as the
/// other writers do. Only an engine failure (an invalid option or a bug in the
/// generated Typst source) returns [`ExportError::Pdf`].
pub fn export_pdf(doc: &ExportDoc, opts: &PdfOptions) -> Result<Vec<u8>, ExportError> {
    validate(opts)?;

    let prepared = prepare_assets(doc)?;
    let labels = heading_labels(doc);
    let has_footnotes = doc.footnotes.iter().any(|footnote| !footnote.blocks.is_empty())
        && doc.blocks.iter().any(|block| matches!(block, ExportBlock::Footnotes));
    let source_text = render_source(doc, opts, &prepared, &labels, has_footnotes);

    let (fonts, book) = collect_fonts(&opts.fonts);
    let main_path = VirtualPath::new("main.typ").expect("a literal virtual path");
    let main = RootedPath::new(VirtualRoot::Project, main_path).intern();
    let mut files = HashMap::new();
    for asset in &prepared {
        if let Some((id, bytes)) = &asset.file {
            files.insert(*id, bytes.clone());
        }
    }
    let world = PdfWorld {
        library: LazyHash::new(Library::default()),
        book: LazyHash::new(book),
        main,
        source: Source::new(main, source_text),
        files,
        fonts,
    };

    let document = typst::compile::<PagedDocument>(&world).output.map_err(|errors| {
        ExportError::Pdf(format!("the typesetter rejected the document: {}", diagnostics(&errors)))
    })?;

    let pdf_options = typst_pdf::PdfOptions {
        ident: Smart::Auto,
        creator: Smart::Custom(Some("lmdv".to_owned())),
        timestamp: None,
        page_ranges: None,
        standards: typst_pdf::PdfStandards::default(),
        tagged: true,
        pretty: false,
    };
    typst_pdf::pdf(&document, &pdf_options)
        .map_err(|errors| ExportError::Pdf(format!("the PDF backend failed: {}", diagnostics(&errors))))
}

impl ExportDoc {
    /// Serializes this document as a PDF; see [`export_pdf`].
    pub fn to_pdf(&self, opts: &PdfOptions) -> Result<Vec<u8>, ExportError> {
        export_pdf(self, opts)
    }
}

/// Rejects options the typesetter cannot express.
fn validate(opts: &PdfOptions) -> Result<(), ExportError> {
    let page = opts.page;
    if !(6.0..=24.0).contains(&opts.body_size_pt) {
        return Err(ExportError::Pdf(format!("body size {} pt is outside 6–24 pt", opts.body_size_pt)));
    }
    if !(page.width_mm.is_finite() && page.height_mm.is_finite())
        || page.width_mm < 40.0
        || page.height_mm < 40.0
        || page.width_mm > 2000.0
        || page.height_mm > 2000.0
    {
        return Err(ExportError::Pdf(format!("page size {} × {} mm is not printable", page.width_mm, page.height_mm)));
    }
    let margins = [page.top_mm, page.right_mm, page.bottom_mm, page.left_mm];
    if margins.iter().any(|margin| !margin.is_finite() || *margin < 0.0)
        || page.left_mm + page.right_mm >= page.width_mm
        || page.top_mm + page.bottom_mm >= page.height_mm
    {
        return Err(ExportError::Pdf(format!(
            "margins {} × {} mm do not fit a {} × {} mm page",
            page.left_mm + page.right_mm,
            page.top_mm + page.bottom_mm,
            page.width_mm,
            page.height_mm
        )));
    }
    Ok(())
}

// ---- assets -------------------------------------------------------------

/// One asset as the Typst world sees it.
struct PreparedAsset {
    /// The asset this entry was prepared from.
    asset: AssetId,
    /// The virtual path inside the Typst project, e.g. `assets/3.png`.
    path: String,
    /// The file id and bytes, when the format is one Typst can embed.
    file: Option<(FileId, Bytes)>,
    /// The intrinsic size in CSS pixels (96 dpi), when it can be known.
    size: Option<(u32, u32)>,
}

/// Assigns a virtual path and optional transcoded bytes to every asset.
///
/// Typst reads PNG, JPEG, GIF, WebP and SVG. BMP is decoded and re-encoded as
/// PNG so a valid picture is not silently lost. Anything else falls back to
/// text where it is referenced.
fn prepare_assets(doc: &ExportDoc) -> Result<Vec<PreparedAsset>, ExportError> {
    let mut prepared = Vec::with_capacity(doc.assets.len());
    for (index, asset) in doc.assets.iter().enumerate() {
        let extension = asset_extension(asset);
        let Some(extension) = extension else {
            prepared.push(PreparedAsset { asset: asset.id, path: String::new(), file: None, size: None });
            continue;
        };
        let (bytes, size) = if asset.mime == "image/bmp" {
            let decoded = image::load_from_memory_with_format(&asset.bytes, image::ImageFormat::Bmp)
                .map_err(|error| ExportError::Pdf(format!("could not decode a BMP picture: {error}")))?;
            let size = (decoded.width(), decoded.height());
            (encode_png(&decoded)?, Some(size))
        } else {
            (asset.bytes.clone(), asset.size_in_pixels())
        };
        let path = format!("assets/{index}.{extension}");
        let virtual_path =
            VirtualPath::new(&path).map_err(|error| ExportError::Pdf(format!("bad asset path {path}: {error}")))?;
        let id = RootedPath::new(VirtualRoot::Project, virtual_path).intern();
        prepared.push(PreparedAsset { asset: asset.id, path, file: Some((id, Bytes::new(bytes))), size });
    }
    Ok(prepared)
}

impl ExportAsset {
    /// The preferred size in CSS pixels: explicit dimensions first, else the
    /// decoded raster, else nothing (an SVG carries its own size).
    fn size_in_pixels(&self) -> Option<(u32, u32)> {
        if let (Some(width), Some(height)) = (self.width, self.height)
            && width > 0
            && height > 0
        {
            return Some((width, height));
        }
        let reader = image::ImageReader::new(Cursor::new(&self.bytes)).with_guessed_format().ok()?;
        reader.into_dimensions().ok().filter(|(width, height)| *width > 0 && *height > 0)
    }
}

/// The file extension for an asset MIME type, when Typst can embed it.
fn asset_extension(asset: &ExportAsset) -> Option<&'static str> {
    match asset.mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        "image/bmp" => Some("png"),
        _ => None,
    }
}

/// Encodes a decoded image as PNG for the BMP transcode path.
fn encode_png(image: &image::DynamicImage) -> Result<Vec<u8>, ExportError> {
    let rgba = image.to_rgba8();
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(rgba.as_raw(), rgba.width(), rgba.height(), ExtendedColorType::Rgba8)
        .map_err(|error| ExportError::Pdf(format!("could not encode a picture as PNG: {error}")))?;
    Ok(bytes)
}

// ---- fonts --------------------------------------------------------------

/// Parses the caller fonts and appends the bundled fallback faces.
///
/// The returned vector and font book are parallel: index `n` in the book names
/// `fonts[n]`, which is what [`World::font`] relies on.
fn collect_fonts(supplied: &[PdfFont]) -> (Vec<Font>, FontBook) {
    let mut fonts = Vec::new();
    for font in supplied {
        let bytes = Bytes::new(font.bytes.clone());
        fonts.extend(Font::iter(bytes));
    }
    for bytes in typst_assets::fonts() {
        fonts.extend(Font::iter(Bytes::new(bytes)));
    }
    let book = FontBook::from_fonts(fonts.iter());
    (fonts, book)
}

// ---- Typst world --------------------------------------------------------

struct PdfWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    main: FileId,
    source: Source,
    files: HashMap<FileId, Bytes>,
    fonts: Vec<Font>,
}

impl World for PdfWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main { Ok(self.source.clone()) } else { Err(FileError::NotFound(PathBuf::from("main.typ"))) }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.get(&id).cloned().ok_or_else(|| FileError::NotFound(PathBuf::from("asset")))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<typst::foundations::Duration>) -> Option<Datetime> {
        None
    }
}

// ---- source generation --------------------------------------------------

/// The heading ids that have a stable PDF label, in document order.
///
/// Only top-level headings get labels; a TOC entry whose heading is nested in
/// a quote or list item is still listed, just not clickable.
fn heading_labels(doc: &ExportDoc) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    for (index, block) in doc.blocks.iter().enumerate() {
        if let ExportBlock::Heading { id, .. } = block
            && !id.is_empty()
        {
            labels.entry(id.clone()).or_insert_with(|| format!("h-{index}"));
        }
    }
    labels
}

fn render_source(
    doc: &ExportDoc,
    opts: &PdfOptions,
    assets: &[PreparedAsset],
    labels: &BTreeMap<String, String>,
    has_footnotes: bool,
) -> String {
    let mut writer = Writer {
        doc,
        opts,
        assets,
        labels,
        has_footnotes,
        out: String::new(),
        h1_seen: false,
        document_labels: ExportLabels::for_lang(&doc.lang),
        table_inset_pt: 0.0,
        table_cell_scale: false,
    };
    writer.document();
    writer.out
}

struct Writer<'a> {
    doc: &'a ExportDoc,
    opts: &'a PdfOptions,
    assets: &'a [PreparedAsset],
    labels: &'a BTreeMap<String, String>,
    has_footnotes: bool,
    out: String,
    h1_seen: bool,
    document_labels: ExportLabels,
    /// Horizontal points already consumed by enclosing quotes, callouts and
    /// list bodies; a table nested in one has that much less width (X-12).
    table_inset_pt: f32,
    /// While generating a table cell function: sizes are multiplied by `f`
    /// and long code tokens may be split when its `b` flag is set.
    table_cell_scale: bool,
}

impl Writer<'_> {
    fn document(&mut self) {
        self.preamble();
        let doc = self.doc;
        if doc.blocks.is_empty() {
            // An empty document still prints one page with its header/footer.
            self.out.push_str("#box(width: 0pt, height: 0pt)\n");
            return;
        }
        for block in &doc.blocks {
            self.block(block, true);
        }
    }

    fn preamble(&mut self) {
        let body = self.opts.resolved_body_font();
        let code = self.opts.resolved_code_font();
        let size = self.opts.body_size_pt;
        let page = self.opts.page;
        let lang = language(&self.doc.lang);

        let header = header_text(self.doc, self.opts);
        let _ = writeln!(
            self.out,
            "#let lmdv-footer = context align(center, text(size: 8pt, fill: rgb(\"{MUTED}\"), counter(page).display(\"1 / 1\", both: true)))"
        );
        let _ = writeln!(self.out, "#set document(title: {}, date: none)", string(&self.doc.title));
        let _ = write!(
            self.out,
            "#set page(paper: \"a4\", width: {}mm, height: {}mm, margin: (top: {}mm, right: {}mm, bottom: {}mm, left: {}mm), footer: lmdv-footer",
            mm(page.width_mm),
            mm(page.height_mm),
            mm(page.top_mm),
            mm(page.right_mm),
            mm(page.bottom_mm),
            mm(page.left_mm)
        );
        if !header.is_empty() {
            let _ = write!(
                self.out,
                ", header: align(center, text(size: 8pt, fill: rgb(\"{MUTED}\"), {}))",
                string(&header)
            );
        }
        self.out.push_str(")\n");
        let _ = writeln!(
            self.out,
            "#set text(font: {}, size: {}pt, fill: rgb(\"{TEXT}\"), lang: {})",
            string(body),
            pt(size),
            string(lang)
        );
        self.out.push_str("#set par(leading: 0.7em, spacing: 1.0em, justify: false)\n");
        self.out.push_str("#set heading(numbering: none)\n#show heading: set block(above: 1.2em, below: 0.55em)\n");
        for level in 1..=6 {
            let (factor, fill) = match level {
                1 => (2.0, TEXT),
                2 => (1.5, TEXT),
                3 => (1.25, TEXT),
                4 => (1.0, TEXT),
                5 => (0.875, TEXT),
                _ => (0.85, MUTED),
            };
            let _ = writeln!(
                self.out,
                "#show heading.where(level: {level}): set text(size: {}pt, fill: rgb(\"{fill}\"))",
                pt(size * factor)
            );
        }
        let _ = writeln!(self.out, "#show raw: set text(font: {}, size: 0.8em, fill: rgb(\"{TEXT}\"))", string(code));
        self.out.push_str("#set list(indent: 1.1em, body-indent: 0.45em, spacing: 0.4em)\n");
        self.out.push_str("#set enum(indent: 1.1em, body-indent: 0.45em, spacing: 0.4em)\n");
        self.out.push('\n');
    }

    /// Runs `f` while collecting its output, then restores the outer buffer.
    fn capture(&mut self, f: impl FnOnce(&mut Self)) -> String {
        let outer = std::mem::take(&mut self.out);
        f(self);
        std::mem::replace(&mut self.out, outer)
    }

    /// Markup for a sequence of blocks, for use inside a `[...]` content block.
    fn content(&mut self, blocks: &[ExportBlock]) -> String {
        self.content_inset(blocks, 0.0)
    }

    /// Like [`Writer::content`], for blocks nested in a box whose inset leaves
    /// `extra_pt` less horizontal room (X-12 table fitting).
    fn content_inset(&mut self, blocks: &[ExportBlock], extra_pt: f32) -> String {
        let previous = self.table_inset_pt;
        self.table_inset_pt = previous + extra_pt;
        let doc_blocks = blocks;
        let mut body = self.capture(|writer| {
            for block in doc_blocks {
                writer.block(block, false);
            }
        });
        self.table_inset_pt = previous;
        while body.ends_with("\n\n\n") {
            body.pop();
        }
        body
    }

    /// The width a table may occupy here, in points.
    fn table_available_pt(&self) -> f32 {
        (self.opts.page.content_width_pt() - self.table_inset_pt).max(24.0)
    }

    fn block(&mut self, block: &ExportBlock, top: bool) {
        match block {
            ExportBlock::Heading { level, id, runs } => {
                let level = (*level).clamp(1, 6);
                if top && self.opts.break_on_h1 && level == 1 {
                    if self.h1_seen {
                        self.out.push_str("#pagebreak(weak: true)\n");
                    }
                    self.h1_seen = true;
                }
                let _ = write!(self.out, "#heading(level: {level})[");
                self.runs(runs);
                self.out.push(']');
                if top && let Some(label) = self.labels.get(id) {
                    let _ = write!(self.out, " <{label}>");
                }
                self.out.push_str("\n\n");
            }
            ExportBlock::Para(runs) => {
                if runs.is_empty() {
                    return;
                }
                self.runs(runs);
                self.out.push_str("\n\n");
            }
            ExportBlock::List(list) => {
                let expression = self.list_expression(list);
                let _ = writeln!(self.out, "#{expression}");
                self.out.push('\n');
            }
            ExportBlock::Quote { callout: None, blocks } => {
                let body = self.content_inset(blocks, 20.0);
                let _ = writeln!(
                    self.out,
                    "#block(width: 100%, breakable: false, inset: (left: 12pt, right: 8pt, top: 6pt, bottom: 6pt), stroke: (left: 2.5pt + rgb(\"{BORDER}\")), above: 8pt, below: 8pt)[#set text(fill: rgb(\"{MUTED}\"))\n{body}]"
                );
                self.out.push('\n');
            }
            ExportBlock::Quote { callout: Some(callout), blocks } => {
                let (border, fill) = callout_colors(callout.kind);
                let title =
                    callout.title.clone().unwrap_or_else(|| self.document_labels.callout(callout.kind).to_owned());
                let body = self.content_inset(blocks, 22.0);
                let _ = writeln!(
                    self.out,
                    "#block(width: 100%, breakable: false, fill: rgb(\"{fill}\"), inset: (left: 12pt, right: 10pt, top: 8pt, bottom: 8pt), stroke: (left: 2.5pt + rgb(\"{border}\")), radius: 3pt, above: 10pt, below: 10pt)[#text(weight: \"bold\", fill: rgb(\"{border}\"), {})\n\n{body}]",
                    string(&title)
                );
                self.out.push('\n');
            }
            ExportBlock::Code { lang, text } => {
                if text.is_empty() {
                    return;
                }
                let text = soft_wrap_code(text);
                let raw = match lang.as_deref().filter(|lang| !lang.is_empty()) {
                    Some(lang) => format!("raw({}, block: true, lang: {})", string(&text), string(lang)),
                    None => format!("raw({}, block: true)", string(&text)),
                };
                let _ = writeln!(
                    self.out,
                    "#block(width: 100%, fill: rgb(\"{SURFACE}\"), stroke: 0.5pt + rgb(\"{BORDER}\"), inset: 8pt, radius: 3pt, above: 8pt, below: 8pt)[\n#{raw}\n]"
                );
                self.out.push('\n');
            }
            ExportBlock::Table(table) => {
                let expression = self.table_expression(table);
                let _ = writeln!(self.out, "#{expression}");
                self.out.push('\n');
            }
            ExportBlock::Math { tex, asset, .. } => {
                let expression = match asset {
                    Some(id) => self
                        .image_expression(*id, tex, false)
                        .unwrap_or_else(|| format!("raw({}, block: true)", string(tex))),
                    None => format!("raw({}, block: true)", string(tex)),
                };
                let _ = writeln!(self.out, "#block(width: 100%, above: 8pt, below: 8pt, align(center, {expression}))");
                self.out.push('\n');
            }
            ExportBlock::Image(image) => self.block_image(image),
            ExportBlock::Diagram { source, asset } => match asset {
                Some(id) => self.block_image(&ExportImage {
                    src: String::new(),
                    alt: self.document_labels.mermaid.clone(),
                    title: None,
                    asset: Some(*id),
                }),
                None => self.block(&ExportBlock::Code { lang: Some("mermaid".to_owned()), text: source.clone() }, top),
            },
            ExportBlock::Rule => {
                self.out.push_str("#v(5pt)\n");
                let _ = writeln!(self.out, "#line(length: 100%, stroke: 0.6pt + rgb(\"{BORDER}\"))");
                self.out.push_str("#v(5pt)\n\n");
            }
            ExportBlock::Toc { entries } => {
                if entries.is_empty() {
                    return;
                }
                let list = self.toc_expression(entries);
                let _ = writeln!(
                    self.out,
                    "#block(width: 100%, fill: rgb(\"{SURFACE}\"), stroke: 0.5pt + rgb(\"{BORDER}\"), inset: 10pt, radius: 3pt, above: 8pt, below: 12pt)[#{list}]"
                );
                self.out.push('\n');
            }
            ExportBlock::Footnotes => self.footnotes(),
            ExportBlock::Html(node) => self.html_block(node, top),
        }
    }

    fn block_image(&mut self, image: &ExportImage) {
        let Some(id) = image.asset else {
            self.missing_image(image);
            return;
        };
        let Some(expression) = self.image_expression(id, &image.alt, false) else {
            self.missing_image(image);
            return;
        };
        let _ = writeln!(self.out, "#block(width: 100%, above: 10pt, below: 4pt, align(center, {expression}))");
        if let Some(title) = &image.title {
            let _ = writeln!(self.out, "#align(center, text(size: 9pt, fill: rgb(\"{MUTED}\"), {}))", string(title));
        }
        self.out.push('\n');
    }

    fn missing_image(&mut self, image: &ExportImage) {
        let fallback = if !image.alt.is_empty() {
            image.alt.as_str()
        } else if !image.src.is_empty() {
            image.src.as_str()
        } else {
            self.document_labels.image.as_str()
        };
        let _ = writeln!(self.out, "#text(fill: rgb(\"{MUTED}\"))[#emph({})]\n", string(fallback));
    }

    fn footnotes(&mut self) {
        let doc = self.doc;
        if doc.footnotes.is_empty() {
            return;
        }
        let mut body = String::new();
        for footnote in &doc.footnotes {
            let inner = self.content(&footnote.blocks);
            let number = footnote.number;
            let _ = writeln!(
                body,
                "#block(width: 100%, above: 4pt, below: 2pt)[#text(weight: \"bold\", {})#h(4pt){inner}] <fn-{number}>",
                string(&format!("{number}."))
            );
            body.push('\n');
        }
        let _ = writeln!(
            self.out,
            "#block(width: 100%, above: 16pt, below: 6pt)[#line(length: 100%, stroke: 0.5pt + rgb(\"{BORDER}\"))\n#set text(size: 9pt, fill: rgb(\"{MUTED}\"))\n{body}]"
        );
        self.out.push('\n');
    }

    // ---- inline -------------------------------------------------------

    fn runs(&mut self, runs: &[ExportRun]) {
        for run in runs {
            let expression = self.run(run);
            if expression.is_empty() {
                continue;
            }
            self.out.push('#');
            self.out.push_str(&expression);
        }
    }

    fn run(&self, run: &ExportRun) -> String {
        match run {
            ExportRun::Text { text, style, link } => {
                let body = self.styled_text(text, *style);
                self.with_link(link, body)
            }
            ExportRun::Math { tex, asset, link, .. } => {
                let body = match asset {
                    Some(id) => {
                        self.image_expression(*id, tex, true).unwrap_or_else(|| format!("raw({})", string(tex)))
                    }
                    None => format!("raw({})", string(tex)),
                };
                self.with_link(link, body)
            }
            ExportRun::Image { image, link } => {
                let body = match image.asset {
                    Some(id) => self.image_expression(id, &image.alt, true).unwrap_or_else(|| {
                        format!("text({})", string(&image_fallback(image, &self.document_labels.image)))
                    }),
                    None => format!("text({})", string(&image_fallback(image, &self.document_labels.image))),
                };
                self.with_link(link, body)
            }
            ExportRun::Break => "linebreak()".to_owned(),
            ExportRun::FootnoteRef { number, .. } => {
                if self.has_footnotes {
                    format!("link(<fn-{number}>, super({}))", string(&number.to_string()))
                } else {
                    format!("super({})", string(&number.to_string()))
                }
            }
            ExportRun::Html(node) => self.html_expression(node),
        }
    }

    fn styled_text(&self, text: &str, style: RunStyle) -> String {
        let mut body = if style.code {
            if self.table_cell_scale {
                // The fitting fallback flips `b` on when the table is below
                // the 0.3 floor; every character may then break.
                format!(
                    "if b {{ raw({}) }} else {{ raw({}) }}",
                    string(&soft_wrap_code_at(text, 1)),
                    string(&soft_wrap_code(text))
                )
            } else {
                format!("raw({})", string(&soft_wrap_code(text)))
            }
        } else {
            format!("text({})", string(text))
        };
        if style.strong {
            body = format!("strong({body})");
        }
        if style.em {
            body = format!("emph({body})");
        }
        if style.strike {
            body = format!("strike({body})");
        }
        if style.mark {
            body = format!("highlight(fill: rgb(\"{MARK}\"), {body})");
        }
        if style.sup {
            body = format!("super({body})");
        } else if style.sub {
            body = format!("sub({body})");
        }
        if style.underline {
            body = format!("underline({body})");
        }
        if style.kbd {
            body = format!(
                "box(fill: rgb(\"#eff1f3\"), stroke: 0.5pt + rgb(\"{BORDER}\"), inset: (x: 2.5pt, y: 0pt), radius: 1.5pt, {body})"
            );
        }
        body
    }

    fn with_link(&self, link: &Option<ExportLink>, body: String) -> String {
        let Some(link) = link else { return body };
        if let Some(id) = link.href.strip_prefix('#') {
            return match self.labels.get(id) {
                Some(label) => format!("link(<{label}>, {body})"),
                None => body,
            };
        }
        if !href_is_safe(&link.href) {
            return body;
        }
        format!("link({}, {body})", string(&link.href))
    }

    /// An `image(...)` expression for an asset, scaled to the printable width.
    ///
    /// `alt` is the accessible description (the TeX for a formula). The
    /// baseline shift of an inline formula is applied by the caller through
    /// the asset's descent.
    fn image_expression(&self, id: AssetId, alt: &str, inline: bool) -> Option<String> {
        self.doc.asset(id)?;
        let prepared = self.assets.get(id.index())?;
        if prepared.asset != id {
            return None;
        }
        prepared.file.as_ref()?;
        let path = string(&prepared.path);
        let Some((width, height)) = prepared.size else {
            // An SVG without a raster size: let it use the column width as a
            // block, and its own intrinsic size inline. Inside a fitted table
            // the inline picture has to follow the table scale; a reflowing
            // visual transform is the only way to shrink an intrinsically sized
            // asset (X-12).
            return Some(if inline {
                if self.table_cell_scale {
                    format!("box(scale(f, reflow: true, image({path}, alt: {})))", string(alt))
                } else {
                    format!("image({path}, alt: {})", string(alt))
                }
            } else {
                format!("image({path}, width: 100%, alt: {})", string(alt))
            });
        };
        let max_width = self.opts.page.content_width_pt();
        let mut width_pt = width as f32 * 0.75;
        let mut height_pt = height as f32 * 0.75;
        let mut scale = 1.0;
        if width_pt > max_width {
            scale = max_width / width_pt;
            width_pt *= scale;
            height_pt *= scale;
        }
        let mut expression = format!(
            "image({path}, width: {}, height: {}, alt: {})",
            self.dimension(width_pt),
            self.dimension(height_pt),
            string(alt)
        );
        if inline
            && let Some(asset) = self.doc.asset(id)
            && let Some(baseline) = asset.baseline.filter(|baseline| *baseline > 0)
        {
            expression = format!("box(baseline: {}, {expression})", self.dimension(baseline as f32 * scale * 0.75));
        }
        Some(expression)
    }

    /// A length that follows the table scale when one is in scope.
    fn dimension(&self, value: f32) -> String {
        if self.table_cell_scale { format!("{}pt * f", pt(value)) } else { format!("{}pt", pt(value)) }
    }

    // ---- structure ----------------------------------------------------

    fn list_expression(&mut self, list: &ExportList) -> String {
        if list.items.is_empty() {
            return "none".to_owned();
        }
        let mut items = Vec::with_capacity(list.items.len());
        let body_indent = self.opts.body_size_pt * 1.55;
        for item in &list.items {
            let mut body = String::new();
            if let Some(checked) = item.task {
                body.push('#');
                body.push_str(&checkbox(checked));
                body.push_str("#h(3pt)");
            }
            body.push_str(&self.content_inset(&item.blocks, body_indent));
            items.push(format!("[{body}]"));
        }
        let tight = list.tight;
        if list.ordered {
            let start = if list.start == 1 { String::new() } else { format!(", start: {}", list.start) };
            format!("enum({}, tight: {tight}{start})", items.join(", "))
        } else {
            format!("list({}, tight: {tight})", items.join(", "))
        }
    }

    fn table_expression(&mut self, table: &crate::model::ExportTable) -> String {
        let columns = table.head.len().max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if columns == 0 {
            return "none".to_owned();
        }
        let available = self.table_available_pt();
        if self.table_fits_without_fitting(table, columns, available) {
            return self.plain_table_expression(table, columns);
        }
        self.fitted_table_expression(table, columns, available)
    }

    /// The unchanged table: auto columns, Typst's own wrapping and header
    /// repetition. Used when every column's longest word already fits its fair
    /// share of the paper, so no scaling can happen (X-12).
    fn plain_table_expression(&mut self, table: &crate::model::ExportTable, columns: usize) -> String {
        let aligns: Vec<&str> =
            (0..columns).map(|column| align_name(table.align.get(column).copied().unwrap_or_default())).collect();
        let mut parts = vec![
            format!("columns: {columns}"),
            format!("align: ({})", aligns.join(", ")),
            "inset: 5pt".to_owned(),
            format!("stroke: 0.5pt + rgb(\"{BORDER}\")"),
            format!(
                "fill: (_, y) => if y == 0 {{ rgb(\"{SURFACE}\") }} else if calc.odd(y) {{ rgb(\"{BAND}\") }} else {{ rgb(\"{WHITE}\") }}"
            ),
        ];
        if !table.head.is_empty() {
            let mut cells: Vec<String> = table.head.iter().map(|cell| cell_expression(self, cell)).collect();
            cells.resize(columns, "[]".to_owned());
            parts.push(format!("table.header({})", cells.join(", ")));
        }
        for row in &table.rows {
            let mut cells: Vec<String> = row.iter().map(|cell| cell_expression(self, cell)).collect();
            cells.resize(columns, "[]".to_owned());
            parts.extend(cells);
        }
        format!("table({})", parts.join(", "))
    }

    /// A table that may not fit the paper: measured at layout time against the
    /// width left on the page, then rendered either unchanged (it fits), shrunk
    /// like CSS `zoom` with proportional columns (X-12) or, below the 0.3 floor,
    /// with equal columns and break opportunities inside every word.
    fn fitted_table_expression(&mut self, table: &crate::model::ExportTable, columns: usize, available: f32) -> String {
        let aligns: Vec<&str> =
            (0..columns).map(|column| align_name(table.align.get(column).copied().unwrap_or_default())).collect();

        // One reusable content value per cell, so the probe and the two table
        // functions do not duplicate the generated text.
        let mut definitions = String::new();
        let mut probes = String::new();
        let mut header_cells = Vec::with_capacity(columns);
        let mut body_rows: Vec<Vec<String>> = Vec::with_capacity(table.rows.len());
        for _ in &table.rows {
            body_rows.push(Vec::with_capacity(columns));
        }
        for column in 0..columns {
            let mut column_cells: Vec<&[ExportRun]> = Vec::new();
            if let Some(cell) = table.head.get(column) {
                let name = format!("c{column}_h");
                let body = self.table_cell_body(cell);
                definitions.push_str(&format!("let {name} = (f, b) => [{body}]\n"));
                column_cells.push(cell.as_slice());
                header_cells.push(format!("[#{name}(f, false)]"));
            } else if !table.head.is_empty() {
                header_cells.push("[]".to_owned());
            }
            for (row_index, row) in table.rows.iter().enumerate() {
                match row.get(column) {
                    Some(cell) => {
                        let name = format!("c{column}_r{row_index}");
                        let body = self.table_cell_body(cell);
                        definitions.push_str(&format!("let {name} = (f, b) => [{body}]\n"));
                        column_cells.push(cell.as_slice());
                        body_rows[row_index].push(format!("[#{name}(f, false)]"));
                    }
                    None => body_rows[row_index].push("[]".to_owned()),
                }
            }
            let probe = self.table_probe_body(&column_cells);
            probes.push_str(&format!("let p{column} = (f, b) => [{probe}]\n"));
        }

        let header =
            if table.head.is_empty() { String::new() } else { format!("table.header({})", header_cells.join(", ")) };
        let body_cells: Vec<String> = body_rows.iter().map(|row| row.join(", ")).collect();
        let mut args = vec![
            format!("align: ({})", aligns.join(", ")),
            "inset: 5pt * f".to_owned(),
            format!("stroke: 0.5pt * f + rgb(\"{BORDER}\")"),
            format!(
                "fill: (_, y) => if y == 0 {{ rgb(\"{SURFACE}\") }} else if calc.odd(y) {{ rgb(\"{BAND}\") }} else {{ rgb(\"{WHITE}\") }}"
            ),
        ];
        if !header.is_empty() {
            args.push(header);
        }
        for row in &body_cells {
            if !row.is_empty() {
                args.push(row.clone());
            }
        }
        let common_args = args.join(", ");
        let plain_call = format!("table(columns: {columns}, {common_args})");
        let squeeze_call = format!("table(columns: ws.map(w => w / total * 100%), {common_args})");
        // Below the floor a word may still be wider than its equal share: all
        // gaps and long code tokens become breakable, like overflow-wrap:anywhere.
        let fallback_args = common_args.replace("(f, false)", "(f, true)");
        let fallback_call = format!("table(columns: range({columns}).map(i => 1fr), {fallback_args})");

        let base = pt(self.opts.body_size_pt);
        let available = pt(available);
        let probe_calls: Vec<String> =
            (0..columns).map(|column| format!("measure(p{column}(1.0, false)).width")).collect();
        let probe_calls = if probe_calls.len() == 1 {
            format!("({},)", probe_calls[0])
        } else {
            format!("({})", probe_calls.join(", "))
        };

        // The helper is a local definition, not a page-level function, so no
        // scaffold reaches the rendered page or the PDF bytes of a fitting-free
        // document.
        // The percentage columns are the measured minimum widths scaled by the
        // same factor as the type, so the fit is exact; a hair of slack keeps
        // floating-point rounding from tipping a word over the edge, as the
        // original zoom does.
        format!(
            "{{\n{definitions}{probes}\
let plain = f => {{\n  set text(size: {base}pt * f)\n  {plain_call}\n}}\n\
let squeeze = (f, ws) => {{\n  set text(size: {base}pt * f)\n  let total = ws.sum()\n  {squeeze_call}\n}}\n\
let fallback = f => {{\n  set text(size: {base}pt * f)\n  show regex(\"[^\\\\s]+\"): it => it.text.clusters().join(\"\\u{{200b}}\")\n  {fallback_call}\n}}\n\
context {{\n  let mins = {probe_calls}\n  let ws = mins.map(m => m + 10pt)\n  let total = ws.sum()\n  let factor = calc.clamp({available}pt / total, 0.3, 1.0)\n  if factor >= 1.0 {{ plain(1.0) }} else if factor > 0.3 {{ squeeze(factor * 0.999, ws) }} else {{ fallback(0.3) }}\n}}\n}}"
        )
    }

    /// A cell's content, generated with the fitting scale in scope.
    fn table_cell_body(&mut self, runs: &[ExportRun]) -> String {
        let previous = self.table_cell_scale;
        self.table_cell_scale = true;
        let body = cell_body(self, runs);
        self.table_cell_scale = previous;
        body
    }

    /// Every unbreakable token of one column on its own line, so `measure`
    /// reports the column's narrowest layout (CSS min-content). Repeated tokens
    /// are measured once.
    fn table_probe_body(&mut self, cells: &[&[ExportRun]]) -> String {
        let previous = self.table_cell_scale;
        self.table_cell_scale = true;
        let mut seen = std::collections::HashSet::new();
        let mut body = String::new();
        for runs in cells {
            for run in *runs {
                for (key, expression) in self.probe_run_expressions(run) {
                    if expression.is_empty() || !seen.insert(key) {
                        continue;
                    }
                    body.push('#');
                    body.push_str(&expression);
                    body.push_str("#linebreak()");
                }
            }
        }
        self.table_cell_scale = previous;
        body
    }

    /// Width-only expressions for a run: links and marks are irrelevant here.
    fn probe_run_expressions(&self, run: &ExportRun) -> Vec<(String, String)> {
        match run {
            ExportRun::Text { text, style, .. } => text
                .split_whitespace()
                .map(|token| (format!("text:{style:?}:{token}"), self.styled_text(token, *style)))
                .collect(),
            ExportRun::Math { tex, asset, .. } => {
                let expression = match asset {
                    Some(id) => {
                        self.image_expression(*id, tex, true).unwrap_or_else(|| format!("raw({})", string(tex)))
                    }
                    None => format!("raw({})", string(tex)),
                };
                vec![(format!("math:{expression}"), expression)]
            }
            ExportRun::Image { image, .. } => {
                let expression = match image.asset {
                    Some(id) => self.image_expression(id, &image.alt, true).unwrap_or_else(|| {
                        format!("text({})", string(&image_fallback(image, &self.document_labels.image)))
                    }),
                    None => format!("text({})", string(&image_fallback(image, &self.document_labels.image))),
                };
                vec![(format!("image:{expression}"), expression)]
            }
            ExportRun::Break => Vec::new(),
            ExportRun::FootnoteRef { number, .. } => {
                let expression = format!("super({})", string(&number.to_string()));
                vec![(format!("note:{expression}"), expression)]
            }
            ExportRun::Html(node) => {
                let expression = self.html_expression(node);
                if expression.is_empty() { Vec::new() } else { vec![(format!("html:{expression}"), expression)] }
            }
        }
    }

    /// Whether every column's longest unbreakable token is certainly no wider
    /// than its fair share of the paper. The estimate is deliberately generous;
    /// a `false` answer only costs the measuring pass above.
    fn table_fits_without_fitting(&self, table: &crate::model::ExportTable, columns: usize, available: f32) -> bool {
        let fair = available / columns as f32;
        for column in 0..columns {
            let mut widest = 0.0_f32;
            let mut cells = Vec::with_capacity(1 + table.rows.len());
            if let Some(cell) = table.head.get(column) {
                cells.push(cell);
            }
            for row in &table.rows {
                if let Some(cell) = row.get(column) {
                    cells.push(cell);
                }
            }
            for cell in cells {
                for run in cell {
                    let Some(width) = self.run_min_width_estimate(run) else { return false };
                    widest = widest.max(width);
                }
            }
            // 2 × 5 pt inset, a 0.5 pt rule and rounding slack.
            if widest + 11.5 > fair {
                return false;
            }
        }
        true
    }

    fn run_min_width_estimate(&self, run: &ExportRun) -> Option<f32> {
        let em = self.opts.body_size_pt;
        match run {
            ExportRun::Text { text, .. } => {
                let characters = text.split_whitespace().map(|token| token.chars().count()).max().unwrap_or(0);
                Some(characters as f32 * em * 1.25)
            }
            ExportRun::Math { tex, asset, .. } => Some(
                asset.and_then(|id| self.asset_width_pt(id)).unwrap_or_else(|| tex.chars().count() as f32 * em * 1.25),
            ),
            ExportRun::Image { image, .. } => image.asset.and_then(|id| self.asset_width_pt(id)),
            ExportRun::Break => Some(0.0),
            ExportRun::FootnoteRef { .. } => Some(em),
            ExportRun::Html(_) => None,
        }
    }

    fn asset_width_pt(&self, id: AssetId) -> Option<f32> {
        let prepared = self.assets.get(id.index())?;
        if prepared.asset != id || prepared.file.is_none() {
            return None;
        }
        prepared.size.map(|(width, _)| width as f32 * 0.75)
    }

    fn toc_expression(&self, entries: &[ExportTocEntry]) -> String {
        let mut items = Vec::with_capacity(entries.len());
        for entry in entries {
            let label = self.labels.get(&entry.id);
            let text = match label {
                Some(label) => format!("link(<{label}>, text({}))", string(&entry.text)),
                None => format!("text({})", string(&entry.text)),
            };
            let body = if entry.children.is_empty() {
                format!("[#{text}]")
            } else {
                let children = self.toc_expression(&entry.children);
                format!("[#{text}#{children}]")
            };
            items.push(body);
        }
        format!("list({}, tight: true)", items.join(", "))
    }
}

/// The markup inside one table cell.
fn cell_body(writer: &Writer<'_>, runs: &[ExportRun]) -> String {
    let mut body = String::new();
    for run in runs {
        let expression = writer.run(run);
        if expression.is_empty() {
            continue;
        }
        body.push('#');
        body.push_str(&expression);
    }
    body
}

/// A content expression for one table cell.
fn cell_expression(writer: &Writer<'_>, runs: &[ExportRun]) -> String {
    format!("[{}]", cell_body(writer, runs))
}

/// Inserts zero-width break opportunities in code runs that exceed a line.
///
/// Typst wraps raw lines at spaces. A single token longer than the column (a
/// base64 blob, a minified line) would otherwise run into the margin, which is
/// what `overflow-wrap: anywhere` prevented in the HTML print sheet. The
/// zero-width spaces are invisible and keep every source character in place.
fn soft_wrap_code(text: &str) -> String {
    soft_wrap_code_at(text, 70)
}

/// [`soft_wrap_code`] with an explicit break interval, in characters.
///
/// A fitted table below its scale floor uses one character, so a token can
/// never be wider than the column (`overflow-wrap: anywhere`).
fn soft_wrap_code_at(text: &str, run_width: usize) -> String {
    let run_width = run_width.max(1);
    let mut out = String::with_capacity(text.len());
    let mut run = 0;
    for character in text.chars() {
        out.push(character);
        if character.is_whitespace() {
            run = 0;
        } else {
            run += 1;
            if run >= run_width {
                out.push('\u{200b}');
                run = 0;
            }
        }
    }
    out
}

/// The task checkbox, drawn so it does not depend on a font's glyph coverage.
fn checkbox(checked: bool) -> String {
    let fill = if checked { "text(size: 8pt, fill: rgb(\"#1a7f37\"), \"✓\")" } else { "" };
    format!("box(width: 7pt, height: 7pt, stroke: 0.6pt + rgb(\"{MUTED}\"), radius: 1pt, {fill})")
}

fn image_fallback(image: &ExportImage, image_label: &str) -> String {
    if !image.alt.is_empty() {
        image.alt.clone()
    } else if !image.src.is_empty() {
        image.src.clone()
    } else {
        image_label.to_owned()
    }
}

// ---- preserved HTML -----------------------------------------------------

/// One piece of a raw HTML block, in document order.
///
/// Most HTML maps onto [`ExportBlock`] and then goes through the same builders
/// the Markdown path uses. Two shapes need their own piece: a table with
/// `colspan`/`rowspan` (the shared [`ExportTable`] has no span slots) and a
/// `<details>` card (the export model has no details block).
enum HtmlPiece<'a> {
    /// A block the other writers already understand.
    Block(ExportBlock),
    /// A table with at least one spanning cell.
    Table(HtmlTable),
    /// A `<details>` element, kept for its titled-card layout.
    Details(&'a HtmlNode),
}

impl HtmlPiece<'_> {
    /// The block used when this piece is nested in a list item or quote, where
    /// no card can be opened: details keep their title and body as a quote.
    fn into_block(self, doc: &ExportDoc, labels: &ExportLabels) -> ExportBlock {
        match self {
            Self::Block(block) => block,
            Self::Table(table) => ExportBlock::Table(table.to_export_table()),
            Self::Details(node) => ExportBlock::Quote { callout: None, blocks: html_details_blocks(node, doc, labels) },
        }
    }
}

impl Writer<'_> {
    /// Lays out one raw HTML block from the allowed subset.
    ///
    /// Tables, lists, quotes, code, details and media keep their structure;
    /// inline tags become styled runs; tags the parser dropped have already
    /// degraded to their text.
    fn html_block(&mut self, node: &HtmlNode, top: bool) {
        match node.tag {
            HtmlTag::Details => self.html_details(node),
            HtmlTag::Table => match HtmlTable::from_node(node, self.doc) {
                Some(table) => self.html_table(&table),
                None => self.html_fallback_text(node),
            },
            _ => {
                let pieces = html_pieces(node, self.doc, &self.document_labels);
                for piece in &pieces {
                    self.html_piece(piece, top);
                }
            }
        }
    }

    fn html_piece(&mut self, piece: &HtmlPiece<'_>, top: bool) {
        match piece {
            HtmlPiece::Block(block) => self.block(block, top),
            HtmlPiece::Table(table) => self.html_table(table),
            HtmlPiece::Details(node) => self.html_details(node),
        }
    }

    /// A raw HTML table.
    ///
    /// Without spans it becomes an [`ExportTable`] and reuses the Markdown
    /// table builder — alignment, header repetition, zebra rows and the X-12
    /// wide-table fitting. A table with `colspan`/`rowspan` (or several header
    /// rows, which [`ExportTable`] cannot repeat) needs real Typst cells, so it
    /// gets the same look with explicit `table.cell` arguments.
    fn html_table(&mut self, table: &HtmlTable) {
        if table.needs_cells() {
            let expression = table.span_expression(self);
            let _ = writeln!(self.out, "#{expression}");
            self.out.push('\n');
        } else {
            self.block(&ExportBlock::Table(table.to_export_table()), false);
        }
    }

    /// A `<details>` card: a bold summary line and the body inside a tinted,
    /// bordered block. The print output always shows the body (there is no
    /// pointer to open a closed card).
    fn html_details(&mut self, node: &HtmlNode) {
        let Some(title) = html_details_title(node, &self.document_labels) else { return };
        let pieces = html_details_pieces(node, self.doc, &self.document_labels);
        let body = self.html_content(&pieces, 22.0);
        if body.is_empty() {
            let _ = writeln!(self.out, "#text(weight: \"bold\", {})\n", string(&title));
            return;
        }
        let _ = writeln!(
            self.out,
            "#block(width: 100%, fill: rgb(\"{SURFACE}\"), stroke: 0.5pt + rgb(\"{BORDER}\"), inset: (left: 12pt, right: 10pt, top: 8pt, bottom: 8pt), radius: 3pt, above: 10pt, below: 10pt)[#text(weight: \"bold\", {})\n\n{body}]",
            string(&title)
        );
        self.out.push('\n');
    }

    /// Captured markup for HTML pieces nested in a box that leaves `extra_pt`
    /// less horizontal room (a details body, a quote or a list item).
    fn html_content(&mut self, pieces: &[HtmlPiece<'_>], extra_pt: f32) -> String {
        let previous = self.table_inset_pt;
        self.table_inset_pt = previous + extra_pt;
        let body = self.capture(|writer| {
            for piece in pieces {
                writer.html_piece(piece, false);
            }
        });
        self.table_inset_pt = previous;
        let mut body = body;
        while body.ends_with("\n\n\n") {
            body.pop();
        }
        body
    }

    /// Plain text for an HTML block the structured path cannot use.
    fn html_fallback_text(&mut self, node: &HtmlNode) {
        let text = collapse_whitespace(&node.text());
        let text = text.trim();
        if !text.is_empty() {
            let _ = writeln!(self.out, "#text({})", string(text));
            self.out.push('\n');
        }
    }

    /// A content expression for an inline HTML subtree from the allowed subset.
    fn html_expression(&self, node: &HtmlNode) -> String {
        match node.tag {
            HtmlTag::Fragment | HtmlTag::Span | HtmlTag::Div | HtmlTag::Li | HtmlTag::Summary | HtmlTag::P => {
                html_children(self, node)
            }
            HtmlTag::Strong | HtmlTag::B => wrap("strong", html_children(self, node)),
            HtmlTag::Em | HtmlTag::I => wrap("emph", html_children(self, node)),
            HtmlTag::S | HtmlTag::Del => wrap("strike", html_children(self, node)),
            HtmlTag::Ins | HtmlTag::U => wrap("underline", html_children(self, node)),
            HtmlTag::Mark => {
                let children = html_children(self, node);
                if children.is_empty() {
                    String::new()
                } else {
                    format!("highlight(fill: rgb(\"{MARK}\"), {children})")
                }
            }
            HtmlTag::Sub => wrap("sub", html_children(self, node)),
            HtmlTag::Sup => wrap("super", html_children(self, node)),
            HtmlTag::Kbd => {
                let children = html_children(self, node);
                if children.is_empty() {
                    String::new()
                } else {
                    format!(
                        "box(fill: rgb(\"#eff1f3\"), stroke: 0.5pt + rgb(\"{BORDER}\"), inset: (x: 2.5pt, y: 0pt), radius: 1.5pt, {children})"
                    )
                }
            }
            HtmlTag::Code => format!("raw({})", string(&node.text())),
            HtmlTag::Pre => format!(
                "block(fill: rgb(\"{SURFACE}\"), inset: 6pt, radius: 3pt, raw({}, block: true))",
                string(&node.text())
            ),
            HtmlTag::Br => "linebreak()".to_owned(),
            HtmlTag::A => {
                let body = html_children(self, node);
                match node.attr("href").filter(|href| href_is_safe(href)) {
                    Some(href) if !body.is_empty() => format!("link({}, {body})", string(href)),
                    _ => body,
                }
            }
            HtmlTag::Img => self.html_image(node),
            HtmlTag::Picture => node
                .children
                .iter()
                .find_map(|child| match child {
                    HtmlChild::Node(child) if child.tag == HtmlTag::Img => Some(child),
                    _ => None,
                })
                .map(|image| self.html_image(image))
                .unwrap_or_else(|| html_children(self, node)),
            HtmlTag::Video | HtmlTag::Audio => self.html_media_expression(node),
            HtmlTag::Details => self.html_details_expression(node),
            HtmlTag::Source | HtmlTag::Track => String::new(),
            _ => html_children(self, node),
        }
    }

    fn html_image(&self, node: &HtmlNode) -> String {
        let alt = node.attr("alt").unwrap_or_default();
        if let Some(id) = node.attr("src").and_then(asset_id)
            && let Some(expression) = self.image_expression(id, alt, true)
        {
            return expression;
        }
        if alt.is_empty() { String::new() } else { format!("text({})", string(alt)) }
    }

    /// A media placeholder: the file name, linked only for a local file URL.
    ///
    /// A remote source never becomes a media element and never a link (a PDF
    /// is a static file); export never fetches it, and only the file name is
    /// written.
    fn html_media_expression(&self, node: &HtmlNode) -> String {
        let tag = node.tag.name();
        let src = node.attr("src").or_else(|| node.attr("data-lmdv-remote"));
        let label = src.and_then(media_name).unwrap_or_else(|| tag.to_owned());
        match src {
            Some(src) if src.starts_with("file:///") && href_is_safe(src) => {
                format!("link({}, text({}))", string(src), string(&label))
            }
            _ => format!("text({})", string(&label)),
        }
    }

    /// An inline `<details>`: a small titled box (a block inside a paragraph,
    /// which Typst lays out as its own row).
    fn html_details_expression(&self, node: &HtmlNode) -> String {
        let title = html_details_title(node, &self.document_labels);
        let mut body = Vec::new();
        for child in &node.children {
            match child {
                HtmlChild::Node(child) if child.tag == HtmlTag::Summary => {}
                HtmlChild::Text(text) => {
                    let text = collapse_whitespace(text);
                    let text = text.trim();
                    if !text.is_empty() {
                        body.push(format!("text({})", string(text)));
                    }
                }
                HtmlChild::Node(child) => {
                    let expression = self.html_expression(child);
                    if !expression.is_empty() {
                        body.push(expression);
                    }
                }
            }
        }
        let Some(title) = title else {
            // The inline form of a details element is only its opening tag; the
            // summary and body stay ordinary text around it.
            return String::new();
        };
        if body.is_empty() {
            return format!("text(weight: \"bold\", {})", string(&title));
        }
        format!(
            "block(width: 100%, fill: rgb(\"{SURFACE}\"), stroke: 0.5pt + rgb(\"{BORDER}\"), inset: (left: 10pt, right: 8pt, top: 6pt, bottom: 6pt), radius: 3pt, above: 6pt, below: 6pt)[#text(weight: \"bold\", {})\n\n#({})]",
            string(&title),
            body.join(" + ")
        )
    }
}

// ---- raw HTML structure -------------------------------------------------

/// A raw HTML table, with spans and per-cell alignment.
#[derive(Clone, Debug, Default)]
struct HtmlTable {
    /// The rows in document order.
    rows: Vec<HtmlRow>,
    /// The widest row, counting `colspan`.
    columns: usize,
    /// The leading rows that belong to a header.
    header_rows: usize,
    /// Whether any cell spans more than one column or row.
    has_spans: bool,
}

/// One raw HTML table row.
#[derive(Clone, Debug, Default)]
struct HtmlRow {
    /// Whether the row came from `<thead>` or is entirely `<th>`.
    header: bool,
    /// The cells in source order.
    cells: Vec<HtmlCell>,
}

/// One raw HTML table cell.
#[derive(Clone, Debug, Default)]
struct HtmlCell {
    /// `<th>`, or any cell in a header row.
    header: bool,
    /// The `align` attribute, when it names a known alignment.
    align: Align,
    /// The `colspan`, at least 1.
    colspan: usize,
    /// The `rowspan`, at least 1.
    rowspan: usize,
    /// The cell content, already flattened to styled runs.
    runs: Vec<ExportRun>,
}

impl HtmlTable {
    /// Parses a `<table>` subtree; `None` when it holds no cell.
    fn from_node(node: &HtmlNode, doc: &ExportDoc) -> Option<Self> {
        let mut raw = Vec::new();
        collect_html_rows(node, false, &mut raw);
        let mut table = Self::default();
        for (header_scope, row) in raw {
            let mut cells = Vec::new();
            for child in &row.children {
                let HtmlChild::Node(cell) = child else { continue };
                if !matches!(cell.tag, HtmlTag::Td | HtmlTag::Th) {
                    continue;
                }
                let header = header_scope || cell.tag == HtmlTag::Th;
                let style = RunStyle { strong: header, ..RunStyle::default() };
                let mut runs = Vec::new();
                html_children_runs(cell, doc, style, &None, &mut runs);
                trim_runs_edges(&mut runs);
                cells.push(HtmlCell {
                    header,
                    align: align_attr(cell),
                    colspan: span_attr(cell, "colspan"),
                    rowspan: span_attr(cell, "rowspan"),
                    runs,
                });
            }
            if cells.is_empty() {
                continue;
            }
            let header = header_scope || cells.iter().all(|cell| cell.header);
            table.has_spans |= cells.iter().any(|cell| cell.colspan > 1 || cell.rowspan > 1);
            table.rows.push(HtmlRow { header, cells });
        }
        if table.rows.is_empty() {
            return None;
        }
        table.columns = table
            .rows
            .iter()
            .map(|row| row.cells.iter().map(|cell| cell.colspan).sum::<usize>())
            .max()
            .unwrap_or(0)
            .max(1);
        table.header_rows = table.rows.iter().take_while(|row| row.header).count();
        Some(table)
    }

    /// Whether the table needs explicit Typst cells instead of the shared
    /// [`ExportTable`] model: real spans, or more than one header row (the
    /// model's header is a single row).
    fn needs_cells(&self) -> bool {
        self.has_spans || self.header_rows > 1
    }

    /// The model table the Markdown table builder consumes.
    ///
    /// Only used without spans; spanning cells are padded so the model stays a
    /// rectangular grid (nested contexts cannot carry the span graph).
    fn to_export_table(&self) -> ExportTable {
        let align = self.column_aligns();
        let (head, body) = if self.header_rows == 0 {
            (Vec::new(), self.rows.as_slice())
        } else {
            (self.export_row(&self.rows[0]), &self.rows[1..])
        };
        let rows = body.iter().map(|row| self.export_row(row)).collect();
        ExportTable { align, head, rows }
    }

    fn export_row(&self, row: &HtmlRow) -> Vec<Vec<ExportRun>> {
        let mut cells = Vec::with_capacity(self.columns);
        for cell in &row.cells {
            cells.push(cell.runs.clone());
            for _ in 1..cell.colspan {
                cells.push(Vec::new());
            }
        }
        cells.resize(self.columns, Vec::new());
        cells.truncate(self.columns);
        cells
    }

    /// The first explicit alignment seen at each column start.
    fn column_aligns(&self) -> Vec<Align> {
        let mut aligns = vec![Align::None; self.columns];
        for row in &self.rows {
            let mut column = 0;
            for cell in &row.cells {
                if column >= self.columns {
                    break;
                }
                if aligns[column] == Align::None && cell.align != Align::None {
                    aligns[column] = cell.align;
                }
                column += cell.colspan;
            }
        }
        aligns
    }

    /// A Typst table with real `table.cell(colspan: …, rowspan: …)` slots.
    ///
    /// Equal columns keep a wide raw table inside the printable width; the
    /// header rows repeat on every page they cover.
    fn span_expression(&self, writer: &Writer<'_>) -> String {
        let aligns = self.column_aligns();
        let align_names: Vec<&str> = aligns.iter().map(|align| align_name(*align)).collect();
        let mut parts = vec![
            format!("columns: range({}).map(i => 1fr)", self.columns),
            format!("align: ({})", align_names.join(", ")),
            "inset: 5pt".to_owned(),
            format!("stroke: 0.5pt + rgb(\"{BORDER}\")"),
            self.fill_expression(),
        ];
        if self.header_rows > 0 {
            let mut cells = Vec::new();
            for row in &self.rows[..self.header_rows] {
                cells.extend(self.span_cells(writer, row, &aligns, true));
            }
            parts.push(format!("table.header({})", cells.join(", ")));
        }
        for row in &self.rows[self.header_rows..] {
            parts.extend(self.span_cells(writer, row, &aligns, false));
        }
        format!("table({})", parts.join(", "))
    }

    fn fill_expression(&self) -> String {
        if self.header_rows == 0 {
            format!("fill: (_, y) => if calc.odd(y) {{ rgb(\"{BAND}\") }} else {{ rgb(\"{WHITE}\") }}")
        } else {
            let header = self.header_rows;
            format!(
                "fill: (_, y) => if y < {header} {{ rgb(\"{SURFACE}\") }} else if calc.odd(y) {{ rgb(\"{BAND}\") }} else {{ rgb(\"{WHITE}\") }}"
            )
        }
    }

    fn span_cells(&self, writer: &Writer<'_>, row: &HtmlRow, aligns: &[Align], in_header: bool) -> Vec<String> {
        let mut cells = Vec::with_capacity(row.cells.len());
        let mut column = 0;
        for cell in &row.cells {
            let mut args = Vec::new();
            if cell.colspan > 1 {
                args.push(format!("colspan: {}", cell.colspan));
            }
            if cell.rowspan > 1 {
                args.push(format!("rowspan: {}", cell.rowspan));
            }
            if cell.align != Align::None && aligns.get(column).copied() != Some(cell.align) {
                args.push(format!("align: {}", align_name(cell.align)));
            }
            if !in_header && cell.header {
                args.push(format!("fill: rgb(\"{SURFACE}\")"));
            }
            args.push(format!("[{}]", cell_body(writer, &cell.runs)));
            cells.push(format!("table.cell({})", args.join(", ")));
            column += cell.colspan;
        }
        cells
    }
}

/// Collects `(header_scope, tr)` pairs from a raw HTML table subtree.
fn collect_html_rows<'n>(node: &'n HtmlNode, header: bool, rows: &mut Vec<(bool, &'n HtmlNode)>) {
    for child in &node.children {
        let HtmlChild::Node(child) = child else { continue };
        match child.tag {
            HtmlTag::Tr => rows.push((header, child)),
            HtmlTag::Thead => collect_html_rows(child, true, rows),
            HtmlTag::Tbody | HtmlTag::Tfoot => collect_html_rows(child, false, rows),
            HtmlTag::Table | HtmlTag::Td | HtmlTag::Th => {}
            _ => collect_html_rows(child, header, rows),
        }
    }
}

/// Whether an allowed HTML element starts a block when it appears at block
/// level. Inline elements are grouped into a paragraph instead.
fn html_is_block(tag: HtmlTag) -> bool {
    matches!(
        tag,
        HtmlTag::Fragment
            | HtmlTag::Div
            | HtmlTag::P
            | HtmlTag::Details
            | HtmlTag::Blockquote
            | HtmlTag::Pre
            | HtmlTag::Table
            | HtmlTag::Ul
            | HtmlTag::Ol
            | HtmlTag::Li
            | HtmlTag::Summary
            | HtmlTag::Thead
            | HtmlTag::Tbody
            | HtmlTag::Tfoot
            | HtmlTag::Tr
    )
}

/// The pieces of a block-level HTML node.
fn html_pieces<'a>(node: &'a HtmlNode, doc: &ExportDoc, labels: &ExportLabels) -> Vec<HtmlPiece<'a>> {
    html_child_pieces(&node.children, false, doc, labels)
}

/// The pieces of a `<details>` body, skipping its summary.
fn html_details_pieces<'a>(node: &'a HtmlNode, doc: &ExportDoc, labels: &ExportLabels) -> Vec<HtmlPiece<'a>> {
    html_child_pieces(&node.children, true, doc, labels)
}

/// Splits a child list into paragraphs (runs between block elements) and
/// structured blocks. `skip_summary` drops a `<summary>` element.
fn html_child_pieces<'a>(
    children: &'a [HtmlChild],
    skip_summary: bool,
    doc: &ExportDoc,
    labels: &ExportLabels,
) -> Vec<HtmlPiece<'a>> {
    let mut pieces = Vec::new();
    let mut runs: Vec<ExportRun> = Vec::new();
    for child in children {
        match child {
            HtmlChild::Text(text) => {
                push_html_run(
                    &mut runs,
                    ExportRun::Text { text: collapse_whitespace(text), style: RunStyle::default(), link: None },
                );
            }
            HtmlChild::Node(child) if skip_summary && child.tag == HtmlTag::Summary => {}
            HtmlChild::Node(child) if html_is_block(child.tag) => {
                flush_html_runs(&mut pieces, &mut runs);
                pieces.extend(html_node_pieces(child, doc, labels));
            }
            HtmlChild::Node(child) => {
                html_runs_into(child, doc, RunStyle::default(), &None, &mut runs);
            }
        }
    }
    flush_html_runs(&mut pieces, &mut runs);
    pieces
}

/// The pieces of one block-level HTML node.
fn html_node_pieces<'a>(node: &'a HtmlNode, doc: &ExportDoc, labels: &ExportLabels) -> Vec<HtmlPiece<'a>> {
    match node.tag {
        HtmlTag::Fragment | HtmlTag::Div | HtmlTag::Thead | HtmlTag::Tbody | HtmlTag::Tfoot => {
            html_pieces(node, doc, labels)
        }
        HtmlTag::P | HtmlTag::Li => html_paragraph_pieces(node, doc, false),
        HtmlTag::Summary => html_paragraph_pieces(node, doc, true),
        HtmlTag::Details => vec![HtmlPiece::Details(node)],
        HtmlTag::Blockquote => vec![HtmlPiece::Block(ExportBlock::Quote {
            callout: None,
            blocks: html_blocks_from_children(node, doc, labels),
        })],
        HtmlTag::Pre => {
            let text = node.text();
            let text = text.strip_suffix('\n').unwrap_or(&text).to_owned();
            if text.is_empty() { Vec::new() } else { vec![HtmlPiece::Block(ExportBlock::Code { lang: None, text })] }
        }
        HtmlTag::Table => match HtmlTable::from_node(node, doc) {
            Some(table) => vec![HtmlPiece::Table(table)],
            None => html_text_piece(node),
        },
        HtmlTag::Ul | HtmlTag::Ol => vec![HtmlPiece::Block(ExportBlock::List(html_list(node, doc, labels)))],
        HtmlTag::Br | HtmlTag::Source | HtmlTag::Track => Vec::new(),
        _ => html_paragraph_pieces(node, doc, false),
    }
}

/// An inline element where a block was expected: one paragraph of its runs.
fn html_paragraph_pieces<'a>(node: &'a HtmlNode, doc: &ExportDoc, bold: bool) -> Vec<HtmlPiece<'a>> {
    let style = RunStyle { strong: bold, ..RunStyle::default() };
    let mut runs = Vec::new();
    html_runs_into(node, doc, style, &None, &mut runs);
    let mut pieces = Vec::new();
    flush_html_runs(&mut pieces, &mut runs);
    pieces
}

/// A plain paragraph for an empty or unparseable structure.
fn html_text_piece<'a>(node: &HtmlNode) -> Vec<HtmlPiece<'a>> {
    let text = collapse_whitespace(&node.text());
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    vec![HtmlPiece::Block(ExportBlock::Para(vec![ExportRun::Text {
        text: text.to_owned(),
        style: RunStyle::default(),
        link: None,
    }]))]
}

/// Converts the children of a container to export blocks.
fn html_blocks_from_children(node: &HtmlNode, doc: &ExportDoc, labels: &ExportLabels) -> Vec<ExportBlock> {
    html_blocks_from_pieces(html_pieces(node, doc, labels), doc, labels)
}

fn html_blocks_from_pieces(pieces: Vec<HtmlPiece<'_>>, doc: &ExportDoc, labels: &ExportLabels) -> Vec<ExportBlock> {
    pieces.into_iter().map(|piece| piece.into_block(doc, labels)).collect()
}

/// The fallback blocks of a nested details card: a bold title, then the body.
fn html_details_blocks(node: &HtmlNode, doc: &ExportDoc, labels: &ExportLabels) -> Vec<ExportBlock> {
    let Some(title) = html_details_title(node, labels) else { return Vec::new() };
    let mut blocks = vec![ExportBlock::Para(vec![ExportRun::Text {
        text: title,
        style: RunStyle { strong: true, ..RunStyle::default() },
        link: None,
    }])];
    blocks.extend(html_blocks_from_pieces(html_details_pieces(node, doc, labels), doc, labels));
    blocks
}

fn html_summary(node: &HtmlNode) -> Option<&HtmlNode> {
    node.children.iter().find_map(|child| match child {
        HtmlChild::Node(child) if child.tag == HtmlTag::Summary => Some(child),
        _ => None,
    })
}

/// The details card title: the summary's text, else the localized “Details”
/// label when there is a body. `None` for an empty element (an inline details
/// opening tag is only a tag).
fn html_details_title(node: &HtmlNode, labels: &ExportLabels) -> Option<String> {
    if let Some(summary) = html_summary(node) {
        let text = collapse_whitespace(&summary.text());
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    let has_content = node.children.iter().any(|child| match child {
        HtmlChild::Node(_) => true,
        HtmlChild::Text(text) => !text.trim().is_empty(),
    });
    has_content.then(|| labels.details.clone())
}

/// A raw HTML list, reusing the Markdown list builder for items and nesting.
fn html_list(node: &HtmlNode, doc: &ExportDoc, labels: &ExportLabels) -> ExportList {
    let ordered = node.tag == HtmlTag::Ol;
    let start = node
        .attr("start")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|start| *start >= 1)
        .map_or(1, |start| start as u64);
    let mut items = Vec::new();
    for child in &node.children {
        let HtmlChild::Node(child) = child else { continue };
        if child.tag != HtmlTag::Li {
            continue;
        }
        let blocks = html_blocks_from_children(child, doc, labels);
        items.push(ExportItem { task: None, label: html_item_label(&blocks), blocks });
    }
    ExportList { ordered, start, tight: true, items }
}

/// A list item's accessible label, capped like the Markdown builder's.
fn html_item_label(blocks: &[ExportBlock]) -> String {
    let mut text = String::new();
    for block in blocks {
        if let ExportBlock::Para(runs) = block {
            text.push_str(&runs_text(runs));
            text.push(' ');
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > 120 {
        let cut: String = text.chars().take(119).collect();
        format!("{}…", cut.trim_end())
    } else {
        text
    }
}

fn runs_text(runs: &[ExportRun]) -> String {
    let mut text = String::new();
    for run in runs {
        match run {
            ExportRun::Text { text: value, .. } => text.push_str(value),
            ExportRun::Math { tex, .. } => text.push_str(tex),
            ExportRun::Image { image, .. } => text.push_str(&image.alt),
            ExportRun::Break => text.push(' '),
            ExportRun::FootnoteRef { number, .. } => text.push_str(&number.to_string()),
            ExportRun::Html(node) => text.push_str(&node.text()),
        }
    }
    text
}

// ---- HTML to runs -------------------------------------------------------

/// Flattens an inline HTML subtree into styled export runs.
fn html_runs_into(
    node: &HtmlNode,
    doc: &ExportDoc,
    style: RunStyle,
    link: &Option<ExportLink>,
    out: &mut Vec<ExportRun>,
) {
    match node.tag {
        HtmlTag::Strong | HtmlTag::B => {
            html_children_runs(node, doc, RunStyle { strong: true, ..style }, link, out);
        }
        HtmlTag::Em | HtmlTag::I => {
            html_children_runs(node, doc, RunStyle { em: true, ..style }, link, out);
        }
        HtmlTag::S | HtmlTag::Del => {
            html_children_runs(node, doc, RunStyle { strike: true, ..style }, link, out);
        }
        HtmlTag::Ins | HtmlTag::U => {
            html_children_runs(node, doc, RunStyle { underline: true, ..style }, link, out);
        }
        HtmlTag::Code => {
            html_children_runs(node, doc, RunStyle { code: true, ..style }, link, out);
        }
        HtmlTag::Kbd => {
            html_children_runs(node, doc, RunStyle { kbd: true, ..style }, link, out);
        }
        HtmlTag::Mark => {
            html_children_runs(node, doc, RunStyle { mark: true, ..style }, link, out);
        }
        HtmlTag::Sub => {
            html_children_runs(node, doc, RunStyle { sub: true, ..style }, link, out);
        }
        HtmlTag::Sup => {
            html_children_runs(node, doc, RunStyle { sup: true, ..style }, link, out);
        }
        HtmlTag::Br => out.push(ExportRun::Break),
        HtmlTag::A => {
            let next = match node.attr("href").filter(|href| !href.is_empty()) {
                Some(href) => Some(ExportLink {
                    href: href.to_owned(),
                    title: node.attr("title").map(str::to_owned).filter(|title| !title.is_empty()),
                }),
                None => link.clone(),
            };
            html_children_runs(node, doc, style, &next, out);
        }
        HtmlTag::Img => html_image_run(node, doc, link, out),
        HtmlTag::Picture => match node.children.iter().find_map(|child| match child {
            HtmlChild::Node(child) if child.tag == HtmlTag::Img => Some(child),
            _ => None,
        }) {
            Some(image) => html_image_run(image, doc, link, out),
            None => html_children_runs(node, doc, style, link, out),
        },
        HtmlTag::Video | HtmlTag::Audio => {
            let has_source = node.attr("src").is_some() || node.attr("data-lmdv-remote").is_some();
            if !has_source && !node.children.is_empty() {
                html_children_runs(node, doc, style, link, out);
            } else {
                push_html_run(out, ExportRun::Html(node.clone()));
            }
        }
        HtmlTag::Pre => {
            let text = node.text();
            if !text.is_empty() {
                push_html_run(
                    out,
                    ExportRun::Text { text, style: RunStyle { code: true, ..style }, link: link.clone() },
                );
            }
        }
        HtmlTag::Source | HtmlTag::Track => {}
        _ => html_children_runs(node, doc, style, link, out),
    }
}

fn html_children_runs(
    node: &HtmlNode,
    doc: &ExportDoc,
    style: RunStyle,
    link: &Option<ExportLink>,
    out: &mut Vec<ExportRun>,
) {
    for child in &node.children {
        match child {
            HtmlChild::Text(text) => {
                push_html_run(out, ExportRun::Text { text: collapse_whitespace(text), style, link: link.clone() })
            }
            HtmlChild::Node(child) => html_runs_into(child, doc, style, link, out),
        }
    }
}

fn html_image_run(node: &HtmlNode, doc: &ExportDoc, link: &Option<ExportLink>, out: &mut Vec<ExportRun>) {
    let alt = node.attr("alt").unwrap_or_default();
    if let Some(asset) = node.attr("src").and_then(asset_id)
        && doc.asset(asset).is_some()
    {
        push_html_run(
            out,
            ExportRun::Image {
                image: ExportImage {
                    src: String::new(),
                    alt: alt.to_owned(),
                    title: node.attr("title").map(str::to_owned).filter(|title| !title.is_empty()),
                    asset: Some(asset),
                },
                link: link.clone(),
            },
        );
        return;
    }
    if !alt.is_empty() {
        push_html_run(out, ExportRun::Text { text: alt.to_owned(), style: RunStyle::default(), link: link.clone() });
    }
}

/// Appends a run, merging adjacent text with the same style and link.
fn push_html_run(runs: &mut Vec<ExportRun>, run: ExportRun) {
    match run {
        ExportRun::Text { text, style, link } => {
            if text.is_empty() {
                return;
            }
            if let Some(ExportRun::Text { text: last, style: last_style, link: last_link }) = runs.last_mut()
                && *last_style == style
                && *last_link == link
            {
                last.push_str(&text);
                return;
            }
            runs.push(ExportRun::Text { text, style, link });
        }
        other => runs.push(other),
    }
}

/// Ends the current inline run and appends it as a paragraph, trimming the
/// whitespace that belonged between block elements.
fn flush_html_runs<'a>(pieces: &mut Vec<HtmlPiece<'a>>, runs: &mut Vec<ExportRun>) {
    if runs.is_empty() {
        return;
    }
    let mut runs = std::mem::take(runs);
    trim_runs_edges(&mut runs);
    if !runs.is_empty() {
        pieces.push(HtmlPiece::Block(ExportBlock::Para(runs)));
    }
}

/// Trims HTML's formatting whitespace from the edges of a run list.
fn trim_runs_edges(runs: &mut Vec<ExportRun>) {
    if let Some(ExportRun::Text { text, .. }) = runs.first_mut() {
        *text = text.trim_start().to_owned();
    }
    if let Some(ExportRun::Text { text, .. }) = runs.last_mut() {
        *text = text.trim_end().to_owned();
    }
    runs.retain(|run| !matches!(run, ExportRun::Text { text, .. } if text.is_empty()));
}

fn asset_id(src: &str) -> Option<AssetId> {
    src.strip_prefix("lmdv-asset:").and_then(|id| id.parse::<u32>().ok()).map(AssetId)
}

fn align_attr(node: &HtmlNode) -> Align {
    match node.attr("align").map(str::to_ascii_lowercase).as_deref() {
        Some("left") => Align::Left,
        Some("center") => Align::Center,
        Some("right") => Align::Right,
        _ => Align::None,
    }
}

fn span_attr(node: &HtmlNode, name: &str) -> usize {
    node.attr(name).and_then(|value| value.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 1000)
}

fn align_name(align: Align) -> &'static str {
    match align {
        Align::Center => "center",
        Align::Right => "right",
        Align::Left | Align::None => "left",
    }
}

/// The last path segment of a media URL, for its placeholder text.
fn media_name(src: &str) -> Option<String> {
    let path = src.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?.rsplit('\\').next()?;
    let name = percent_encoding::percent_decode_str(name).decode_utf8_lossy();
    let name = name.trim();
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_owned())
}

fn html_children(writer: &Writer<'_>, node: &HtmlNode) -> String {
    let mut parts = Vec::new();
    for child in &node.children {
        match child {
            HtmlChild::Text(text) => {
                let text = collapse_whitespace(text);
                if !text.is_empty() {
                    parts.push(format!("text({})", string(&text)));
                }
            }
            HtmlChild::Node(child) => {
                let expression = writer.html_expression(child);
                if !expression.is_empty() {
                    parts.push(expression);
                }
            }
        }
    }
    match parts.len() {
        0 => String::new(),
        1 => parts.remove(0),
        _ => format!("({})", parts.join(" + ")),
    }
}

/// Wraps an HTML child expression, yielding nothing for an empty body.
fn wrap(function: &str, children: String) -> String {
    if children.is_empty() { String::new() } else { format!("{function}({children})") }
}

/// Collapses runs of HTML whitespace to one space, keeping edge spaces.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            out.push(character);
            space = false;
        }
    }
    if space && !out.is_empty() {
        out.push(' ');
    }
    if text.starts_with(char::is_whitespace) && !out.starts_with(' ') {
        out.insert(0, ' ');
    }
    out
}

// ---- helpers ------------------------------------------------------------

fn header_text(doc: &ExportDoc, opts: &PdfOptions) -> String {
    let date = opts.date.as_deref().unwrap_or("").trim();
    let title = doc.title.trim();
    match (title.is_empty(), date.is_empty()) {
        (true, true) => String::new(),
        (true, false) => date.to_owned(),
        (false, true) => title.to_owned(),
        (false, false) => format!("{title} · {date}"),
    }
}

/// The primary language subtag, when it is a plausible Typst language code.
fn language(lang: &str) -> &str {
    let primary = lang.split(['-', '_']).next().unwrap_or("").trim();
    if (2..=3).contains(&primary.len()) && primary.chars().all(|character| character.is_ascii_alphabetic()) {
        primary
    } else {
        "en"
    }
}

fn callout_colors(kind: CalloutKind) -> (&'static str, &'static str) {
    match kind {
        CalloutKind::Note => ("#0969da", "#ddf4ff"),
        CalloutKind::Tip => ("#1a7f37", "#dafbe1"),
        CalloutKind::Important => ("#8250df", "#fbefff"),
        CalloutKind::Warning => ("#9a6700", "#fff8c5"),
        CalloutKind::Caution => ("#cf222e", "#ffebe9"),
    }
}

/// Whether an already-resolved href may become a PDF link annotation.
///
/// A PDF has no document folder, so a relative destination would resolve
/// against the PDF's location: those stay plain text. Absolute web, mail and
/// `file:///` destinations are clickable; script and data schemes never are.
/// Internal heading links are handled by [`Writer::with_link`] before this.
fn href_is_safe(href: &str) -> bool {
    if href.is_empty() || href.chars().any(|character| character.is_control()) {
        return false;
    }
    if href.starts_with("file:///") {
        return true;
    }
    matches!(crate::link::scheme(href).map(str::to_ascii_lowercase).as_deref(), Some("http" | "https" | "mailto"))
}

/// A Typst string literal: the only place arbitrary document text enters the
/// generated source, so markup characters stay data.
fn string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => {}
            other if other.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", other as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// A point value with two decimals; deterministic and valid Typst.
fn pt(value: f32) -> String {
    format!("{value:.2}")
}

/// A millimetre value with two decimals.
fn mm(value: f32) -> String {
    format!("{value:.2}")
}

fn diagnostics(errors: &[SourceDiagnostic]) -> String {
    let messages: Vec<String> = errors.iter().take(3).map(|error| error.message.to_string()).collect();
    if messages.is_empty() { "no diagnostic".to_owned() } else { messages.join("; ") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExportOptions, NoAssets};

    fn source(markdown: &str) -> String {
        let doc = ExportDoc::from_md(&lmdv_md::parse(markdown), &ExportOptions::default(), &NoAssets)
            .expect("an export model");
        render_source(&doc, &PdfOptions::default(), &[], &BTreeMap::new(), false)
    }

    #[test]
    fn narrow_tables_keep_the_plain_expression() {
        let source = source("| A | B |\n| --- | --- |\n| 1 | 2 |\n");
        assert!(source.contains("#table(columns: 2,"), "the ordinary table changed shape:\n{source}");
        assert!(!source.contains("calc.clamp"), "an ordinary table must not be measured:\n{source}");
        assert!(!source.contains("show regex"), "an ordinary table must not install break rules:\n{source}");
    }

    #[test]
    fn a_table_that_measures_wider_than_its_estimate_still_measures() {
        // 4 × 10-character words exceed the conservative fair-share estimate,
        // yet fit the paper; the measured branch must then choose the plain,
        // unscaled table (verified byte-for-byte against the pre-fitting writer).
        let markdown = "| Alpha | Bravo | Charlie | Delta |\n| --- | --- | --- | --- |\n| abcdefghij | abcdefghij | abcdefghij | abcdefghij |\n";
        let source = source(markdown);
        assert!(source.contains("calc.clamp(504.57pt / total"), "the table should be measured:\n{source}");
        assert!(source.contains("if factor >= 1.0 { plain(1.0) }"), "{source}");
    }

    #[test]
    fn wide_tables_measure_then_scale_with_proportional_columns() {
        let head: Vec<String> = (1..=12).map(|column| format!("Header {column}")).collect();
        let separator: Vec<&str> = (1..=12)
            .map(|column| match column % 3 {
                0 => "---:",
                1 => ":---:",
                _ => ":---",
            })
            .collect();
        let row: Vec<String> = (1..=12).map(|column| format!("column {column} has several words")).collect();
        let markdown = format!("| {} |\n| {} |\n| {} |\n", head.join(" | "), separator.join(" | "), row.join(" | "));
        let source = source(&markdown);
        assert!(source.contains("calc.clamp(504.57pt / total, 0.3, 1.0)"), "{source}");
        assert!(source.contains("let mins = ("), "the table was not measured:\n{source}");
        assert!(source.contains("columns: ws.map(w => w / total * 100%)"), "columns are not proportional:\n{source}");
        assert!(source.contains("align: (center, left, right"), "column alignment was lost:\n{source}");
        assert!(source.contains("table.header("), "header repetition was lost:\n{source}");
        assert!(source.contains("rgb(\"#f6f8fa\")"), "header shading was lost:\n{source}");
        assert!(source.contains("calc.odd(y)"), "zebra rows were lost:\n{source}");
        assert!(source.contains("show regex(\"[^\\\\s]+\")"), "the fallback break rule is missing:\n{source}");
        assert!(source.contains("if factor >= 1.0 { plain(1.0) }"), "a fitting table cannot stay plain:\n{source}");
    }

    #[test]
    fn a_table_in_a_quote_measures_against_the_inner_width() {
        let head: Vec<String> = (1..=12).map(|column| format!("Header {column}")).collect();
        let separator: Vec<&str> = std::iter::repeat_n("---", 12).collect();
        let row: Vec<String> = (1..=12).map(|column| format!("column {column} has several words")).collect();
        let table = format!("| {} |\n| {} |\n| {} |\n", head.join(" | "), separator.join(" | "), row.join(" | "));
        let quoted = format!("> {}", table).replace('\n', "\n> ");
        let source = source(&quoted);
        // The quote insets 12 pt left and 8 pt right.
        assert!(source.contains("calc.clamp(484.57pt / total, 0.3, 1.0)"), "{source}");
    }

    #[test]
    fn the_fallback_can_break_between_every_character() {
        let broken = soft_wrap_code_at("abcdef", 1);
        assert_eq!(broken, "a\u{200b}b\u{200b}c\u{200b}d\u{200b}e\u{200b}f\u{200b}");
        assert_eq!(without_breaks(&broken), "abcdef");
        assert_eq!(soft_wrap_code_at("short", 70), "short");
        let long = soft_wrap_code_at(&"x".repeat(75), 70);
        assert_eq!(without_breaks(&long), "x".repeat(75));
        assert_eq!(long.matches('\u{200b}').count(), 1);
    }

    fn source_with(markdown: &str, opts: ExportOptions) -> String {
        let doc = ExportDoc::from_md(&lmdv_md::parse(markdown), &opts, &NoAssets).expect("an export model");
        render_source(&doc, &PdfOptions::default(), &[], &BTreeMap::new(), false)
    }

    #[test]
    fn raw_html_tables_reuse_the_markdown_table_builder() {
        let source = source(concat!(
            "<table>\n",
            "<thead><tr><th align=\"left\">Item name</th><th align=\"center\">Unit count</th>",
            "<th align=\"right\">Total cost</th></tr></thead>\n",
            "<tbody><tr><td>Alpha row</td><td>Twenty two</td><td>Thirty three</td></tr></tbody>\n",
            "</table>\n",
        ));
        assert!(source.contains("#table(columns: 3,"), "the raw table should use the shared builder:\n{source}");
        assert!(source.contains("align: (left, center, right)"), "cell alignment was lost:\n{source}");
        assert!(source.contains("table.header("), "the raw header should repeat:\n{source}");
        assert!(source.contains("strong(text(\"Item name\"))"), "header emphasis is missing:\n{source}");
        assert!(!source.contains("text(\"Item name\") + text(\"Unit count\")"), "the table was flattened:\n{source}");
        assert!(source.contains("text(\"Alpha row\")"), "a body cell was lost:\n{source}");
    }

    #[test]
    fn raw_html_spanning_cells_become_typst_cells() {
        let source = source(concat!(
            "<table>\n",
            "<tr><th>Span key</th><th>Span value</th></tr>\n",
            "<tr><td colspan=\"2\">Wide cell</td></tr>\n",
            "<tr><td rowspan=\"2\">Tall cell</td><td>First</td></tr>\n",
            "<tr><td>Second</td></tr>\n",
            "</table>\n",
        ));
        assert!(source.contains("columns: range(2).map(i => 1fr)"), "span columns are not fitted:\n{source}");
        assert!(source.contains("colspan: 2"), "colspan was lost:\n{source}");
        assert!(source.contains("rowspan: 2"), "rowspan was lost:\n{source}");
        assert!(source.contains("table.cell("), "the span table should use Typst cells:\n{source}");
        assert!(source.contains("table.header("), "the span header should repeat:\n{source}");
        assert!(source.contains("text(\"Wide cell\")"), "{source}");
        assert!(source.contains("text(\"First\")") && source.contains("text(\"Second\")"), "{source}");
    }

    #[test]
    fn raw_html_lists_reuse_the_list_builder() {
        let source = source(concat!(
            "<ul>\n",
            "<li>First bullet</li>\n",
            "<li>Second bullet<ol start=\"3\"><li>Nested third</li></ol></li>\n",
            "</ul>\n",
        ));
        assert!(source.contains("#list("), "the raw list should use the shared builder:\n{source}");
        assert!(source.contains("tight: true"), "{source}");
        assert!(source.contains("enum("), "the nested ordered list is missing:\n{source}");
        assert!(source.contains("start: 3"), "the ordered list start was lost:\n{source}");
        assert!(source.contains("text(\"Nested third\")"), "{source}");
    }

    #[test]
    fn raw_html_details_become_a_titled_block() {
        let source = source(
            "<details>\n<summary>More information</summary>\nBody text with <code>inline code</code>.\n</details>\n",
        );
        assert!(source.contains("weight: \"bold\""), "the details title is not emphasized:\n{source}");
        assert!(source.contains("\"More information\""), "the summary is missing:\n{source}");
        assert!(source.contains("text(\"Body text with \")"), "the details body is missing:\n{source}");
        assert!(source.contains("raw(\"inline code\")"), "the details body lost its inline code:\n{source}");
        assert!(source.contains("fill: rgb(\"#f6f8fa\")"), "the details card is not tinted:\n{source}");
    }

    #[test]
    fn a_details_without_a_summary_uses_the_localized_title() {
        let source = source_with(
            "<details><p>Body paragraph</p></details>\n",
            ExportOptions { lang: "it".into(), ..ExportOptions::default() },
        );
        assert!(
            source.contains("text(weight: \"bold\", \"Dettagli\")"),
            "the fallback title is not localized:\n{source}"
        );
        assert!(source.contains("text(\"Body paragraph\")"), "{source}");
    }

    #[test]
    fn raw_html_media_is_a_local_file_link_and_remote_is_only_a_name() {
        let source = source_with(
            concat!(
                "Before <video src=\"./media/clip.mp4\"></video> after\n\n",
                "<audio src=\"https://example.com/remote.mp3\"></audio>\n",
            ),
            ExportOptions { source_path: Some("/home/jane/docs/notes.md".into()), ..ExportOptions::default() },
        );
        assert!(
            source.contains("link(\"file:///home/jane/docs/media/clip.mp4\", text(\"clip.mp4\"))"),
            "local media should be a file-name link:\n{source}"
        );
        assert!(source.contains("text(\"remote.mp3\")"), "remote media should keep its file name:\n{source}");
        assert!(!source.contains("example.com"), "a remote media URL must never reach the PDF:\n{source}");
        assert!(!source.contains("text(\"video\")"), "the placeholder should be the file name, not the tag:\n{source}");
    }

    #[test]
    fn unknown_raw_tags_still_degrade_to_text() {
        let source = source("<custom-tag>Fallback paragraph</custom-tag>\n");
        assert!(source.contains("text(\"Fallback paragraph\")"), "an unknown tag dropped its text:\n{source}");
    }

    fn without_breaks(text: &str) -> String {
        text.chars().filter(|character| *character != '\u{200b}').collect()
    }
}
