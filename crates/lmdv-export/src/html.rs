//! Self-contained HTML serializer.
//!
//! One file, no scripts, no network: the stylesheet is inline, every picture and
//! formula is a `data:` URI, and the HTML subset the parser allowed is written
//! back with its dangerous attributes dropped. The layout is the original's
//! export layout — light theme, 16 px sans body, 760 px column — never the
//! reader's theme or typography (X-07, X-10).

use lmdv_i18n::{Key, text_in};
use lmdv_md::{Align, CalloutKind, HtmlChild, HtmlNode, HtmlTag};
use rune_highlights::{Highlighter, Lang};

use crate::labels::ExportLabels;
use crate::model::{
    ExportAsset, ExportBlock, ExportDoc, ExportImage, ExportLink, ExportList, ExportRun, ExportTocEntry, RunStyle,
};

/// The localized front-matter title.
const FRONT_MATTER: Key = lmdv_i18n::key("render.frontMatter");
/// The localized table-of-contents name.
const TOC: Key = lmdv_i18n::key("render.toc");

/// Serializes `doc` as one standalone HTML file.
pub(crate) fn render(doc: &ExportDoc) -> String {
    let mut writer = Writer::new(doc);
    writer.document();
    writer.out
}

struct Writer<'a> {
    doc: &'a ExportDoc,
    out: String,
    highlighter: Highlighter,
    front_matter_label: String,
    toc_label: String,
    labels: ExportLabels,
}

impl<'a> Writer<'a> {
    fn new(doc: &'a ExportDoc) -> Self {
        let locale = lmdv_i18n::negotiate(std::iter::once(doc.lang.as_str()));
        Self {
            doc,
            out: String::new(),
            highlighter: Highlighter::new(),
            front_matter_label: text_in(locale, FRONT_MATTER).to_owned(),
            toc_label: text_in(locale, TOC).to_owned(),
            labels: ExportLabels::for_lang(&doc.lang),
        }
    }

    fn document(&mut self) {
        let lang = if self.doc.lang.trim().is_empty() { "en" } else { self.doc.lang.as_str() };
        self.out.push_str("<!doctype html>\n<html lang=\"");
        escape_attr(&mut self.out, lang);
        self.out.push_str("\">\n<head>\n<meta charset=\"utf-8\">\n");
        self.out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>");
        escape_text(&mut self.out, &self.doc.title);
        self.out.push_str("</title>\n<style>\n");
        self.out.push_str(STYLE);
        self.out.push_str("</style>\n</head>\n<body>\n<article class=\"markdown-body\">\n");
        self.front_matter();
        for block in &self.doc.blocks {
            self.block(block);
        }
        self.out.push_str("</article>\n</body>\n</html>\n");
    }

    fn block(&mut self, block: &ExportBlock) {
        match block {
            ExportBlock::Heading { level, id, runs } => {
                let level = (*level).clamp(1, 6);
                self.out.push_str("<h");
                self.out.push(char::from(b'0' + level));
                if !id.is_empty() {
                    self.out.push_str(" id=\"");
                    escape_attr(&mut self.out, id);
                    self.out.push('"');
                }
                self.out.push('>');
                self.runs(runs);
                self.out.push_str("</h");
                self.out.push(char::from(b'0' + level));
                self.out.push_str(">\n");
            }
            ExportBlock::Para(runs) => {
                if runs.is_empty() {
                    return;
                }
                self.out.push_str("<p>");
                self.runs(runs);
                self.out.push_str("</p>\n");
            }
            ExportBlock::List(list) => self.list(list),
            ExportBlock::Quote { callout: None, blocks } => {
                self.out.push_str("<blockquote>\n");
                for block in blocks {
                    self.block(block);
                }
                self.out.push_str("</blockquote>\n");
            }
            ExportBlock::Quote { callout: Some(callout), blocks } => {
                self.out.push_str("<aside class=\"callout callout-");
                self.out.push_str(callout_class(callout.kind));
                self.out.push_str("\" role=\"note\">\n<p class=\"callout-title\">");
                let title = callout.title.as_deref().unwrap_or_else(|| self.labels.callout(callout.kind));
                escape_text(&mut self.out, title);
                self.out.push_str("</p>\n");
                for block in blocks {
                    self.block(block);
                }
                self.out.push_str("</aside>\n");
            }
            ExportBlock::Code { lang, text } => self.code(lang.as_deref(), text),
            ExportBlock::Table(table) => self.table(table),
            ExportBlock::Math { tex, asset, .. } => {
                self.out.push_str("<div class=\"math math-display\">");
                self.math_image(tex, *asset, true);
                self.out.push_str("</div>\n");
            }
            ExportBlock::Image(image) => self.block_image(image),
            ExportBlock::Diagram { source, asset } => self.diagram(source, *asset),
            ExportBlock::Rule => self.out.push_str("<hr>\n"),
            ExportBlock::Toc { entries } => self.toc(entries),
            ExportBlock::Footnotes => self.footnotes(),
            ExportBlock::Html(node) => {
                self.html_node(node);
                self.out.push('\n');
            }
        }
    }

