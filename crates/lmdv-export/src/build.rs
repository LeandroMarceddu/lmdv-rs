//! Builds [`ExportDoc`] from the Rune-free [`lmdv_md::MdDoc`] model.
//!
//! This is where a document loses its parser-specific shape: inline text is
//! swept once at style/link/object boundaries, formulas are rasterized at 2×,
//! pictures and diagrams are resolved to bytes, footnote references are
//! numbered, and the table of contents is nested. The serializer never touches
//! a resolver or a renderer, so HTML, PDF and DOCX share exactly the same
//! content.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use base64::Engine as _;
use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::png::PngEncoder;
use lmdv_i18n::Key;
use lmdv_math::{MathRenderer, MathRequest, RenderedMath};
use lmdv_md::{
    Block, FmValue, HtmlChild, HtmlNode, HtmlTag, ImageBlock, Inline, InlineLink, InlineMath, InlineObject,
    InlineObjectKind, InlineStyle, MdDoc, Table,
};

use crate::model::{
    AssetId, AssetResolver, ExportAsset, ExportBlock, ExportCallout, ExportDoc, ExportError, ExportFootnote,
    ExportFrontMatter, ExportImage, ExportItem, ExportLink, ExportList, ExportOptions, ExportRun, ExportTable,
    ExportTocEntry, ExportWarning, ExportWarningKind, ExportedAsset, RunStyle,
};

/// Formula raster scale: twice the CSS pixel size, for crisp print and HiDPI.
const MATH_SCALE: f32 = 2.0;
/// The formula's em size in CSS pixels, matching the exported body text.
const MATH_SIZE: f32 = 16.0;
/// The formula ink, matching the light article text.
const MATH_COLOR: [u8; 4] = [31, 35, 40, 255];
/// The largest data URI image the exporter decodes on its own.
const MAX_DATA_IMAGE: usize = 8 * 1024 * 1024;

/// Builds the model.
pub(crate) fn build(doc: &MdDoc, opts: &ExportOptions, assets: &dyn AssetResolver) -> Result<ExportDoc, ExportError> {
    let mut builder = Builder::new(doc, opts, assets);
    let mut blocks = Vec::with_capacity(doc.blocks.len());
    let mut block_sources = Vec::with_capacity(doc.blocks.len());
    let mut has_toc = false;
    for (index, block) in doc.blocks.iter().enumerate() {
        if matches!(block, Block::Toc) {
            has_toc = true;
        }
        if let Some(block) = builder.block(block, index)? {
            blocks.push(block);
            // An empty span (the generated footnote section) is still a real
            // source position; a missing one means the model has no parse.
            block_sources.push(doc.block_spans.get(index).map(|span| span.source.clone()));
        }
    }
    if opts.include_toc && !has_toc {
        let entries = builder.toc_entries(None);
        if !entries.is_empty() {
            blocks.insert(0, ExportBlock::Toc { entries });
            block_sources.insert(0, None);
        }
    }
    let footnotes = builder.footnotes()?;
    let front_matter = if opts.front_matter { doc.front_matter.as_ref().map(front_matter) } else { None };
    let title = document_title(doc, opts);
    let lang = if opts.lang.trim().is_empty() { "en".to_owned() } else { opts.lang.trim().to_owned() };
    Ok(ExportDoc {
        title,
        lang,
        blocks,
        block_sources,
        footnotes,
        assets: builder.assets_out,
        front_matter,
        source_path: opts.source_path.clone(),
        warnings: builder.warnings,
    })
}

/// The localized fallback for a document with no file name or heading.
const PASTED_TEXT: Key = lmdv_i18n::key("paste.untitled");

/// The document title: explicit, file stem, first h1, localized fallback.
fn document_title(doc: &MdDoc, opts: &ExportOptions) -> String {
    if let Some(title) = opts.title.as_deref().map(str::trim).filter(|title| !title.is_empty()) {
        return title.to_owned();
    }
    if let Some(path) = &opts.source_path
        && let Some(stem) = path.file_stem()
    {
        let stem = stem.to_string_lossy();
        let stem = stem.trim();
        if !stem.is_empty() {
            return stem.to_owned();
        }
    }
    if let Some(heading) = doc.headings.iter().find(|heading| heading.level == 1 && !heading.text.trim().is_empty()) {
        return heading.text.trim().to_owned();
    }
    let locale = lmdv_i18n::negotiate(std::iter::once(opts.lang.as_str()));
    lmdv_i18n::text_in(locale, PASTED_TEXT).to_owned()
}

/// Converts the front matter to its flat export form.
fn front_matter(front: &lmdv_md::FrontMatter) -> ExportFrontMatter {
    let entries = front
        .entries
        .iter()
        .map(|(key, value)| {
            let values = match value {
                FmValue::Scalar(value) => vec![value.clone()],
                FmValue::List(values) => values.clone(),
            };
            (key.clone(), values)
        })
        .collect();
    ExportFrontMatter { raw: front.raw.clone(), valid: front.valid, entries }
}

/// A formula or an object that replaces part of the inline text.
#[derive(Clone, Copy)]
enum Piece<'a> {
    Math(&'a InlineMath),
    Object(&'a InlineObject),
}

impl<'a> Piece<'a> {
    fn range(self) -> &'a Range<usize> {
        match self {
            Self::Math(math) => &math.range,
            Self::Object(object) => &object.range,
        }
    }
}

