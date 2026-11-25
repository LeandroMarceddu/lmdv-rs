//! Word-friendly rich HTML for the clipboard (X-02, X-03).
//!
//! The clipboard fragment is the same content as [`ExportDoc::to_html`], but
//! flattened for paste targets: every rule is an inline style, the theme is
//! always the light export theme, and Word's weak spots get dedicated markup —
//! code blocks become single-cell shaded tables, task boxes become ☑/☐, alerts
//! carry their emoji and colour, and pictures travel as `data:` URIs.
//!
//! The original's rich copy (`share.ts:345-577`) first rendered the live
//! article, then walked the DOM. The export model already has the resolved
//! semantic content, so this writer produces the portable clone directly: no
//! classes, no ids outside heading/footnote bookmarks, no scripts, no
//! stylesheet and no external resource. The plain-text form of the copy is the
//! document's Markdown source, supplied by the application. [`export_rich_fragment`]
//! runs the same writer over one contiguous top-level block range, so a
//! selection copy (X-03) keeps the same markup and styling.
//!
//! Math is the embedded 2× PNG with the TeX in `alt`; native MathML/OMML is
//! still open (OI-066). Mermaid is an embedded SVG image when the app's resolver
//! provides one, and falls back to its source otherwise (OI-106).

use std::ops::Range;
use std::path::{Path, PathBuf};

use lmdv_i18n::{Key, text_in};
use lmdv_md::{Align, CalloutKind, HtmlChild, HtmlNode, HtmlTag};
use rune_highlights::{Highlighter, Lang};

use crate::labels::ExportLabels;
use crate::model::{
    AssetId, ExportAsset, ExportBlock, ExportDoc, ExportImage, ExportLink, ExportList, ExportRun, ExportTocEntry,
    RunStyle,
};

/// The localized table-of-contents name.
const TOC: Key = lmdv_i18n::key("render.toc");

/// The font stack Word and browser paste targets share.
const SANS: &str = "system-ui,-apple-system,Segoe UI,Roboto,Helvetica Neue,Arial,sans-serif";
/// The monospace stack of code, inline code and keyboard keys.
const MONO: &str = "ui-monospace,Cascadia Code,Source Code Pro,Menlo,Consolas,monospace";

const TEXT: &str = "#1f2328";
const MUTED: &str = "#59636e";
const BORDER: &str = "#d1d9e0";
const SURFACE: &str = "#f6f8fa";
const SURFACE_2: &str = "#eff1f3";
const ACCENT: &str = "#0969da";
const MARK: &str = "#fff8c5";

/// What the rich-copy writer needs besides the document.
///
/// The document already carries its language, source path and title; the
/// options override them for a copy whose context differs (a pasted buffer, a
/// window in another UI language). `title` is the fragment's accessible name
/// (`role="document"`), never a visible heading: the copy must not invent
/// content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichOptions {
    /// The BCP 47 tag for the document labels; empty keeps the document's.
    pub lang: String,
    /// The Markdown file the copy came from, for relative local links. When
    /// set it wins over the document's own path.
    pub source_path: Option<PathBuf>,
    /// The accessible name of the copied fragment.
    pub title: Option<String>,
}

/// Serializes `doc` as one rich-text clipboard fragment.
///
/// The result is a `<div>` with inline styles only; it carries no `<style>`,
/// class, script or stylesheet reference. Front matter is never included, and
/// pictures are `data:` URIs (or their alt text when the resolver had none).
pub fn export_rich_html(doc: &ExportDoc, opts: &RichOptions) -> String {
    let mut writer = Writer::new(doc, opts, true);
    writer.fragment(0..doc.blocks.len());
    writer.out
}

/// Serializes one contiguous range of top-level blocks as a clipboard fragment
/// (X-03).
///
/// This is the selection copy: the same wrapper, labels, links, images, math,
/// callouts, tables, code and task styling as [`export_rich_html`], but only
/// `blocks`. Out-of-range indices are clamped to the document; an empty range
/// (or one that clamps to nothing) returns the empty wrapper `<div …></div>`,
/// exactly as an empty document does.
///
/// The document title is **not** the fragment's accessible name here: a
/// selection must not claim the whole document's label. Pass
/// [`RichOptions::title`] when the copy needs one.
///
/// Map a source selection to `blocks` with [`crate::blocks_for_source`] when
/// the model carries block spans.
pub fn export_rich_fragment(doc: &ExportDoc, blocks: Range<usize>, opts: &RichOptions) -> String {
    let mut writer = Writer::new(doc, opts, false);
    writer.fragment(block_range(blocks, doc.blocks.len()));
    writer.out
}

/// Clamps a block range to `0..len`; a reversed or empty range becomes empty.
fn block_range(blocks: Range<usize>, len: usize) -> Range<usize> {
    let start = blocks.start.min(len);
    let end = blocks.end.min(len);
    if start < end { start..end } else { start..start }
}

struct Writer<'a> {
    doc: &'a ExportDoc,
    source_path: Option<&'a Path>,
    title: Option<&'a str>,
    out: String,
    highlighter: Highlighter,
    labels: ExportLabels,
    toc_label: String,
    footnotes_label: String,
    quote_depth: usize,
    note_mode: bool,
}

impl<'a> Writer<'a> {
    fn new(doc: &'a ExportDoc, opts: &'a RichOptions, document_title: bool) -> Self {
        let lang = if !opts.lang.trim().is_empty() {
            opts.lang.trim().to_owned()
        } else if !doc.lang.trim().is_empty() {
            doc.lang.trim().to_owned()
        } else {
            "en".to_owned()
        };
        let locale = lmdv_i18n::negotiate(std::iter::once(lang.as_str()));
        // A selection copy only carries a title the caller passed; the whole
        // document keeps its own title as the accessible name.
        let fallback = if document_title { doc.title.as_str() } else { "" };
        let title = opts.title.as_deref().unwrap_or(fallback).trim();
        Self {
            doc,
            source_path: opts.source_path.as_deref().or(doc.source_path.as_deref()),
            title: (!title.is_empty()).then_some(title),
            out: String::new(),
            highlighter: Highlighter::new(),
            labels: ExportLabels::for_lang(&lang),
            toc_label: text_in(locale, TOC).to_owned(),
            footnotes_label: text_in(locale, lmdv_i18n::key("render.footnotes")).to_owned(),
            quote_depth: 0,
            note_mode: false,
        }
    }