    fn list(&mut self, list: &ExportList) {
        let tag = if list.ordered { "ol" } else { "ul" };
        self.out.push('<');
        self.out.push_str(tag);
        if list.ordered && list.start != 1 {
            self.out.push_str(" start=\"");
            self.out.push_str(&list.start.to_string());
            self.out.push('"');
        }
        if list.items.iter().any(|item| item.task.is_some()) {
            self.out.push_str(" class=\"contains-task-list\"");
        }
        self.out.push_str(">\n");
        for item in &list.items {
            self.out.push_str("<li");
            if item.task.is_some() {
                self.out.push_str(" class=\"task-list-item\"");
            }
            self.out.push('>');
            let (first, rest) = match item.blocks.as_slice() {
                [ExportBlock::Para(runs), rest @ ..] => (Some(runs), rest),
                blocks => (None, blocks),
            };
            if list.tight {
                if let Some(checked) = item.task {
                    self.checkbox(checked, &item.label);
                }
                if let Some(runs) = first {
                    self.runs(runs);
                }
                for block in rest {
                    self.out.push('\n');
                    self.block(block);
                }
            } else {
                if let Some(runs) = first {
                    self.out.push_str("<p>");
                    if let Some(checked) = item.task {
                        self.checkbox(checked, &item.label);
                    }
                    self.runs(runs);
                    self.out.push_str("</p>\n");
                } else if let Some(checked) = item.task {
                    self.checkbox(checked, &item.label);
                }
                for block in rest {
                    self.block(block);
                }
            }
            self.out.push_str("</li>\n");
        }
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push_str(">\n");
    }

    fn checkbox(&mut self, checked: bool, label: &str) {
        self.out.push_str("<input type=\"checkbox\" class=\"task-checkbox\" disabled");
        if checked {
            self.out.push_str(" checked");
        }
        if !label.is_empty() {
            self.out.push_str(" aria-label=\"");
            escape_attr(&mut self.out, label);
            self.out.push('"');
        }
        self.out.push_str("> ");
    }

    fn code(&mut self, lang: Option<&str>, text: &str) {
        self.out.push_str("<pre><code");
        if let Some(lang) = lang.filter(|lang| !lang.is_empty()) {
            self.out.push_str(" class=\"language-");
            escape_attr(&mut self.out, lang);
            self.out.push('"');
        }
        self.out.push('>');
        self.code_text(lang, text);
        self.out.push_str("</code></pre>\n");
    }