/// The converter's mutable state: asset caches and reference counters.
struct Builder<'a> {
    doc: &'a MdDoc,
    opts: &'a ExportOptions,
    resolver: &'a dyn AssetResolver,
    math: MathRenderer,
    math_cache: HashMap<(String, bool), Option<AssetId>>,
    image_cache: HashMap<String, Option<AssetId>>,
    diagram_cache: HashMap<String, Option<AssetId>>,
    ref_counts: HashMap<String, usize>,
    assets_out: Vec<ExportAsset>,
    warnings: Vec<ExportWarning>,
    warned: HashSet<String>,
}

impl<'a> Builder<'a> {
    fn new(doc: &'a MdDoc, opts: &'a ExportOptions, resolver: &'a dyn AssetResolver) -> Self {
        Self {
            doc,
            opts,
            resolver,
            math: MathRenderer::new(256),
            math_cache: HashMap::new(),
            image_cache: HashMap::new(),
            diagram_cache: HashMap::new(),
            ref_counts: HashMap::new(),
            assets_out: Vec::new(),
            warnings: Vec::new(),
            warned: HashSet::new(),
        }
    }

    /// One block; `top` is its top-level index, used by `[TOC]`.
    fn block(&mut self, block: &Block, top: usize) -> Result<Option<ExportBlock>, ExportError> {
        Ok(match block {
            Block::Para(inline) => {
                let runs = self.inline(inline, false)?;
                (!runs.is_empty()).then_some(ExportBlock::Para(runs))
            }
            Block::Heading { heading, inline } => {
                let model = self.doc.headings.get(*heading);
                let level = model.map(|heading| heading.level).unwrap_or(1).clamp(1, 6);
                let id = model.map(|heading| heading.id.clone()).unwrap_or_default();
                Some(ExportBlock::Heading { level, id, runs: self.inline(inline, false)? })
            }
            Block::Code { lang, text, .. } => Some(ExportBlock::Code { lang: lang.clone(), text: text.clone() }),
            Block::List { ordered, start, tight, items } => {
                Some(ExportBlock::List(self.list(*ordered, *start, *tight, items, top)?))
            }
            Block::Quote { blocks, callout } => Some(ExportBlock::Quote {
                callout: callout
                    .as_ref()
                    .map(|callout| ExportCallout { kind: callout.kind, title: callout.title.clone() }),
                blocks: self.blocks(blocks, top)?,
            }),
            Block::Table(table) => Some(ExportBlock::Table(self.table(table)?)),
            Block::Rule => Some(ExportBlock::Rule),
            Block::Mermaid { source, .. } => {
                Some(ExportBlock::Diagram { source: source.clone(), asset: self.diagram(source) })
            }
            Block::MathBlock { tex, .. } => {
                Some(ExportBlock::Math { tex: tex.clone(), display: true, asset: self.math(tex, true)? })
            }
            Block::HtmlBlock(node) => {
                let node = self.prepare_html(node)?;
                (!is_empty_html(&node)).then_some(ExportBlock::Html(node))
            }
            Block::Image(image) => self.image_from_block(image).map(ExportBlock::Image),
            Block::FootnoteDefs => Some(ExportBlock::Footnotes),
            Block::Toc => Some(ExportBlock::Toc { entries: self.toc_entries(Some(top)) }),
        })
    }

    fn blocks(&mut self, blocks: &[Block], top: usize) -> Result<Vec<ExportBlock>, ExportError> {
        let mut out = Vec::with_capacity(blocks.len());
        for block in blocks {
            if let Some(block) = self.block(block, top)? {
                out.push(block);
            }
        }
        Ok(out)
    }

    fn list(
        &mut self,
        ordered: bool,
        start: u64,
        tight: bool,
        items: &[Vec<Block>],
        top: usize,
    ) -> Result<ExportList, ExportError> {
        let mut out = Vec::with_capacity(items.len());
        for blocks in items {
            out.push(self.item(blocks, top)?);
        }
        Ok(ExportList { ordered, start, tight, items: out })
    }

    fn item(&mut self, blocks: &[Block], top: usize) -> Result<ExportItem, ExportError> {
        let task = first_task(blocks);
        let label = item_label(blocks);
        let mut out = Vec::with_capacity(blocks.len());
        for (index, block) in blocks.iter().enumerate() {
            if index == 0
                && let Block::Para(inline) = block
            {
                let runs = self.inline(inline, task.is_some())?;
                if !runs.is_empty() {
                    out.push(ExportBlock::Para(runs));
                }
                continue;
            }
            if let Some(block) = self.block(block, top)? {
                out.push(block);
            }
        }
        Ok(ExportItem { task, label, blocks: out })
    }

    fn table(&mut self, table: &Table) -> Result<ExportTable, ExportError> {
        let mut head = Vec::with_capacity(table.head.len());
        for cell in &table.head {
            head.push(self.inline(cell, false)?);
        }
        let mut rows = Vec::with_capacity(table.rows.len());
        for row in &table.rows {
            let mut cells = Vec::with_capacity(row.len());
            for cell in row {
                cells.push(self.inline(cell, false)?);
            }
            rows.push(cells);
        }
        Ok(ExportTable { align: table.align.clone(), head, rows })
    }