    fn fragment(&mut self, blocks: Range<usize>) {
        self.out.push_str("<div role=\"document\"");
        if let Some(title) = self.title {
            self.out.push_str(" aria-label=\"");
            escape_attr(&mut self.out, title);
            self.out.push('"');
        }
        self.out.push_str(" style=\"max-width:760px;font-family:");
        self.out.push_str(SANS);
        self.out.push_str(";font-size:16px;line-height:1.7;color:");
        self.out.push_str(TEXT);
        self.out.push_str(";background-color:#ffffff\">");
        for block in &self.doc.blocks[blocks] {
            self.block(block);
        }
        self.out.push_str("</div>");
    }

    fn block(&mut self, block: &ExportBlock) {
        match block {
            ExportBlock::Heading { level, id, runs } => self.heading(*level, id, runs),
            ExportBlock::Para(runs) => {
                if runs.is_empty() {
                    return;
                }
                self.paragraph(runs);
            }
            ExportBlock::List(list) => self.list(list),
            ExportBlock::Quote { callout: None, blocks } => {
                self.out.push_str("<blockquote style=\"margin:0 0 16px;padding:2px 0 2px 16px;border-left:3px solid ");
                self.out.push_str(BORDER);
                self.out.push_str(";color:");
                self.out.push_str(MUTED);
                self.out.push_str("\">");
                self.quote_depth += 1;
                for block in blocks {
                    self.block(block);
                }
                self.quote_depth -= 1;
                self.out.push_str("</blockquote>\n");
            }
            ExportBlock::Quote { callout: Some(callout), blocks } => {
                let (color, background, symbol) = callout_colors(callout.kind);
                self.out.push_str(
                    "<aside role=\"note\" style=\"margin:0 0 16px;padding:6px 16px 2px;border-left:3px solid ",
                );
                self.out.push_str(color);
                self.out.push_str(";background-color:");
                self.out.push_str(background);
                self.out.push_str(";border-radius:0 8px 8px 0\">");
                self.out.push_str("<p style=\"margin:0 0 6px;font-family:");
                self.out.push_str(SANS);
                self.out.push_str(";font-size:15px;font-weight:600;line-height:1.4;color:");
                self.out.push_str(color);
                self.out.push_str("\">");
                let title = callout.title.as_deref().unwrap_or_else(|| self.labels.callout(callout.kind));
                self.out.push_str(symbol);
                self.out.push(' ');
                escape_text(&mut self.out, title);
                self.out.push_str("</p>");
                for block in blocks {
                    self.block(block);
                }
                self.out.push_str("</aside>\n");
            }
            ExportBlock::Code { lang, text } => self.code_table(lang.as_deref(), text),
            ExportBlock::Table(table) => self.table(table),
            ExportBlock::Math { tex, asset, .. } => {
                self.out.push_str("<div style=\"margin:16px 0;text-align:center\">");
                self.math_image(tex, *asset, true);
                self.out.push_str("</div>\n");
            }
            ExportBlock::Image(image) => self.block_image(image),
            ExportBlock::Diagram { source, asset } => self.diagram(source, *asset),
            ExportBlock::Rule => {
                self.out.push_str("<hr style=\"height:0;margin:24px 0;border:0;border-top:1px solid ");
                self.out.push_str(BORDER);
                self.out.push_str("\">\n");
            }
            ExportBlock::Toc { entries } => self.toc(entries),
            ExportBlock::Footnotes => self.footnotes(),
            ExportBlock::Html(node) => {
                self.html_node(node);
                self.out.push('\n');
            }
        }
    }

    fn heading(&mut self, level: u8, id: &str, runs: &[ExportRun]) {
        let level = level.clamp(1, 6);
        let tag = char::from(b'0' + level);
        let (size, weight, margin, border) = match level {
            1 => ("30px", "700", "36px 0 18px", true),
            2 => ("22px", "600", "32px 0 14px", true),
            3 => ("19px", "600", "30px 0 9px", false),
            _ => ("16px", "600", "22px 0 6px", false),
        };
        self.out.push_str("<h");
        self.out.push(tag);
        // GitHub's heading id stays: Word turns it into a bookmark and TOC
        // links keep working. Every other app id is stripped.
        if !id.is_empty() {
            self.out.push_str(" id=\"");
            escape_attr(&mut self.out, id);
            self.out.push('"');
        }
        self.out.push_str(" style=\"margin:");
        self.out.push_str(margin);
        self.out.push_str(";font-family:");
        self.out.push_str(SANS);
        self.out.push_str(";font-size:");
        self.out.push_str(size);
        self.out.push_str(";font-weight:");
        self.out.push_str(weight);
        self.out.push_str(";line-height:1.3;color:");
        self.out.push_str(TEXT);
        if border {
            self.out.push_str(";padding-bottom:7px;border-bottom:1px solid ");
            self.out.push_str(BORDER);
        }
        self.out.push_str("\">");
        self.runs(runs);
        self.out.push_str("</h");
        self.out.push(tag);
        self.out.push_str(">\n");
    }

    fn paragraph(&mut self, runs: &[ExportRun]) {
        self.out.push_str("<p style=\"");
        self.out.push_str(&self.paragraph_style());
        self.out.push_str("\">");
        self.runs(runs);
        self.out.push_str("</p>\n");
    }