    fn code_text(&mut self, lang: Option<&str>, text: &str) {
        let tokens =
            lang.and_then(Lang::from_name).map(|lang| self.highlighter.highlight(lang, text)).unwrap_or_default();
        if tokens.is_empty() {
            escape_text(&mut self.out, text);
            return;
        }
        let mut cursor = 0;
        for (range, token) in tokens {
            if range.start < cursor || range.end > text.len() {
                continue;
            }
            if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
                continue;
            }
            if range.start > cursor {
                escape_text(&mut self.out, &text[cursor..range.start]);
            }
            self.out.push_str("<span class=\"");
            self.out.push_str(token_class(token.name()));
            self.out.push_str("\">");
            escape_text(&mut self.out, &text[range.clone()]);
            self.out.push_str("</span>");
            cursor = range.end;
        }
        if cursor < text.len() {
            escape_text(&mut self.out, &text[cursor..]);
        }
    }

    fn table(&mut self, table: &crate::model::ExportTable) {
        self.out.push_str("<div class=\"table-wrap\">\n<table>\n");
        if !table.head.is_empty() {
            self.out.push_str("<thead>\n<tr>\n");
            for (column, cell) in table.head.iter().enumerate() {
                self.out.push_str("<th");
                self.align_attr(table.align.get(column).copied().unwrap_or_default());
                self.out.push_str(" scope=\"col\">");
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
                    self.out.push_str("<td");
                    self.align_attr(table.align.get(column).copied().unwrap_or_default());
                    self.out.push('>');
                    self.runs(cell);
                    self.out.push_str("</td>\n");
                }
                self.out.push_str("</tr>\n");
            }
            self.out.push_str("</tbody>\n");
        }
        self.out.push_str("</table>\n</div>\n");
    }

    fn align_attr(&mut self, align: Align) {
        let class = match align {
            Align::None => return,
            Align::Left => "align-left",
            Align::Center => "align-center",
            Align::Right => "align-right",
        };
        self.out.push_str(" class=\"");
        self.out.push_str(class);
        self.out.push('"');
    }

    fn block_image(&mut self, image: &ExportImage) {
        if let Some(asset) =
            image.asset.and_then(|id| self.doc.asset(id)).filter(|asset| asset.mime.starts_with("image/"))
        {
            self.out.push_str("<figure class=\"image\"><img src=\"");
            self.out.push_str(&asset.data_uri());
            self.out.push_str("\" alt=\"");
            escape_attr(&mut self.out, &image.alt);
            self.out.push('"');
            if let Some(title) = &image.title {
                self.out.push_str(" title=\"");
                escape_attr(&mut self.out, title);
                self.out.push('"');
            }
            self.out.push('>');
            if let Some(title) = &image.title {
                self.out.push_str("<figcaption>");
                escape_text(&mut self.out, title);
                self.out.push_str("</figcaption>");
            }
            self.out.push_str("</figure>\n");
        } else {
            self.out.push_str("<p class=\"missing-image\">");
            let fallback = if !image.alt.is_empty() {
                image.alt.as_str()
            } else if !image.src.is_empty() {
                image.src.as_str()
            } else {
                self.labels.image.as_str()
            };
            escape_text(&mut self.out, fallback);
            self.out.push_str("</p>\n");
        }
    }

    fn diagram(&mut self, source: &str, asset: Option<crate::model::AssetId>) {
        if let Some(asset) = asset.and_then(|id| self.doc.asset(id)).filter(|asset| asset.mime.starts_with("image/")) {
            self.out.push_str("<figure class=\"diagram\"><img src=\"");
            self.out.push_str(&asset.data_uri());
            self.out.push_str("\" alt=\"");
            escape_attr(&mut self.out, &self.labels.mermaid);
            self.out.push_str("\"></figure>\n");
        } else {
            self.out.push_str("<figure class=\"diagram diagram-source\">\n");
            self.code(Some("mermaid"), source);
            self.out.push_str("</figure>\n");
        }
    }

    fn toc(&mut self, entries: &[ExportTocEntry]) {
        if entries.is_empty() {
            return;
        }
        self.out.push_str("<nav class=\"toc\" aria-label=\"");
        escape_attr(&mut self.out, &self.toc_label);
        self.out.push_str("\">\n");
        self.toc_level(entries);
        self.out.push_str("</nav>\n");
    }

    fn toc_level(&mut self, entries: &[ExportTocEntry]) {
        self.out.push_str("<ul>\n");
        for entry in entries {
            self.out.push_str("<li><a href=\"#");
            escape_attr(&mut self.out, &entry.id);
            self.out.push_str("\">");
            escape_text(&mut self.out, &entry.text);
            self.out.push_str("</a>");
            if !entry.children.is_empty() {
                self.out.push('\n');
                self.toc_level(&entry.children);
            }
            self.out.push_str("</li>\n");
        }
        self.out.push_str("</ul>\n");
    }

    fn footnotes(&mut self) {
        if self.doc.footnotes.is_empty() {
            return;
        }
        self.out.push_str("<section class=\"footnotes\" aria-label=\"");
        escape_attr(&mut self.out, &self.labels.footnotes);
        self.out.push_str("\">\n<hr>\n<ol class=\"footnotes-list\">\n");
        for footnote in &self.doc.footnotes {
            self.out.push_str("<li id=\"fn-");
            self.out.push_str(&footnote.number.to_string());
            self.out.push_str("\" class=\"footnote-item\">\n");
            for block in &footnote.blocks {
                self.block(block);
            }
            self.out.push_str("<p class=\"footnote-back\"><a href=\"#fnref-");
            self.out.push_str(&footnote.number.to_string());
            self.out.push_str("\" class=\"footnote-backref\" aria-label=\"");
            let back = lmdv_i18n::render(&self.labels.footnote_back, &[("n", &footnote.number)]);
            escape_attr(&mut self.out, &back);
            self.out.push_str("\">↩</a></p>\n</li>\n");
        }
        self.out.push_str("</ol>\n</section>\n");
    }

    fn front_matter(&mut self) {
        let Some(front) = &self.doc.front_matter else { return };
        self.out.push_str("<details class=\"front-matter\">\n<summary>");
        escape_text(&mut self.out, &self.front_matter_label);
        self.out.push_str("</summary>\n");
        if front.valid {
            self.out.push_str("<dl>\n");
            for (key, values) in &front.entries {
                self.out.push_str("<dt>");
                escape_text(&mut self.out, key);
                self.out.push_str("</dt><dd>");
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        self.out.push_str(", ");
                    }
                    escape_text(&mut self.out, value);
                }
                self.out.push_str("</dd>\n");
            }
            self.out.push_str("</dl>\n");
        } else {
            self.out.push_str("<pre class=\"front-matter-raw\"><code>");
            escape_text(&mut self.out, &front.raw);
            self.out.push_str("</code></pre>\n");
        }
        self.out.push_str("</details>\n");
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
                self.open_link(link);
                self.open_style(*style);
                escape_text(&mut self.out, text);
                self.close_style(*style);
                self.close_link(link);
            }
            ExportRun::Math { tex, display, asset, link } => {
                self.open_link(link);
                self.math_image(tex, *asset, *display);
                self.close_link(link);
            }
            ExportRun::Image { image, link } => {
                self.open_link(link);
                self.inline_image(image);
                self.close_link(link);
            }
            ExportRun::Break => self.out.push_str("<br>\n"),
            ExportRun::FootnoteRef { number, occurrence, .. } => {
                self.out.push_str("<sup class=\"footnote-ref\"><a href=\"#fn-");
                self.out.push_str(&number.to_string());
                self.out.push_str("\" id=\"fnref-");
                self.out.push_str(&number.to_string());
                if *occurrence > 0 {
                    self.out.push('-');
                    self.out.push_str(&occurrence.to_string());
                }
                self.out.push_str("\" aria-label=\"Footnote ");
                self.out.push_str(&number.to_string());
                self.out.push_str("\">");
                self.out.push_str(&number.to_string());
                self.out.push_str("</a></sup>");
            }
            ExportRun::Html(node) => self.html_node(node),
        }
    }

    fn open_style(&mut self, style: RunStyle) {
        if style.strong {
            self.out.push_str("<strong>");
        }
        if style.em {
            self.out.push_str("<em>");
        }
        if style.strike {
            self.out.push_str("<s>");
        }
        if style.mark {
            self.out.push_str("<mark>");
        }
        if style.sup {
            self.out.push_str("<sup>");
        } else if style.sub {
            self.out.push_str("<sub>");
        }
        if style.kbd {
            self.out.push_str("<kbd>");
        }
        if style.code {
            self.out.push_str("<code>");
        }
        if style.underline {
            self.out.push_str("<u>");
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

    fn open_link(&mut self, link: &Option<ExportLink>) {
        let Some(link) = link else { return };
        if !link_is_writable(link) {
            return;
        }
        self.out.push_str("<a href=\"");
        escape_attr(&mut self.out, &link.href);
        self.out.push('"');
        if let Some(title) = &link.title {
            self.out.push_str(" title=\"");
            escape_attr(&mut self.out, title);
            self.out.push('"');
        }
        self.out.push('>');
    }

    fn close_link(&mut self, link: &Option<ExportLink>) {
        if link.as_ref().is_some_and(link_is_writable) {
            self.out.push_str("</a>");
        }
    }

    fn math_image(&mut self, tex: &str, asset: Option<crate::model::AssetId>, display: bool) {
        let Some(asset) = asset.and_then(|id| self.doc.asset(id)).filter(|asset| asset.mime.starts_with("image/"))
        else {
            self.out.push_str("<code class=\"math-tex\">");
            escape_text(&mut self.out, tex);
            self.out.push_str("</code>");
            return;
        };
        self.out.push_str("<img class=\"math ");
        self.out.push_str(if display { "math-display-image" } else { "math-inline" });
        self.out.push_str("\" src=\"");
        self.out.push_str(&asset.data_uri());
        self.out.push_str("\" alt=\"");
        escape_attr(&mut self.out, tex);
        self.out.push_str("\" aria-label=\"");
        let label = lmdv_i18n::render(&self.labels.math, &[("tex", &tex)]);
        escape_attr(&mut self.out, &label);
        self.out.push('"');
        self.asset_size(asset);
        if !display
            && let Some(baseline) = asset.baseline
            && baseline > 0
        {
            self.out.push_str(" style=\"vertical-align: -");
            self.out.push_str(&baseline.to_string());
            self.out.push_str("px\"");
        }
        self.out.push('>');
    }

    fn asset_size(&mut self, asset: &ExportAsset) {
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
    }

    fn inline_image(&mut self, image: &ExportImage) {
        match image.asset.and_then(|id| self.doc.asset(id)).filter(|asset| asset.mime.starts_with("image/")) {
            Some(asset) => {
                self.out.push_str("<img src=\"");
                self.out.push_str(&asset.data_uri());
                self.out.push_str("\" alt=\"");
                escape_attr(&mut self.out, &image.alt);
                self.out.push('"');
                if let Some(title) = &image.title {
                    self.out.push_str(" title=\"");
                    escape_attr(&mut self.out, title);
                    self.out.push('"');
                }
                self.out.push('>');
            }
            None => {
                let fallback = if !image.alt.is_empty() {
                    image.alt.as_str()
                } else if !image.src.is_empty() {
                    image.src.as_str()
                } else {
                    self.labels.image.as_str()
                };
                self.out.push_str("<span class=\"missing-image\">");
                escape_text(&mut self.out, fallback);
                self.out.push_str("</span>");
            }
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
        let Some(src) = node.attr("src") else {
            let alt = node.attr("alt").unwrap_or_default();
            if !alt.is_empty() {
                escape_text(&mut self.out, alt);
            }
            return;
        };
        let Some(id) = src.strip_prefix("lmdv-asset:").and_then(|id| id.parse::<u32>().ok()) else {
            let alt = node.attr("alt").unwrap_or_default();
            escape_text(&mut self.out, alt);
            return;
        };
        let Some(asset) = self.doc.asset(crate::model::AssetId(id)) else { return };
        if !asset.mime.starts_with("image/") {
            return;
        }
        self.out.push_str("<img src=\"");
        self.out.push_str(&asset.data_uri());
        self.out.push_str("\" alt=\"");
        escape_attr(&mut self.out, node.attr("alt").unwrap_or_default());
        self.out.push('"');
        if let Some(title) = node.attr("title").filter(|title| !title.is_empty()) {
            self.out.push_str(" title=\"");
            escape_attr(&mut self.out, title);
            self.out.push('"');
        }
        self.out.push('>');
    }

    fn html_media(&mut self, node: &HtmlNode) {
        let name = node.tag.name();
        self.out.push_str("<span class=\"media\">");
        if let Some(src) = node.attr("src") {
            // Only a local file URL is put in a media element; other sources
            // stay a plain link so a self-contained export never loads one.
            if src.starts_with("file:///") {
                self.out.push('<');
                self.out.push_str(name);
                self.out.push_str(" controls preload=\"none\" src=\"");
                escape_attr(&mut self.out, src);
                self.out.push_str("\"></");
                self.out.push_str(name);
                self.out.push('>');
            }
            if href_is_safe(src) {
                self.out.push_str(" <a href=\"");
                escape_attr(&mut self.out, src);
                self.out.push_str("\">");
                escape_text(&mut self.out, name);
                self.out.push_str("</a>");
            }
        } else if let Some(href) = node.attr("data-lmdv-remote") {
            if href_is_safe(href) {
                self.out.push_str("<a href=\"");
                escape_attr(&mut self.out, href);
                self.out.push_str("\">");
                escape_text(&mut self.out, name);
                self.out.push_str("</a>");
            }
        } else {
            escape_text(&mut self.out, name);
        }
        self.out.push_str("</span>");
    }

    fn html_attrs(&mut self, node: &HtmlNode, name: &str) {
        if matches!(node.tag, HtmlTag::Th | HtmlTag::Td)
            && let Some(align) = node.attr("align")
        {
            let class = match align.to_ascii_lowercase().as_str() {
                "center" => Some("align-center"),
                "right" => Some("align-right"),
                "left" => Some("align-left"),
                _ => None,
            };
            if let Some(class) = class {
                self.out.push_str(" class=\"");
                self.out.push_str(class);
                self.out.push('"');
            }
        }
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
                    if href_is_safe(value) {
                        self.out.push_str(" href=\"");
                        escape_attr(&mut self.out, value);
                        self.out.push('"');
                    }
                }
                "start" if name == "ol" => {
                    if value.chars().all(|c| c.is_ascii_digit()) && !value.is_empty() {
                        self.out.push_str(" start=\"");
                        self.out.push_str(value);
                        self.out.push('"');
                    }
                }
                "open" if name == "details" => self.out.push_str(" open"),
                "colspan" | "rowspan"
                    if matches!(node.tag, HtmlTag::Th | HtmlTag::Td)
                        && value.chars().all(|c| c.is_ascii_digit())
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
    }
}

/// Whether a link may be written verbatim.
fn link_is_writable(link: &ExportLink) -> bool {
    !link.href.is_empty() && href_is_safe(&link.href)
}

/// Whether an already-resolved href is safe to write.
fn href_is_safe(href: &str) -> bool {
    if href.chars().any(|character| character.is_control()) {
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

/// Maps a callout kind to its CSS class suffix.
fn callout_class(kind: CalloutKind) -> &'static str {
    match kind {
        CalloutKind::Note => "note",
        CalloutKind::Tip => "tip",
        CalloutKind::Important => "important",
        CalloutKind::Warning => "warning",
        CalloutKind::Caution => "caution",
    }
}

/// Maps a highlighter capture name (`keyword.control`) to a CSS class.
fn token_class(name: &str) -> &str {
    let category = name.split('.').next().unwrap_or("token");
    match category {
        "keyword" => "tok-keyword",
        "string" => "tok-string",
        "comment" => "tok-comment",
        "number" => "tok-number",
        "constant" => "tok-constant",
        "function" => "tok-function",
        "type" => "tok-type",
        "variable" => "tok-variable",
        "property" => "tok-property",
        "attribute" => "tok-attribute",
        "operator" => "tok-operator",
        "punctuation" => "tok-punctuation",
        "tag" => "tok-tag",
        "meta" => "tok-meta",
        "label" => "tok-label",
        "escape" => "tok-escape",
        "invalid" => "tok-invalid",
        "namespace" => "tok-namespace",
        "entity" => "tok-entity",
        _ => "tok-token",
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

/// The export stylesheet: light, print-safe, no external resources.
const STYLE: &str = r#"
:root {
  color-scheme: light;
  --text: #1f2328;
  --muted: #59636e;
  --border: #d1d9e0;
  --surface: #f6f8fa;
  --surface-2: #eff1f3;
  --accent: #0969da;
  --mark: #fff8c5;
  --code-bg: #f6f8fa;
  --mono: ui-monospace, "Cascadia Code", "Source Code Pro", Menlo, Consolas, monospace;
  --sans: system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
}
* { box-sizing: border-box; }
html { font-size: 16px; }
body {
  margin: 0;
  background: #fff;
  color: var(--text);
  font-family: var(--sans);
  font-size: 1rem;
  line-height: 1.7;
  -webkit-font-smoothing: antialiased;
}
article.markdown-body {
  max-width: 760px;
  margin: 0 auto;
  padding: 48px 32px 96px;
}
h1, h2, h3, h4, h5, h6 {
  margin: 1.4em 0 0.6em;
  font-weight: 600;
  line-height: 1.3;
}
h1 { font-size: 2em; padding-bottom: 0.3em; border-bottom: 1px solid var(--border); }
h2 { font-size: 1.5em; padding-bottom: 0.3em; border-bottom: 1px solid var(--border); }
h3 { font-size: 1.25em; }
h4 { font-size: 1em; }
h5 { font-size: 0.875em; }
h6 { font-size: 0.85em; color: var(--muted); }
p { margin: 0 0 1em; }
a { color: var(--accent); text-decoration: underline; text-underline-offset: 0.15em; }
a:hover { text-decoration-thickness: 2px; }
strong { font-weight: 600; }
mark { background: var(--mark); padding: 0.05em 0.15em; border-radius: 2px; }
sub, sup { font-size: 0.75em; line-height: 0; }
code, kbd, samp {
  font-family: var(--mono);
  font-size: 0.875em;
  background: var(--surface-2);
  padding: 0.15em 0.35em;
  border-radius: 4px;
}
kbd { border: 1px solid var(--border); box-shadow: 0 1px 0 var(--border); }
pre {
  margin: 0 0 1em;
  padding: 16px;
  overflow-x: auto;
  background: var(--code-bg);
  border: 1px solid var(--border);
  border-radius: 6px;
  line-height: 1.45;
}
pre code {
  padding: 0;
  background: none;
  border: 0;
  font-size: 0.875em;
  white-space: pre;
}
ul, ol { margin: 0 0 1em; padding-left: 2em; }
li + li { margin-top: 0.2em; }
ul ul, ul ol, ol ul, ol ol { margin-bottom: 0; }
.contains-task-list, .task-list { list-style: none; padding-left: 0; }
.task-list-item { list-style: none; }
.task-checkbox { margin: 0 0.5em 0 0; vertical-align: middle; }
blockquote {
  margin: 0 0 1em;
  padding: 0 1em;
  color: var(--muted);
  border-left: 0.25em solid var(--border);
}
blockquote > :last-child { margin-bottom: 0; }
hr { height: 0; margin: 1.5em 0; border: 0; border-top: 1px solid var(--border); }
.table-wrap { width: 100%; overflow-x: auto; margin: 0 0 1em; }
table { border-collapse: collapse; border-spacing: 0; width: 100%; }
th, td { padding: 6px 13px; border: 1px solid var(--border); vertical-align: top; }
th { font-weight: 600; background: var(--surface); }
tbody tr:nth-child(2n) { background: #fbfcfd; }
.align-left { text-align: left; }
.align-center { text-align: center; }
.align-right { text-align: right; }
.callout {
  margin: 0 0 1em;
  padding: 0.75em 1em;
  border-left: 0.25em solid var(--border);
  border-radius: 6px;
  background: var(--surface);
}
.callout > :last-child { margin-bottom: 0; }
.callout-title { margin: 0 0 0.4em; font-weight: 600; }
.callout-note { border-color: #0969da; background: #ddf4ff; }
.callout-note .callout-title { color: #0969da; }
.callout-tip { border-color: #1a7f37; background: #dafbe1; }
.callout-tip .callout-title { color: #1a7f37; }
.callout-important { border-color: #8250df; background: #fbefff; }
.callout-important .callout-title { color: #8250df; }
.callout-warning { border-color: #9a6700; background: #fff8c5; }
.callout-warning .callout-title { color: #9a6700; }
.callout-caution { border-color: #cf222e; background: #ffebe9; }
.callout-caution .callout-title { color: #cf222e; }
figure { margin: 1.5em 0; }
figure img { max-width: 100%; height: auto; }
figcaption { margin-top: 0.5em; color: var(--muted); font-size: 0.875em; text-align: center; }
.image { text-align: center; }
.diagram { text-align: center; }
.diagram-source { text-align: left; }
.media { display: inline-flex; gap: 0.4em; align-items: center; color: var(--muted); }
.missing-image { color: var(--muted); font-style: italic; }
.math { max-width: 100%; }
.math-display { margin: 1.25em 0; text-align: center; overflow-x: auto; }
.math-display-image { display: inline-block; max-width: 100%; height: auto; vertical-align: middle; }
.math-inline { display: inline-block; max-width: 100%; height: auto; vertical-align: baseline; }
.math-tex {
  font-family: var(--mono);
  font-size: 0.9375em;
  background: var(--surface-2);
  padding: 0.1em 0.3em;
  border-radius: 4px;
}
.toc {
  margin: 0 0 2em;
  padding: 1em 1.25em;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
}
.toc ul { margin: 0.25em 0; padding-left: 1.2em; list-style: none; }
.toc > ul { padding-left: 0; }
.toc a { text-decoration: none; }
.toc a:hover { text-decoration: underline; }
.front-matter {
  margin: 0 0 2em;
  padding: 0.5em 1em;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
}
.front-matter summary { cursor: pointer; font-weight: 600; }
.front-matter dl { display: grid; grid-template-columns: max-content 1fr; gap: 0.25em 1em; margin: 0.75em 0 0.25em; }
.front-matter dt { color: var(--muted); }
.front-matter dd { margin: 0; }
.front-matter-raw { margin: 0.75em 0 0; }
.footnotes {
  margin-top: 3em;
  padding-top: 1em;
  border-top: 1px solid var(--border);
  color: var(--muted);
  font-size: 0.875em;
}
.footnotes > hr { margin: 0 0 1em; }
.footnotes-list { padding-left: 1.5em; }
.footnote-item > p { margin-bottom: 0.5em; }
.footnote-ref a { text-decoration: none; padding: 0 0.1em; }
.footnote-backref { text-decoration: none; margin-left: 0.4em; }
.tok-keyword { color: #cf222e; }
.tok-string { color: #0a3069; }
.tok-escape { color: #0550ae; }
.tok-comment { color: #59636e; font-style: italic; }
.tok-number, .tok-constant { color: #0550ae; }
.tok-function { color: #8250df; }
.tok-type { color: #953800; }
.tok-variable, .tok-property { color: #0550ae; }
.tok-attribute { color: #0550ae; }
.tok-operator, .tok-punctuation { color: #24292f; }
.tok-tag { color: #116329; }
.tok-meta, .tok-label, .tok-namespace { color: #59636e; }
.tok-entity { color: #0550ae; }
.tok-invalid { color: #cf222e; text-decoration: underline wavy; }
.tok-token { color: inherit; }
@page { size: A4; margin: 20mm 16mm 18mm 16mm; }
@media print {
  html, body { background: #fff; }
  body { font-size: 16px; }
  article.markdown-body { max-width: none; margin: 0; padding: 0; }
  a { color: inherit; }
  pre, blockquote, figure, table, tr, img, .callout { break-inside: avoid; }
  h1, h2, h3, h4 { break-after: avoid; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; }
  .table-wrap { overflow: visible; }
  .toc { break-inside: avoid; }
  .footnotes { break-before: auto; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_document_is_still_valid_html() {
        let html = ExportDoc::default().to_html().unwrap();
        assert!(html.starts_with("<!doctype html>\n<html lang=\"en\">"));
        assert!(html.contains("<article class=\"markdown-body\">\n</article>"));
    }

    #[test]
    fn text_and_attributes_are_escaped() {
        let mut out = String::new();
        escape_text(&mut out, "<b> & \"quoted\" 'x'");
        assert_eq!(out, "&lt;b&gt; &amp; &quot;quoted&quot; &#39;x&#39;");
    }

    #[test]
    fn token_categories_map_to_classes() {
        assert_eq!(token_class("keyword.control"), "tok-keyword");
        assert_eq!(token_class("string.escape"), "tok-string");
        assert_eq!(token_class("function.method"), "tok-function");
        assert_eq!(token_class("odd.thing"), "tok-token");
    }
}