    fn footnotes(&mut self) -> Result<Vec<ExportFootnote>, ExportError> {
        let definitions: Vec<(String, usize, Vec<Block>)> = self
            .doc
            .footnotes
            .iter()
            .map(|footnote| (footnote.label.clone(), footnote.number, footnote.blocks.clone()))
            .collect();
        let mut out = Vec::with_capacity(definitions.len());
        for (label, number, blocks) in definitions {
            out.push(ExportFootnote { label, number, blocks: self.blocks(&blocks, usize::MAX)? });
        }
        Ok(out)
    }

    /// The nested table of contents. `after` limits it to headings after a
    /// `[TOC]` marker; `None` includes every heading.
    fn toc_entries(&self, after: Option<usize>) -> Vec<ExportTocEntry> {
        let mut heads: Vec<(u8, String, String)> = self
            .doc
            .headings
            .iter()
            .filter(|heading| heading.in_toc && after.is_none_or(|block| heading.block > block))
            .map(|heading| (heading.level.clamp(1, 6), heading.id.clone(), heading.text.clone()))
            .collect();
        let Some(top) = heads.iter().map(|(level, _, _)| *level).min() else { return Vec::new() };
        // Flatten the heading levels into contiguous depths: an h4 after an h2
        // nests one level, not two, as the reader's navigation does.
        let mut ancestors: Vec<u8> = Vec::new();
        let mut rows: Vec<(usize, u8, String, String)> = Vec::with_capacity(heads.len());
        for (level, id, text) in heads.drain(..) {
            let level = level.max(top);
            while ancestors.last().is_some_and(|last| level <= *last) {
                ancestors.pop();
            }
            rows.push((ancestors.len(), level, id, text));
            ancestors.push(level);
        }
        let mut index = 0;
        toc_level(&rows, &mut index, 0)
    }

    // ---- inline ---------------------------------------------------------