    fn paragraph_style(&self) -> String {
        let color = if self.quote_depth > 0 || self.note_mode { MUTED } else { TEXT };
        let size = if self.note_mode { "14px" } else { "16px" };
        format!("margin:0 0 16px;font-family:{SANS};font-size:{size};line-height:1.7;color:{color}")
    }

    fn list(&mut self, list: &ExportList) {
        let tag = if list.ordered { "ol" } else { "ul" };
        let has_tasks = list.items.iter().any(|item| item.task.is_some());
        self.out.push('<');
        self.out.push_str(tag);
        if list.ordered && list.start != 1 {
            self.out.push_str(" start=\"");
            self.out.push_str(&list.start.to_string());
            self.out.push('"');
        }
        self.out.push_str(" style=\"margin:0 0 16px;padding-left:");
        self.out.push_str(if has_tasks { "0" } else { "26px" });
        self.out.push(';');
        if has_tasks {
            self.out.push_str("list-style:none;");
        }
        self.out.push_str("\">\n");
        for item in &list.items {
            self.item(item, list.tight, has_tasks);
        }
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push_str(">\n");
    }

    fn item(&mut self, item: &crate::model::ExportItem, tight: bool, task_list: bool) {
        self.out.push_str("<li style=\"margin:0 0 5px;");
        if task_list || item.task.is_some() {
            self.out.push_str("list-style:none;");
        }
        self.out.push_str("font-family:");
        self.out.push_str(SANS);
        self.out.push_str(";font-size:16px;line-height:1.7;color:");
        self.out.push_str(if self.quote_depth > 0 { MUTED } else { TEXT });
        self.out.push_str("\">");
        let (first, rest) = match item.blocks.as_slice() {
            [ExportBlock::Para(runs), rest @ ..] => (Some(runs), rest),
            blocks => (None, blocks),
        };
        if tight {
            if let Some(checked) = item.task {
                self.task_box(checked);
            }
            if let Some(runs) = first {
                self.runs(runs);
            }
            for block in rest {
                self.block(block);
            }
        } else {
            if let Some(runs) = first {
                self.out.push_str("<p style=\"");
                self.out.push_str(&self.paragraph_style());
                self.out.push_str("\">");
                if let Some(checked) = item.task {
                    self.task_box(checked);
                }
                self.runs(runs);
                self.out.push_str("</p>");
            } else if let Some(checked) = item.task {
                self.task_box(checked);
            }
            for block in rest {
                self.block(block);
            }
        }
        self.out.push_str("</li>\n");
    }

    fn task_box(&mut self, checked: bool) {
        self.out.push_str("<span style=\"font-family:");
        self.out.push_str(SANS);
        self.out.push_str(";font-size:15px;color:");
        self.out.push_str(if checked { "#1a7f37" } else { MUTED });
        self.out.push_str("\">");
        self.out.push(if checked { '\u{2611}' } else { '\u{2610}' });
        self.out.push_str("</span>&nbsp; ");
    }

    fn code_table(&mut self, lang: Option<&str>, text: &str) {
        let lang = lang.map(str::trim).filter(|lang| !lang.is_empty());
        self.out.push_str(
            "<table cellspacing=\"0\" cellpadding=\"0\" style=\"border-collapse:collapse;width:100%;margin:0 0 16px;\
             border:1px solid ",
        );
        self.out.push_str(BORDER);
        self.out.push_str(";background-color:");
        self.out.push_str(SURFACE);
        self.out.push_str("\">\n<tbody>\n");
        if let Some(lang) = lang {
            self.out.push_str("<tr><td style=\"padding:4px 14px;border-bottom:1px solid ");
            self.out.push_str(BORDER);
            self.out.push_str(";font-family:");
            self.out.push_str(MONO);
            self.out.push_str(";font-size:11.5px;color:");
            self.out.push_str(MUTED);
            self.out.push_str(";text-align:left;vertical-align:top\">");
            escape_text(&mut self.out, &lang.to_ascii_lowercase());
            self.out.push_str("</td></tr>\n");
        }
        self.out.push_str("<tr><td style=\"padding:12px 14px;font-family:");
        self.out.push_str(MONO);
        self.out.push_str(";font-size:13.5px;line-height:1.55;color:");
        self.out.push_str(TEXT);
        self.out.push_str(";text-align:left;vertical-align:top\">");
        self.code_text(lang, text);
        self.out.push_str("</td></tr>\n</tbody>\n</table>\n");
    }

    /// Code with its indentation made paste-safe: newlines become `<br>`,
    /// leading spaces and tabs become non-breaking spaces. Interior spaces stay
    /// soft so long lines still wrap.
    fn code_text(&mut self, lang: Option<&str>, text: &str) {
        let tokens =
            lang.and_then(Lang::from_name).map(|lang| self.highlighter.highlight(lang, text)).unwrap_or_default();
        let mut at_line_start = true;
        if tokens.is_empty() {
            escape_code(&mut self.out, text, &mut at_line_start);
            return;
        }
        let mut cursor = 0;
        for (range, token) in tokens {
            if range.start < cursor
                || range.end > text.len()
                || !text.is_char_boundary(range.start)
                || !text.is_char_boundary(range.end)
            {
                continue;
            }
            if range.start > cursor {
                escape_code(&mut self.out, &text[cursor..range.start], &mut at_line_start);
            }
            let (color, italic) = token_color(token.name());
            self.out.push_str("<span style=\"");
            if let Some(color) = color {
                self.out.push_str("color:");
                self.out.push_str(color);
            }
            if italic {
                self.out.push_str(";font-style:italic");
            }
            self.out.push_str("\">");
            escape_code(&mut self.out, &text[range.clone()], &mut at_line_start);
            self.out.push_str("</span>");
            cursor = range.end;
        }
        if cursor < text.len() {
            escape_code(&mut self.out, &text[cursor..], &mut at_line_start);
        }
    }

    fn table(&mut self, table: &crate::model::ExportTable) {
        self.out.push_str(
            "<table cellspacing=\"0\" cellpadding=\"0\" style=\"border-collapse:collapse;border-spacing:0;width:100%;\
             margin:0 0 16px;border:1px solid ",
        );
        self.out.push_str(BORDER);
        self.out.push_str("\">\n");
        if !table.head.is_empty() {
            self.out.push_str("<thead>\n<tr>\n");
            for (column, cell) in table.head.iter().enumerate() {
                self.out.push_str("<th scope=\"col\" style=\"padding:6px 13px;border:1px solid ");
                self.out.push_str(BORDER);
                self.out.push_str(";background-color:");
                self.out.push_str(SURFACE);
                self.out.push_str(";font-family:");
                self.out.push_str(SANS);
                self.out.push_str(";font-size:15px;font-weight:600;color:");
                self.out.push_str(TEXT);
                self.out.push(';');
                self.align_style(table.align.get(column).copied().unwrap_or_default());
                self.out.push_str(";vertical-align:top\">");
                self.runs(cell);
                self.out.push_str("</th>\n");
            }
            self.out.push_str("</tr>\n</thead>\n");
        }
        if !table.rows.is_empty() {
            self.out.push_str("<tbody>\n");
            for row in &table.rows {
                self.out.push_str("<tr>\n");
                for (column, cell) in row.iter().enumerate() {
                    self.out.push_str("<td style=\"padding:6px 13px;border:1px solid ");
                    self.out.push_str(BORDER);
                    self.out.push_str(";font-family:");
                    self.out.push_str(SANS);
                    self.out.push_str(";font-size:15px;color:");
                    self.out.push_str(TEXT);
                    self.out.push(';');
                    self.align_style(table.align.get(column).copied().unwrap_or_default());
                    self.out.push_str(";vertical-align:top\">");
                    self.runs(cell);
                    self.out.push_str("</td>\n");
                }
                self.out.push_str("</tr>\n");
            }
            self.out.push_str("</tbody>\n");
        }
        self.out.push_str("</table>\n");
    }

    fn align_style(&mut self, align: Align) {
        let value = match align {
            Align::None | Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
        };
        self.out.push_str("text-align:");
        self.out.push_str(value);
    }

    fn block_image(&mut self, image: &ExportImage) {
        self.out.push_str("<p style=\"margin:0 0 16px;text-align:center\">");
        self.image(image);
        self.out.push_str("</p>\n");
        if let Some(title) = &image.title
            && image.asset.and_then(|id| self.doc.asset(id)).is_some()
        {
            self.out.push_str("<p style=\"margin:-8px 0 16px;text-align:center;font-family:");
            self.out.push_str(SANS);
            self.out.push_str(";font-size:14px;color:");
            self.out.push_str(MUTED);
            self.out.push_str("\">");
            escape_text(&mut self.out, title);
            self.out.push_str("</p>\n");
        }
    }

    fn diagram(&mut self, source: &str, asset: Option<AssetId>) {
        if let Some(asset) = asset.and_then(|id| self.doc.asset(id)).filter(|asset| asset.mime.starts_with("image/")) {
            let alt = self.labels.mermaid.clone();
            self.out.push_str("<p style=\"margin:0 0 16px;text-align:center\">");
            self.image_element(asset, &alt, None);
            self.out.push_str("</p>\n");
        } else {
            self.code_table(Some("mermaid"), source);
        }
    }

    fn image(&mut self, image: &ExportImage) {
        match image.asset.and_then(|id| self.doc.asset(id)).filter(|asset| asset.mime.starts_with("image/")) {
            Some(asset) => self.image_element(asset, &image.alt, image.title.as_deref()),
            None => {
                let fallback = if !image.alt.is_empty() {
                    image.alt.as_str()
                } else if !image.src.is_empty() {
                    image.src.as_str()
                } else {
                    self.labels.image.as_str()
                };
                self.out.push_str("<span style=\"font-family:");
                self.out.push_str(SANS);
                self.out.push_str(";font-size:15px;color:");
                self.out.push_str(MUTED);
                self.out.push_str(";font-style:italic\">");
                escape_text(&mut self.out, fallback);
                self.out.push_str("</span>");
            }
        }
    }

    fn image_element(&mut self, asset: &ExportAsset, alt: &str, title: Option<&str>) {
        self.out.push_str("<img src=\"");
        self.out.push_str(&asset.data_uri());
        self.out.push_str("\" alt=\"");
        escape_attr(&mut self.out, alt);
        self.out.push('"');
        if let Some(title) = title {
            self.out.push_str(" title=\"");
            escape_attr(&mut self.out, title);
            self.out.push('"');
        }
        self.out.push_str(" style=\"max-width:100%;height:auto;border-radius:6px\">");
    }

    fn toc(&mut self, entries: &[ExportTocEntry]) {
        if entries.is_empty() {
            return;
        }
        self.out.push_str("<nav aria-label=\"");
        escape_attr(&mut self.out, &self.toc_label);
        self.out.push_str("\" style=\"margin:0 0 24px;padding:12px 16px;background-color:");
        self.out.push_str(SURFACE);
        self.out.push_str(";border:1px solid ");
        self.out.push_str(BORDER);
        self.out.push_str(";border-radius:6px\">\n<p style=\"margin:0;font-family:");
        self.out.push_str(SANS);
        self.out.push_str(";font-size:15px;font-weight:600;color:");
        self.out.push_str(TEXT);
        self.out.push_str("\">");
        escape_text(&mut self.out, &self.toc_label);
        self.out.push_str("</p>\n");
        self.toc_level(entries, true);
        self.out.push_str("</nav>\n");
    }