    /// Splits an inline into styled runs at every style, link and object edge.
    fn inline(&mut self, inline: &Inline, strip_tasks: bool) -> Result<Vec<ExportRun>, ExportError> {
        let len = inline.text.len();
        let mut pieces: Vec<Piece<'_>> = Vec::with_capacity(inline.math.len() + inline.objects.len());
        for math in &inline.math {
            pieces.push(Piece::Math(math));
        }
        for object in &inline.objects {
            if matches!(object.kind, InlineObjectKind::LineBreak) {
                continue;
            }
            pieces.push(Piece::Object(object));
        }
        pieces.retain(|piece| {
            let range = piece.range();
            range.start <= range.end && range.end <= len
        });
        pieces.sort_by_key(|piece| (piece.range().start, piece.range().end));

        let mut events = vec![0, len];
        for span in &inline.spans {
            push_range(&mut events, &span.range, len, &inline.text);
        }
        for link in &inline.links {
            push_range(&mut events, &link.range, len, &inline.text);
        }
        for piece in &pieces {
            push_range(&mut events, piece.range(), len, &inline.text);
        }
        events.sort_unstable();
        events.dedup();

        // A zero-width inline object — a standalone `<video>`, `<audio>` or
        // `<details>` tag, or a picture with no text around it — has no range
        // to slice, so the text windows below never reach it. It still has to
        // reach the writers (a media placeholder would otherwise vanish from
        // an otherwise empty paragraph), so keep those objects aside and emit
        // them at their byte position.
        let mut zero_by_position: BTreeMap<usize, Vec<Piece<'_>>> = BTreeMap::new();
        let mut spanning: Vec<Piece<'_>> = Vec::with_capacity(pieces.len());
        for piece in pieces {
            let range = piece.range();
            if range.is_empty() {
                zero_by_position.entry(boundary(&inline.text, range.start)).or_default().push(piece);
            } else {
                spanning.push(piece);
            }
        }

        let mut runs = Vec::new();
        let mut piece_index = 0;
        for pair in events.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            if start >= end {
                continue;
            }
            if let Some(pieces) = zero_by_position.remove(&start) {
                for piece in pieces {
                    self.emit_piece(piece, inline, &mut runs, strip_tasks)?;
                }
            }
            while piece_index < spanning.len() && spanning[piece_index].range().end <= start {
                piece_index += 1;
            }
            if piece_index < spanning.len() {
                let range = spanning[piece_index].range().clone();
                if range.start <= start && start < range.end {
                    if range.start == start {
                        let piece = spanning[piece_index];
                        self.emit_piece(piece, inline, &mut runs, strip_tasks)?;
                    }
                    continue;
                }
            }
            let style = active_style(inline, start);
            let link_index = active_link(inline, start);
            let raw_link = link_index.map(|index| &inline.links[index]);
            if let Some(raw) = raw_link
                && let Some(label) = raw.dest.strip_prefix("fn:")
                && let Some(number) = self.footnote_number(label)
            {
                let text = &inline.text[start..end];
                if text.trim() == number.to_string() {
                    let occurrence = self.next_occurrence(label);
                    runs.push(ExportRun::FootnoteRef { label: label.to_owned(), number, occurrence });
                    continue;
                }
            }
            let link = link_index.and_then(|index| self.export_link(&inline.links[index]));
            self.emit_text(&inline.text[start..end], style, link, &mut runs);
        }
        if let Some(pieces) = zero_by_position.remove(&len) {
            for piece in pieces {
                self.emit_piece(piece, inline, &mut runs, strip_tasks)?;
            }
        }
        Ok(runs)
    }

    fn emit_piece(
        &mut self,
        piece: Piece<'_>,
        inline: &Inline,
        runs: &mut Vec<ExportRun>,
        strip_tasks: bool,
    ) -> Result<(), ExportError> {
        match piece {
            Piece::Math(math) => {
                let link = self.object_link(inline, math.range.start);
                let asset = self.math(&math.tex, math.display)?;
                runs.push(ExportRun::Math { tex: math.tex.clone(), display: math.display, asset, link });
            }
            Piece::Object(object) => match &object.kind {
                InlineObjectKind::TaskCheckbox { checked } => {
                    if !strip_tasks {
                        let glyph = if *checked { "\u{2611}" } else { "\u{2610}" };
                        push_text(runs, glyph.to_owned(), RunStyle::default(), None);
                    }
                }
                InlineObjectKind::Image => {
                    if let Some(node) = &object.html
                        && let Some(image) = self.image_from_html(node)
                    {
                        let link = self.object_link(inline, object.range.start);
                        runs.push(ExportRun::Image { image, link });
                    }
                }
                InlineObjectKind::Picture => {
                    if let Some(node) = &object.html
                        && let Some(image) = self.picture_from_html(node)
                    {
                        let link = self.object_link(inline, object.range.start);
                        runs.push(ExportRun::Image { image, link });
                    }
                }
                InlineObjectKind::Video
                | InlineObjectKind::Audio
                | InlineObjectKind::Details
                | InlineObjectKind::Unknown => {
                    if let Some(node) = &object.html {
                        let node = self.prepare_html(node)?;
                        if !is_empty_html(&node) {
                            runs.push(ExportRun::Html(node));
                        }
                    }
                }
                InlineObjectKind::LineBreak => {}
            },
        }
        Ok(())
    }

    /// Appends text between objects, turning newlines into hard breaks.
    fn emit_text(&mut self, text: &str, style: RunStyle, link: Option<ExportLink>, runs: &mut Vec<ExportRun>) {
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                runs.push(ExportRun::Break);
            }
            if !part.is_empty() {
                let part = part.replace(lmdv_md::OBJECT_CHAR, "");
                if !part.is_empty() {
                    push_text(runs, part, style, link.clone());
                }
            }
        }
    }

    /// The link covering an inline object, if any.
    fn object_link(&mut self, inline: &Inline, position: usize) -> Option<ExportLink> {
        let index = active_link(inline, position)?;
        self.export_link(&inline.links[index])
    }

    /// Resolves a Markdown link, including the app's `fn:` and `#fragment`
    /// conventions.
    fn export_link(&mut self, link: &InlineLink) -> Option<ExportLink> {
        let href = self.resolve_href(&link.dest)?;
        Some(ExportLink { href, title: link.title.clone().filter(|title| !title.is_empty()) })
    }

    fn resolve_href(&mut self, dest: &str) -> Option<String> {
        if let Some(label) = dest.strip_prefix("fn:")
            && let Some(number) = self.footnote_number(label)
        {
            return Some(format!("#fn-{number}"));
        }
        if dest.starts_with('#')
            && let Some(index) = self.doc.resolve_heading(dest)
        {
            return Some(format!("#{}", self.doc.headings[index].id));
        }
        match crate::link::resolve(dest, self.opts.source_path.as_deref()) {
            Some(href) => Some(href),
            None => {
                self.warn(
                    ExportWarningKind::LinkRefused,
                    format!("link destination refused: {dest}"),
                    Some(dest.to_owned()),
                );
                None
            }
        }
    }

    fn footnote_number(&self, label: &str) -> Option<usize> {
        self.doc.footnotes.iter().find(|footnote| footnote.label == label).map(|footnote| footnote.number)
    }

    fn next_occurrence(&mut self, label: &str) -> usize {
        let count = self.ref_counts.entry(label.to_owned()).or_insert(0);
        let occurrence = *count;
        *count += 1;
        occurrence
    }

    // ---- pictures -------------------------------------------------------

    fn image_from_block(&mut self, image: &ImageBlock) -> Option<ExportImage> {
        let src = self.block_image_src(image)?;
        let alt = image.alt.clone();
        let asset = self.image_asset(&src, &alt);
        Some(ExportImage { src, alt, title: image.title.clone().filter(|title| !title.is_empty()), asset })
    }

    /// The light source of a block picture: a light `<picture>` candidate, a
    /// `srcset` candidate, or the plain `src`. Dark-theme-only sources are
    /// dropped because exported pages are light.
    fn block_image_src(&self, image: &ImageBlock) -> Option<String> {
        let from_source = image
            .picture
            .iter()
            .find(|source| media_is_light(source.media.as_deref()))
            .and_then(|source| source.srcset.first())
            .map(|candidate| candidate.url.clone());
        let src = from_source
            .or_else(|| image.srcset.first().map(|candidate| candidate.url.clone()))
            .unwrap_or_else(|| image.src.clone());
        (!src.is_empty() && !is_dark_only(&src)).then(|| clean_image_src(&src).to_owned())
    }

    fn image_from_html(&mut self, node: &HtmlNode) -> Option<ExportImage> {
        match node.tag {
            HtmlTag::Img => {
                let src = node
                    .attr("src")
                    .filter(|src| !src.is_empty())
                    .map(str::to_owned)
                    .or_else(|| node.srcset.first().map(|candidate| candidate.url.clone()))?;
                if is_dark_only(&src) {
                    return None;
                }
                let src = clean_image_src(&src).to_owned();
                let alt = node.attr("alt").unwrap_or_default().to_owned();
                let asset = self.image_asset(&src, &alt);
                let title = node.attr("title").map(str::to_owned).filter(|title| !title.is_empty());
                Some(ExportImage { src, alt, title, asset })
            }
            HtmlTag::Picture => self.picture_from_html(node),
            _ => None,
        }
    }

    fn picture_from_html(&mut self, node: &HtmlNode) -> Option<ExportImage> {
        let mut fallback = None;
        let mut source_image = None;
        for child in &node.children {
            let HtmlChild::Node(child) = child else { continue };
            match child.tag {
                HtmlTag::Source if media_is_light(child.attr("media")) => {
                    if let Some(src) = child.srcset.first().map(|candidate| candidate.url.clone())
                        && source_image.is_none()
                    {
                        source_image = self.image_from_src(&src, node);
                    }
                }
                HtmlTag::Img => fallback = Some(child),
                _ => {}
            }
        }
        if source_image.as_ref().is_some_and(|image| image.asset.is_some()) {
            return source_image;
        }
        fallback.and_then(|child| self.image_from_html(child)).or(source_image)
    }

    fn image_from_src(&mut self, src: &str, picture: &HtmlNode) -> Option<ExportImage> {
        if is_dark_only(src) {
            return None;
        }
        let src = clean_image_src(src);
        let img = picture.children.iter().find_map(|child| match child {
            HtmlChild::Node(child) if child.tag == HtmlTag::Img => Some(child),
            _ => None,
        });
        let alt = img.and_then(|img| img.attr("alt")).unwrap_or_default().to_owned();
        let title = img.and_then(|img| img.attr("title")).map(str::to_owned).filter(|title| !title.is_empty());
        let asset = self.image_asset(src, &alt);
        Some(ExportImage { src: src.to_owned(), alt, title, asset })
    }

    fn image_asset(&mut self, src: &str, alt: &str) -> Option<AssetId> {
        if src.is_empty() || is_dark_only(src) {
            return None;
        }
        if is_remote_asset(src) {
            self.warn(
                ExportWarningKind::ImageRefused,
                format!("network image not embedded: {src}"),
                Some(src.to_owned()),
            );
            return None;
        }
        if let Some(hit) = self.image_cache.get(src) {
            return *hit;
        }
        let supplied = self.resolver.image(src, alt);
        let asset = match supplied {
            Some(asset) => self.push_image(asset, src).or_else(|| self.data_image(src)),
            None => self.data_image(src),
        };
        self.image_cache.insert(src.to_owned(), asset);
        if asset.is_none() {
            self.warn(ExportWarningKind::ImageMissing, format!("image not available: {src}"), Some(src.to_owned()));
        }
        asset
    }

    /// Accepts a resolver asset if its type is a known image and an SVG is safe.
    fn push_image(&mut self, asset: ExportedAsset, source: &str) -> Option<AssetId> {
        let Some(mime) = image_mime(asset.mime) else {
            self.warn(
                ExportWarningKind::ImageRefused,
                format!("unsupported image type {}: {source}", asset.mime),
                Some(source.to_owned()),
            );
            return None;
        };
        if asset.bytes.is_empty() {
            return None;
        }
        if mime == "image/svg+xml" && svg_is_unsafe(&asset.bytes) {
            self.warn(
                ExportWarningKind::ImageRefused,
                format!("SVG with scripts or external references refused: {source}"),
                Some(source.to_owned()),
            );
            return None;
        }
        Some(self.push_asset(mime, asset.bytes, None, None, None))
    }

    /// Embeds a small `data:` image already present in the Markdown, even when
    /// the resolver cannot handle data URIs.
    fn data_image(&mut self, src: &str) -> Option<AssetId> {
        if !src.get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:")) {
            return None;
        }
        let rest = &src[5..];
        let (meta, payload) = rest.split_once(',')?;
        let mut parts = meta.split(';');
        let mime = image_mime(parts.next()?)?;
        if !parts.any(|part| part.eq_ignore_ascii_case("base64")) {
            return None;
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()?;
        if bytes.is_empty() || bytes.len() > MAX_DATA_IMAGE {
            return None;
        }
        if mime == "image/svg+xml" && svg_is_unsafe(&bytes) {
            return None;
        }
        Some(self.push_asset(mime, bytes, None, None, None))
    }

    fn push_asset(
        &mut self,
        mime: &'static str,
        bytes: Vec<u8>,
        width: Option<u32>,
        height: Option<u32>,
        baseline: Option<u32>,
    ) -> AssetId {
        let id = AssetId(self.assets_out.len() as u32);
        self.assets_out.push(ExportAsset { id, mime, bytes, width, height, baseline });
        id
    }

    // ---- math and diagrams ----------------------------------------------

    fn math(&mut self, tex: &str, display: bool) -> Result<Option<AssetId>, ExportError> {
        if !self.opts.math {
            return Ok(None);
        }
        let key = (tex.to_owned(), display);
        if let Some(hit) = self.math_cache.get(&key) {
            return Ok(*hit);
        }
        let request =
            MathRequest { tex: tex.to_owned(), size_pt: MATH_SIZE, color: MATH_COLOR, display, scale: MATH_SCALE };
        let asset = match self.math.render(&request) {
            Ok(rendered) if rendered.width > 0 && rendered.height > 0 => Some(self.math_png(&rendered)?),
            Ok(_) => {
                self.warn(
                    ExportWarningKind::MathFallback,
                    "empty formula rendered nothing".to_owned(),
                    Some(tex.to_owned()),
                );
                None
            }
            Err(error) => {
                self.warn(ExportWarningKind::MathFallback, error.to_string(), Some(tex.to_owned()));
                None
            }
        };
        self.math_cache.insert(key, asset);
        Ok(asset)
    }

    /// Encodes the 2× raster as PNG and records its logical CSS size.
    fn math_png(&mut self, rendered: &RenderedMath) -> Result<AssetId, ExportError> {
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&rendered.rgba, rendered.width, rendered.height, ExtendedColorType::Rgba8)
            .map_err(|error| ExportError::MathImage(error.to_string()))?;
        let width = (rendered.width as f32 / MATH_SCALE).round().max(1.0) as u32;
        let height = (rendered.height as f32 / MATH_SCALE).round().max(1.0) as u32;
        let baseline = rendered.descent_pt.round().max(0.0) as u32;
        Ok(self.push_asset("image/png", bytes, Some(width), Some(height), Some(baseline)))
    }

    fn diagram(&mut self, source: &str) -> Option<AssetId> {
        if let Some(hit) = self.diagram_cache.get(source) {
            return *hit;
        }
        let supplied = self.resolver.diagram(source);
        let asset = supplied.and_then(|asset| self.push_image(asset, source));
        self.diagram_cache.insert(source.to_owned(), asset);
        if asset.is_none() {
            self.warn(
                ExportWarningKind::DiagramFallback,
                "diagram rendered as source".to_owned(),
                Some(source.to_owned()),
            );
        }
        asset
    }

    // ---- HTML subset -----------------------------------------------------

    /// Prepares an allowed HTML subtree: pictures are collapsed to one source,
    /// images become asset ids, media sources become local file URLs or plain
    /// links, and source/track children are dropped.
    fn prepare_html(&mut self, node: &HtmlNode) -> Result<HtmlNode, ExportError> {
        let mut node = node.clone();
        self.prepare_node(&mut node)?;
        Ok(node)
    }

    fn prepare_node(&mut self, node: &mut HtmlNode) -> Result<(), ExportError> {
        match node.tag {
            HtmlTag::Img => {
                self.prepare_img(node);
                Ok(())
            }
            HtmlTag::Picture => {
                self.prepare_picture(node);
                Ok(())
            }
            HtmlTag::Video | HtmlTag::Audio => {
                self.prepare_media(node);
                self.prepare_children(node)
            }
            HtmlTag::A => {
                let href = node.attr("href").map(str::to_owned);
                let resolved = href.and_then(|href| crate::link::resolve(&href, self.opts.source_path.as_deref()));
                match resolved {
                    Some(resolved) => set_attr(node, "href", resolved),
                    None => {
                        node.tag = HtmlTag::Span;
                        remove_attr(node, "href");
                    }
                }
                self.prepare_children(node)
            }
            HtmlTag::Source | HtmlTag::Track => Ok(()),
            _ => self.prepare_children(node),
        }
    }

    fn prepare_children(&mut self, node: &mut HtmlNode) -> Result<(), ExportError> {
        let old = std::mem::take(&mut node.children);
        let mut children = Vec::with_capacity(old.len());
        for child in old {
            match child {
                HtmlChild::Node(mut child) => {
                    if matches!(child.tag, HtmlTag::Source | HtmlTag::Track) {
                        continue;
                    }
                    self.prepare_node(&mut child)?;
                    if is_empty_html(&child) {
                        continue;
                    }
                    children.push(HtmlChild::Node(child));
                }
                HtmlChild::Text(text) => children.push(HtmlChild::Text(text)),
            }
        }
        node.children = children;
        Ok(())
    }

    fn prepare_img(&mut self, node: &mut HtmlNode) {
        let src = node
            .attr("src")
            .filter(|src| !src.is_empty())
            .map(str::to_owned)
            .or_else(|| node.srcset.first().map(|candidate| candidate.url.clone()));
        node.srcset.clear();
        let Some(src) = src.filter(|src| !is_dark_only(src)) else {
            drop_image(node);
            return;
        };
        let src = clean_image_src(&src).to_owned();
        let alt = node.attr("alt").unwrap_or_default().to_owned();
        match self.image_asset(&src, &alt) {
            Some(id) => set_attr(node, "src", format!("lmdv-asset:{}", id.0)),
            None => {
                remove_attr(node, "src");
            }
        }
    }

    fn prepare_picture(&mut self, node: &mut HtmlNode) {
        let mut source = None;
        let mut fallback = None;
        for child in &node.children {
            let HtmlChild::Node(child) = child else { continue };
            match child.tag {
                HtmlTag::Source if media_is_light(child.attr("media")) => {
                    if let Some(src) = child.srcset.first().map(|candidate| candidate.url.clone())
                        && source.is_none()
                    {
                        source = Some((src, child.clone()));
                    }
                }
                HtmlTag::Img => fallback = Some(child.clone()),
                _ => {}
            }
        }
        if let Some((src, _)) = source {
            let mut img = fallback.clone().unwrap_or_else(|| HtmlNode {
                tag: HtmlTag::Img,
                attrs: Vec::new(),
                children: Vec::new(),
                srcset: Vec::new(),
            });
            set_attr(&mut img, "src", src);
            self.prepare_img(&mut img);
            if img.attr("src").is_some() {
                *node = img;
                return;
            }
        }
        if let Some(mut img) = fallback {
            self.prepare_img(&mut img);
            *node = img;
            return;
        }
        drop_image(node);
    }

    fn prepare_media(&mut self, node: &mut HtmlNode) {
        let Some(src) = node.attr("src").filter(|src| !src.is_empty()).map(str::to_owned) else { return };
        remove_attr(node, "src");
        remove_attr(node, "poster");
        if is_remote_asset(&src) {
            set_attr(node, "data-lmdv-remote", src);
        } else if let Some(href) = crate::link::resolve(&src, self.opts.source_path.as_deref()) {
            set_attr(node, "src", href);
        }
    }

    // ---- diagnostics -----------------------------------------------------

    fn warn(&mut self, kind: ExportWarningKind, message: String, source: Option<String>) {
        let key = format!("{kind:?}|{message}|{}", source.as_deref().unwrap_or_default());
        if self.warned.insert(key) {
            self.warnings.push(ExportWarning { kind, message, source });
        }
    }
}