    fn toc_level(&mut self, entries: &[ExportTocEntry], top: bool) {
        self.out.push_str("<ul style=\"margin:4px 0 0;padding-left:");
        self.out.push_str(if top { "0" } else { "20px" });
        self.out.push_str(";list-style:none\">\n");
        for entry in entries {
            self.out.push_str("<li style=\"margin:3px 0;font-family:");
            self.out.push_str(SANS);
            self.out.push_str(";font-size:15px;line-height:1.6\"><a href=\"#");
            escape_attr(&mut self.out, &entry.id);
            self.out.push_str("\" style=\"color:");
            self.out.push_str(ACCENT);
            self.out.push_str(";text-decoration:none\">");
            escape_text(&mut self.out, &entry.text);
            self.out.push_str("</a>");
            if !entry.children.is_empty() {
                self.toc_level(&entry.children, false);
            }
            self.out.push_str("</li>\n");
        }
        self.out.push_str("</ul>\n");
    }

    fn footnotes(&mut self) {
        if self.doc.footnotes.is_empty() {
            return;
        }
        self.out.push_str("<section aria-label=\"");
        escape_attr(&mut self.out, &self.footnotes_label);
        self.out.push_str("\" style=\"margin-top:32px;padding-top:16px;border-top:1px solid ");
        self.out.push_str(BORDER);
        self.out.push_str(";color:");
        self.out.push_str(MUTED);
        self.out.push_str("\">\n<hr style=\"height:0;margin:0 0 16px;border:0;border-top:1px solid ");
        self.out.push_str(BORDER);
        self.out.push_str("\">\n<ol style=\"margin:0;padding-left:24px\">\n");
        for footnote in &self.doc.footnotes {
            self.out.push_str("<li id=\"fn-");
            self.out.push_str(&footnote.number.to_string());
            self.out.push_str("\" style=\"margin:0 0 8px;font-family:");
            self.out.push_str(SANS);
            self.out.push_str(";font-size:14px;line-height:1.6;color:");
            self.out.push_str(MUTED);
            self.out.push_str("\">\n");
            self.note_mode = true;
            for block in &footnote.blocks {
                self.block(block);
            }
            self.note_mode = false;
            self.out.push_str("<p style=\"margin:0 0 8px;font-size:14px\"><a href=\"#fnref-");
            self.out.push_str(&footnote.number.to_string());
            self.out.push_str("\" style=\"color:");
            self.out.push_str(ACCENT);
            self.out.push_str(";text-decoration:none\">\u{21a9}</a></p>\n</li>\n");
        }
        self.out.push_str("</ol>\n</section>\n");
    }

    // ---- inline ---------------------------------------------------------

    fn runs(&mut self, runs: &[ExportRun]) {
        for run in runs {
            self.run(run);
        }
    }

    fn run(&mut self, run: &ExportRun) {
        match run {
            ExportRun::Text { text, style, link } => {
                let href = self.link_href(link);
                self.open_link(link, href.as_deref());
                self.open_style(*style);
                escape_text(&mut self.out, text);
                self.close_style(*style);
                self.close_link(href.as_deref());
            }
            ExportRun::Math { tex, display, asset, link } => {
                let href = self.link_href(link);
                self.open_link(link, href.as_deref());
                self.math_image(tex, *asset, *display);
                self.close_link(href.as_deref());
            }
            ExportRun::Image { image, link } => {
                let href = self.link_href(link);
                self.open_link(link, href.as_deref());
                self.image(image);
                self.close_link(href.as_deref());
            }
            ExportRun::Break => self.out.push_str("<br>"),
            ExportRun::FootnoteRef { number, occurrence, .. } => {
                self.out.push_str("<sup style=\"font-size:75%;line-height:0\"><a href=\"#fn-");
                self.out.push_str(&number.to_string());
                self.out.push_str("\" id=\"fnref-");
                self.out.push_str(&number.to_string());
                if *occurrence > 0 {
                    self.out.push('-');
                    self.out.push_str(&occurrence.to_string());
                }
                self.out.push_str("\" style=\"color:");
                self.out.push_str(ACCENT);
                self.out.push_str(";text-decoration:none\">");
                self.out.push_str(&number.to_string());
                self.out.push_str("</a></sup>");
            }
            ExportRun::Html(node) => self.html_node(node),
        }
    }

    fn open_style(&mut self, style: RunStyle) {
        if style.strong {
            self.out.push_str("<strong style=\"font-weight:600\">");
        }
        if style.em {
            self.out.push_str("<em style=\"font-style:italic\">");
        }
        if style.strike {
            self.out.push_str("<s style=\"text-decoration:line-through\">");
        }
        if style.mark {
            self.out.push_str("<mark style=\"background-color:");
            self.out.push_str(MARK);
            self.out.push_str(";color:");
            self.out.push_str(TEXT);
            self.out.push_str(";padding:1px 2px\">");
        }
        if style.sup {
            self.out.push_str("<sup style=\"font-size:75%;line-height:0\">");
        } else if style.sub {
            self.out.push_str("<sub style=\"font-size:75%;line-height:0\">");
        }
        if style.kbd {
            self.out.push_str("<kbd style=\"font-family:");
            self.out.push_str(MONO);
            self.out.push_str(";font-size:13.5px;background-color:");
            self.out.push_str(SURFACE);
            self.out.push_str(";border:1px solid ");
            self.out.push_str(BORDER);
            self.out.push_str(";border-radius:5px;padding:1px 6px\">");
        }
        if style.code {
            self.out.push_str("<code style=\"font-family:");
            self.out.push_str(MONO);
            self.out.push_str(";font-size:13.5px;background-color:");
            self.out.push_str(SURFACE_2);
            self.out.push_str(";border:1px solid ");
            self.out.push_str(BORDER);
            self.out.push_str(";border-radius:4px;padding:1px 5px;color:");
            self.out.push_str(TEXT);
            self.out.push_str("\">");
        }
        if style.underline {
            self.out.push_str("<u style=\"text-decoration:underline\">");
        }
    }

    fn close_style(&mut self, style: RunStyle) {
        if style.underline {
            self.out.push_str("</u>");
        }
        if style.code {
            self.out.push_str("</code>");
        }
        if style.kbd {
            self.out.push_str("</kbd>");
        }
        if style.sup || style.sub {
            self.out.push_str(if style.sup { "</sup>" } else { "</sub>" });
        }
        if style.mark {
            self.out.push_str("</mark>");
        }
        if style.strike {
            self.out.push_str("</s>");
        }
        if style.em {
            self.out.push_str("</em>");
        }
        if style.strong {
            self.out.push_str("</strong>");
        }
    }

    fn open_link(&mut self, link: &Option<ExportLink>, href: Option<&str>) {
        let Some(href) = href else { return };
        self.out.push_str("<a href=\"");
        escape_attr(&mut self.out, href);
        self.out.push('"');
        if let Some(title) = link.as_ref().and_then(|link| link.title.as_deref()) {
            self.out.push_str(" title=\"");
            escape_attr(&mut self.out, title);
            self.out.push('"');
        }
        self.out.push_str(" style=\"color:");
        self.out.push_str(ACCENT);
        self.out.push_str(";text-decoration:underline\">");
    }

    fn close_link(&mut self, href: Option<&str>) {
        if href.is_some() {
            self.out.push_str("</a>");
        }
    }

    /// The href a link may carry in the copy. Already-resolved `file:///`
    /// links stay; a relative link is resolved against the source folder when
    /// one is known; script, data and unknown schemes are dropped.
    fn link_href(&self, link: &Option<ExportLink>) -> Option<String> {
        let href = link.as_ref().map(|link| link.href.trim()).filter(|href| !href.is_empty())?;
        if href.chars().any(char::is_control) {
            return None;
        }
        if href.starts_with('#') || href.starts_with("file:///") {
            return Some(href.to_owned());
        }
        match crate::link::scheme(href).map(str::to_ascii_lowercase).as_deref() {
            Some("http" | "https" | "mailto") => Some(href.to_owned()),
            Some(_) => None,
            None => crate::link::resolve(href, self.source_path),
        }
    }

    fn math_image(&mut self, tex: &str, asset: Option<AssetId>, display: bool) {
        let Some(asset) = asset.and_then(|id| self.doc.asset(id)).filter(|asset| asset.mime.starts_with("image/"))
        else {
            self.out.push_str("<code style=\"font-family:");
            self.out.push_str(MONO);
            self.out.push_str(";font-size:15px;background-color:");
            self.out.push_str(SURFACE_2);
            self.out.push_str(";padding:1px 4px;border-radius:4px\">");
            escape_text(&mut self.out, tex);
            self.out.push_str("</code>");
            return;
        };
        self.out.push_str("<img src=\"");
        self.out.push_str(&asset.data_uri());
        self.out.push_str("\" alt=\"");
        escape_attr(&mut self.out, tex);
        self.out.push('"');
        if let Some(width) = asset.width {
            self.out.push_str(" width=\"");
            self.out.push_str(&width.to_string());
            self.out.push('"');
        }
        if let Some(height) = asset.height {
            self.out.push_str(" height=\"");
            self.out.push_str(&height.to_string());
            self.out.push('"');
        }
        self.out.push_str(" style=\"max-width:100%;height:auto;display:inline-block;");
        if display {
            self.out.push_str("vertical-align:middle\">");
        } else if let Some(baseline) = asset.baseline.filter(|baseline| *baseline > 0) {
            self.out.push_str("vertical-align:-");
            self.out.push_str(&baseline.to_string());
            self.out.push_str("px\">");
        } else {
            self.out.push_str("vertical-align:baseline\">");
        }
    }

    // ---- preserved HTML --------------------------------------------------

    fn html_node(&mut self, node: &HtmlNode) {
        match node.tag {
            HtmlTag::Fragment => self.html_children(node),
            HtmlTag::Img => self.html_img(node),
            HtmlTag::Picture => {
                if let Some(img) = node.children.iter().find_map(|child| match child {
                    HtmlChild::Node(child) if child.tag == HtmlTag::Img => Some(child),
                    _ => None,
                }) {
                    self.html_img(img);
                } else {
                    self.html_children(node);
                }
            }
            HtmlTag::Video | HtmlTag::Audio => self.html_media(node),
            HtmlTag::Source | HtmlTag::Track => {}
            HtmlTag::Br => self.out.push_str("<br>"),
            // Word has no details element: the card becomes a plain bordered
            // block so its content stays visible.
            HtmlTag::Details => {
                self.out.push_str("<div style=\"margin:0 0 16px;padding:8px 16px;background-color:");
                self.out.push_str(SURFACE);
                self.out.push_str(";border:1px solid ");
                self.out.push_str(BORDER);
                self.out.push_str(";border-radius:6px\">");
                self.html_children(node);
                self.out.push_str("</div>");
            }
            HtmlTag::Summary => {
                self.out.push_str("<p style=\"margin:0 0 8px;font-family:");
                self.out.push_str(SANS);
                self.out.push_str(";font-size:15px;font-weight:600;color:");
                self.out.push_str(TEXT);
                self.out.push_str("\">");
                self.html_children(node);
                self.out.push_str("</p>");
            }
            tag => {
                let name = tag.name();
                self.out.push('<');
                self.out.push_str(name);
                self.html_attrs(node, name);
                self.out.push('>');
                self.html_children(node);
                self.out.push_str("</");
                self.out.push_str(name);
                self.out.push('>');
            }
        }
    }

    fn html_children(&mut self, node: &HtmlNode) {
        for child in &node.children {
            match child {
                HtmlChild::Text(text) => escape_text(&mut self.out, text),
                HtmlChild::Node(child) => self.html_node(child),
            }
        }
    }