/// Pushes a range into the boundary list, clamped to the text and to character
/// boundaries so slicing is always valid.
fn push_range(events: &mut Vec<usize>, range: &Range<usize>, len: usize, text: &str) {
    events.push(boundary(text, range.start.min(len)));
    events.push(boundary(text, range.end.min(len)));
}

fn boundary(text: &str, mut byte: usize) -> usize {
    byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

/// The styles covering a byte.
fn active_style(inline: &Inline, position: usize) -> RunStyle {
    let mut style = RunStyle::default();
    for span in &inline.spans {
        if span.range.start <= position && position < span.range.end {
            match span.style {
                InlineStyle::Strong => style.strong = true,
                InlineStyle::Em => style.em = true,
                InlineStyle::Strike => style.strike = true,
                InlineStyle::Code => style.code = true,
                InlineStyle::Mark => style.mark = true,
                InlineStyle::Sub => style.sub = true,
                InlineStyle::Sup => style.sup = true,
                InlineStyle::Kbd => style.kbd = true,
                InlineStyle::Underline => style.underline = true,
                InlineStyle::Plain | InlineStyle::Link => {}
            }
        }
    }
    style
}

/// The index of the innermost link covering a byte.
fn active_link(inline: &Inline, position: usize) -> Option<usize> {
    inline
        .links
        .iter()
        .enumerate()
        .filter(|(_, link)| link.range.start <= position && position < link.range.end)
        .max_by_key(|(index, link)| (link.range.start, *index))
        .map(|(index, _)| index)
}

/// Appends text, merging with the previous run when the style and link match.
fn push_text(runs: &mut Vec<ExportRun>, text: String, style: RunStyle, link: Option<ExportLink>) {
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

/// The first task checkbox of an item's first block.
fn first_task(blocks: &[Block]) -> Option<bool> {
    let Block::Para(inline) = blocks.first()? else { return None };
    inline.objects.iter().find_map(|object| match &object.kind {
        InlineObjectKind::TaskCheckbox { checked } => Some(*checked),
        _ => None,
    })
}

/// The item's own text, trimmed and capped at 120 characters.
fn item_label(blocks: &[Block]) -> String {
    let mut text = String::new();
    for block in blocks {
        match block {
            Block::Para(inline) | Block::Heading { inline, .. } => {
                text.extend(inline.text.chars().filter(|c| *c != lmdv_md::OBJECT_CHAR));
                text.push(' ');
            }
            Block::Code { text: code, .. } => {
                text.push_str(code);
                text.push(' ');
            }
            _ => {}
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

/// Nests the flat `(depth, level, id, text)` rows into a tree.
fn toc_level(rows: &[(usize, u8, String, String)], index: &mut usize, depth: usize) -> Vec<ExportTocEntry> {
    let mut out: Vec<ExportTocEntry> = Vec::new();
    while let Some((row_depth, level, id, text)) = rows.get(*index) {
        if *row_depth < depth {
            break;
        }
        if *row_depth > depth {
            let children = toc_level(rows, index, *row_depth);
            if let Some(last) = out.last_mut() {
                last.children.extend(children);
            } else {
                out.extend(children);
            }
            continue;
        }
        *index += 1;
        let mut entry = ExportTocEntry { id: id.clone(), text: text.clone(), level: *level, children: Vec::new() };
        entry.children = toc_level(rows, index, depth + 1);
        out.push(entry);
    }
    out
}

/// Whether a media query selects the light scheme (or does not select one).
fn media_is_light(media: Option<&str>) -> bool {
    let Some(media) = media else { return true };
    !media.to_ascii_lowercase().contains("dark")
}

/// GitHub's `#gh-dark-mode-only` fragment hides a picture on light paper.
fn is_dark_only(src: &str) -> bool {
    src.to_ascii_lowercase().contains("#gh-dark-mode-only")
}

/// Removes GitHub's light/dark scheme fragment before a path is resolved.
fn clean_image_src(src: &str) -> &str {
    let lower = src.to_ascii_lowercase();
    for suffix in ["#gh-dark-mode-only", "#gh-light-mode-only"] {
        if lower.ends_with(suffix) {
            return &src[..src.len() - suffix.len()];
        }
    }
    src
}

/// Whether an image source would need the network or a non-file scheme.
fn is_remote_asset(src: &str) -> bool {
    let src = src.trim();
    if src.starts_with("//") {
        return true;
    }
    crate::link::scheme(src)
        .is_some_and(|scheme| !scheme.eq_ignore_ascii_case("file") && !scheme.eq_ignore_ascii_case("data"))
}

/// Maps a resolver MIME type to the static types the exporter embeds.
fn image_mime(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next().unwrap_or_default().trim();
    Some(match mime.to_ascii_lowercase().as_str() {
        "image/png" => "image/png",
        "image/jpeg" | "image/jpg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "image/avif" => "image/avif",
        "image/bmp" => "image/bmp",
        "image/svg+xml" => "image/svg+xml",
        _ => return None,
    })
}

/// An SVG with a script or an external reference is refused: an exported file
/// must never reach the network, even from inside a picture. The W3C namespace
/// declarations are the only allowed `http` URLs.
fn svg_is_unsafe(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    let text = text.replace("http://www.w3.org/", "").replace("https://www.w3.org/", "");
    [
        "<script",
        "javascript:",
        "http://",
        "https://",
        "url(//",
        "href=\"//",
        "href='//",
        "@import",
        "onload=",
        "onerror=",
        "<foreignobject",
        "<iframe",
        "<embed",
        "<object",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// Replaces an image node with an empty inline span.
fn drop_image(node: &mut HtmlNode) {
    node.tag = HtmlTag::Span;
    node.attrs.clear();
    node.children.clear();
    node.srcset.clear();
}

fn set_attr(node: &mut HtmlNode, name: &str, value: String) {
    if let Some((_, current)) = node.attrs.iter_mut().find(|(key, _)| key == name) {
        *current = value;
    } else {
        node.attrs.push((name.to_owned(), value));
    }
}

fn remove_attr(node: &mut HtmlNode, name: &str) {
    node.attrs.retain(|(key, _)| key != name);
}

/// Whether a prepared HTML node would render nothing.
fn is_empty_html(node: &HtmlNode) -> bool {
    match node.tag {
        HtmlTag::Span | HtmlTag::Fragment | HtmlTag::A => node.children.iter().all(is_empty_child),
        HtmlTag::Img => node.attr("src").is_none() && node.attr("alt").unwrap_or_default().is_empty(),
        _ => false,
    }
}

fn is_empty_child(child: &HtmlChild) -> bool {
    match child {
        HtmlChild::Text(text) => text.is_empty(),
        HtmlChild::Node(node) => is_empty_html(node),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_title_prefers_the_file_stem() {
        let doc = lmdv_md::parse("# From heading\n");
        let mut opts = ExportOptions { source_path: Some("/tmp/notes.md".into()), ..ExportOptions::default() };
        assert_eq!(document_title(&doc, &opts), "notes");
        opts.title = Some("Given".into());
        assert_eq!(document_title(&doc, &opts), "Given");
        opts.title = None;
        opts.source_path = None;
        assert_eq!(document_title(&doc, &opts), "From heading");

        let empty = lmdv_md::parse("");
        assert_eq!(document_title(&empty, &opts), "Pasted text");
        let italian = ExportOptions { lang: "it".into(), ..ExportOptions::default() };
        assert_eq!(document_title(&empty, &italian), "Testo incollato");
    }

    #[test]
    fn image_types_are_restricted_to_pictures() {
        assert_eq!(image_mime("image/png"), Some("image/png"));
        assert_eq!(image_mime("IMAGE/PNG; charset=binary"), Some("image/png"));
        assert_eq!(image_mime("text/html"), None);
        assert_eq!(image_mime("application/pdf"), None);
    }

    #[test]
    fn dark_pictures_are_recognized() {
        assert!(is_dark_only("a.png#gh-dark-mode-only"));
        assert!(!is_dark_only("a.png#gh-light-mode-only"));
        assert!(media_is_light(Some("(prefers-color-scheme: light)")));
        assert!(!media_is_light(Some("(prefers-color-scheme: dark)")));
    }

    #[test]
    fn svg_namespaces_pass_but_scripts_and_remote_references_do_not() {
        assert!(!svg_is_unsafe(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"));
        assert!(svg_is_unsafe(b"<svg><script>alert(1)</script></svg>"));
        assert!(svg_is_unsafe(b"<svg><image href=\"https://example.com/a.png\"/></svg>"));
        assert!(svg_is_unsafe(b"<svg><image href='//example.com/a.png'/></svg>"));
    }
}