    fn html_img(&mut self, node: &HtmlNode) {
        let alt = node.attr("alt").unwrap_or_default();
        let Some(id) =
            node.attr("src").and_then(|src| src.strip_prefix("lmdv-asset:")).and_then(|id| id.parse::<u32>().ok())
        else {
            if !alt.is_empty() {
                escape_text(&mut self.out, alt);
            }
            return;
        };
        let Some(asset) = self.doc.asset(AssetId(id)).filter(|asset| asset.mime.starts_with("image/")) else {
            return;
        };
        self.image_element(asset, alt, node.attr("title"));
    }

    fn html_media(&mut self, node: &HtmlNode) {
        let name = node.tag.name();
        let Some(src) = node.attr("src").filter(|src| href_is_writable(src)) else {
            escape_text(&mut self.out, name);
            return;
        };
        self.out.push_str("<span style=\"font-family:");
        self.out.push_str(SANS);
        self.out.push_str(";font-size:15px\"><a href=\"");
        escape_attr(&mut self.out, src);
        self.out.push_str("\" style=\"color:");
        self.out.push_str(ACCENT);
        self.out.push_str(";text-decoration:underline\">");
        escape_text(&mut self.out, name);
        self.out.push_str("</a></span>");
    }

    fn html_attrs(&mut self, node: &HtmlNode, name: &str) {
        for (attr, value) in &node.attrs {
            match attr.as_str() {
                "title" | "alt" => {
                    self.out.push(' ');
                    self.out.push_str(attr);
                    self.out.push_str("=\"");
                    escape_attr(&mut self.out, value);
                    self.out.push('"');
                }
                "href" => {
                    if let Some(href) = self.html_href(value) {
                        self.out.push_str(" href=\"");
                        escape_attr(&mut self.out, &href);
                        self.out.push('"');
                    }
                }
                "start" if name == "ol" && value.chars().all(|ch| ch.is_ascii_digit()) && !value.is_empty() => {
                    self.out.push_str(" start=\"");
                    self.out.push_str(value);
                    self.out.push('"');
                }
                "colspan" | "rowspan"
                    if matches!(node.tag, HtmlTag::Th | HtmlTag::Td)
                        && value.chars().all(|ch| ch.is_ascii_digit())
                        && !value.is_empty() =>
                {
                    self.out.push(' ');
                    self.out.push_str(attr);
                    self.out.push_str("=\"");
                    self.out.push_str(value);
                    self.out.push('"');
                }
                _ => {}
            }
        }
        let mut style = String::from(tag_style(node.tag));
        // A preserved text block carries its own base too: paste targets do
        // not reliably inherit typography from the fragment wrapper.
        let (size, color) = match node.tag {
            HtmlTag::P | HtmlTag::Li => ("16px", TEXT),
            HtmlTag::Th | HtmlTag::Td => ("15px", TEXT),
            HtmlTag::Blockquote => ("16px", MUTED),
            _ => ("", ""),
        };
        if !size.is_empty() {
            if !style.is_empty() {
                style.push(';');
            }
            style.push_str("font-family:");
            style.push_str(SANS);
            style.push_str(";font-size:");
            style.push_str(size);
            style.push_str(";line-height:1.7;color:");
            style.push_str(color);
        }
        let align = matches!(node.tag, HtmlTag::Th | HtmlTag::Td)
            .then(|| node.attr("align").map(str::to_ascii_lowercase))
            .flatten()
            .filter(|align| matches!(align.as_str(), "left" | "center" | "right"));
        if !style.is_empty() || align.is_some() {
            self.out.push_str(" style=\"");
            self.out.push_str(&style);
            if let Some(align) = align {
                self.out.push_str(";text-align:");
                self.out.push_str(&align);
            }
            self.out.push('"');
        }
    }

    fn html_href(&self, href: &str) -> Option<String> {
        let href = href.trim();
        if href.is_empty() || href.chars().any(char::is_control) {
            return None;
        }
        if href.starts_with('#') || href.starts_with("file:///") {
            return Some(href.to_owned());
        }
        match crate::link::scheme(href).map(str::to_ascii_lowercase).as_deref() {
            Some("http" | "https" | "mailto") => Some(href.to_owned()),
            Some(_) => None,
            None => crate::link::resolve(href, self.source_path),
        }
    }
}

/// The alert's accent, tint and paste-safe symbol (share.ts:280-289).
fn callout_colors(kind: CalloutKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        CalloutKind::Note => ("#0969da", "#ddf4ff", "\u{2139}\u{fe0f}"),
        CalloutKind::Tip => ("#1a7f37", "#dafbe1", "\u{1f4a1}"),
        CalloutKind::Important => ("#8250df", "#fbefff", "\u{2757}"),
        CalloutKind::Warning => ("#9a6700", "#fff8c5", "\u{26a0}\u{fe0f}"),
        CalloutKind::Caution => ("#cf222e", "#ffebe9", "\u{26d4}"),
    }
}

/// GitHub Light token colours, mirroring the HTML export's stylesheet.
fn token_color(name: &str) -> (Option<&'static str>, bool) {
    match name.split('.').next().unwrap_or("token") {
        "keyword" => (Some("#cf222e"), false),
        "string" => (Some("#0a3069"), false),
        "comment" => (Some(MUTED), true),
        "number" | "constant" | "variable" | "property" | "attribute" | "entity" => (Some("#0550ae"), false),
        "function" => (Some("#8250df"), false),
        "type" => (Some("#953800"), false),
        "tag" => (Some("#116329"), false),
        "meta" | "label" | "namespace" => (Some(MUTED), false),
        "invalid" => (Some("#cf222e"), false),
        _ => (None, false),
    }
}

/// The inline style of a preserved HTML tag that has a visual equivalent.
fn tag_style(tag: HtmlTag) -> &'static str {
    match tag {
        HtmlTag::A => "color:#0969da;text-decoration:underline",
        HtmlTag::B | HtmlTag::Strong => "font-weight:600",
        HtmlTag::I | HtmlTag::Em => "font-style:italic",
        HtmlTag::S | HtmlTag::Del => "text-decoration:line-through",
        HtmlTag::U | HtmlTag::Ins => "text-decoration:underline",
        HtmlTag::Mark => "background-color:#fff8c5;color:#1f2328;padding:1px 2px",
        HtmlTag::Kbd => {
            "font-family:ui-monospace,Cascadia Code,Source Code Pro,Menlo,Consolas,monospace;font-size:13.5px;\
             background-color:#f6f8fa;border:1px solid #d1d9e0;border-radius:5px;padding:1px 6px"
        }
        HtmlTag::Code => {
            "font-family:ui-monospace,Cascadia Code,Source Code Pro,Menlo,Consolas,monospace;font-size:13.5px;\
             background-color:#eff1f3;border:1px solid #d1d9e0;border-radius:4px;padding:1px 5px;color:#1f2328"
        }
        HtmlTag::Pre => {
            "margin:0 0 16px;padding:12px 14px;background-color:#f6f8fa;border:1px solid #d1d9e0;border-radius:6px;\
             font-family:ui-monospace,Cascadia Code,Source Code Pro,Menlo,Consolas,monospace;font-size:13.5px;\
             line-height:1.55;color:#1f2328;white-space:pre-wrap"
        }
        HtmlTag::Blockquote => "margin:0 0 16px;padding:2px 0 2px 16px;border-left:3px solid #d1d9e0;color:#59636e",
        HtmlTag::Table => {
            "border-collapse:collapse;border-spacing:0;width:100%;margin:0 0 16px;border:1px solid #d1d9e0"
        }
        HtmlTag::Th => {
            "padding:6px 13px;border:1px solid #d1d9e0;background-color:#f6f8fa;font-weight:600;text-align:left;\
             vertical-align:top;font-size:15px;color:#1f2328"
        }
        HtmlTag::Td => {
            "padding:6px 13px;border:1px solid #d1d9e0;text-align:left;vertical-align:top;font-size:15px;color:#1f2328"
        }
        HtmlTag::Ul | HtmlTag::Ol => "margin:0 0 16px;padding-left:26px",
        HtmlTag::Li => "margin:0 0 5px",
        HtmlTag::P => "margin:0 0 16px;font-size:16px;line-height:1.7",
        HtmlTag::Sup | HtmlTag::Sub => "font-size:75%;line-height:0",
        HtmlTag::Div => "",
        _ => "",
    }
}

/// Whether an already-resolved href is safe to write.
fn href_is_writable(href: &str) -> bool {
    if href.chars().any(char::is_control) {
        return false;
    }
    if href.starts_with('#') || href.starts_with("file:///") {
        return true;
    }
    match crate::link::scheme(href).map(str::to_ascii_lowercase).as_deref() {
        Some("http" | "https" | "mailto") => true,
        Some(_) => false,
        None => true,
    }
}

/// Escapes text content.
fn escape_text(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\0' => {}
            other => out.push(other),
        }
    }
}

/// Escapes an attribute value.
fn escape_attr(out: &mut String, text: &str) {
    escape_text(out, text);
}

/// Escapes code while hardening whitespace for Word: a newline is `<br>`, a
/// tab four non-breaking spaces, and a space at the start of a line a
/// non-breaking space.
fn escape_code(out: &mut String, text: &str, at_line_start: &mut bool) {
    for character in text.chars() {
        match character {
            '\n' => {
                out.push_str("<br>");
                *at_line_start = true;
            }
            '\t' => {
                for _ in 0..4 {
                    out.push_str("&nbsp;");
                }
            }
            ' ' if *at_line_start => out.push_str("&nbsp;"),
            '&' => {
                out.push_str("&amp;");
                *at_line_start = false;
            }
            '<' => {
                out.push_str("&lt;");
                *at_line_start = false;
            }
            '>' => {
                out.push_str("&gt;");
                *at_line_start = false;
            }
            '"' => {
                out.push_str("&quot;");
                *at_line_start = false;
            }
            '\'' => {
                out.push_str("&#39;");
                *at_line_start = false;
            }
            '\0' => {}
            other => {
                out.push(other);
                *at_line_start = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_attributes_are_escaped() {
        let mut out = String::new();
        escape_text(&mut out, "<b> & \"quoted\" 'x'");
        assert_eq!(out, "&lt;b&gt; &amp; &quot;quoted&quot; &#39;x&#39;");
    }

    #[test]
    fn code_whitespace_is_hardened_for_paste_targets() {
        let mut out = String::new();
        let mut at_line_start = true;
        escape_code(&mut out, "  let\tx = 1;\nif a < b & c {\n\treturn;\n}", &mut at_line_start);
        assert_eq!(
            out,
            "&nbsp;&nbsp;let&nbsp;&nbsp;&nbsp;&nbsp;x = 1;<br>if a &lt; b &amp; c {<br>&nbsp;&nbsp;&nbsp;&nbsp;return;<br>}"
        );
    }

    #[test]
    fn callouts_carry_the_original_symbols_and_colours() {
        assert_eq!(callout_colors(CalloutKind::Note), ("#0969da", "#ddf4ff", "ℹ️"));
        assert_eq!(callout_colors(CalloutKind::Caution).0, "#cf222e");
    }

    #[test]
    fn token_categories_map_to_github_light_colours() {
        assert_eq!(token_color("keyword.control"), (Some("#cf222e"), false));
        assert_eq!(token_color("comment.line"), (Some(MUTED), true));
        assert_eq!(token_color("odd.thing"), (None, false));
    }
}
