//! Inline content: styled runs, links, code, math placeholders, images and the
//! inline objects `lmdv-md` records.
//!
//! The parser guarantees that [`lmdv_md::Inline::text`] is what is drawn:
//! soft breaks are spaces, hard breaks are `\n`, and every formula, inline
//! image and task checkbox is one U+FFFC (three bytes) with a side-table entry.
//! Drawing therefore has two jobs: give each placeholder something to draw (a
//! [`rune::Span::image`], a chip, or the escaped source) and keep every range in
//! the side tables aligned with the string that is finally laid out.
//!
//! Replacement text (a failed formula's TeX, an image's alt) shifts byte
//! offsets, so [`Built`] remaps spans, strikes and links through the same
//! replacement list in one pass.

use std::ops::Range;

use lmdv_md::{HtmlNode, HtmlTag, Inline, InlineObjectKind, InlineStyle};
use rune::text::InlineImage;
use rune::{Color, El, Link, Span, TextHighlight, TextStyle, rich};

use crate::block::Renderer;
use crate::math::fit;
use crate::{HighlightKind, highlight_key};

/// A hard line break as the parser writes it.
const HARD_BREAK: char = '\n';

pub(crate) struct InlineOptions<'a> {
    pub style: TextStyle,
    pub color: Color,
    pub key: Option<&'a str>,
    /// Drop a leading task checkbox (the list renderer draws it in the marker
    /// column instead, as the original does).
    pub strip_tasks: bool,
    /// `copy_breaks` around the drawn text.
    pub breaks: (u8, u8),
    /// A list marker copied before the first line (M-26: copy as drawn).
    pub copy_prefix: Option<String>,
}

impl<'a> InlineOptions<'a> {
    pub(crate) fn new(style: TextStyle, color: Color) -> Self {
        Self { style, color, key: None, strip_tasks: false, breaks: (2, 2), copy_prefix: None }
    }

    pub(crate) fn key(mut self, key: &'a str) -> Self {
        self.key = Some(key);
        self
    }

    pub(crate) fn breaks(mut self, before: u8, after: u8) -> Self {
        self.breaks = (before, after);
        self
    }

    pub(crate) fn strip_tasks(mut self, strip: bool) -> Self {
        self.strip_tasks = strip;
        self
    }

    pub(crate) fn copy_prefix(mut self, prefix: String) -> Self {
        self.copy_prefix = Some(prefix);
        self
    }
}

/// One replacement in the drawn text.
struct Rep {
    start: usize,
    end: usize,
    text: String,
    /// Where the U+FFFC sits inside `text`, for an inline image.
    placeholder: Option<usize>,
    image: Option<InlineImage>,
    /// The source the replacement came from, delimiters and Markdown syntax
    /// included, so a highlight over any of it paints what is drawn (OI-127).
    source: Option<Range<usize>>,
    /// The replacement bytes are a code/mono chip.
    chip: bool,
    /// The replacement bytes are secondary text.
    muted: bool,
    /// Bytes of `text` that copying skips.
    uncopied: Vec<Range<usize>>,
    /// A link over the whole replacement.
    link: Option<String>,
    /// The picture object's place in `Inline::objects`, for the preview link.
    picture: Option<usize>,
    /// Filled while composing: the replacement's position in the drawn text.
    new_start: usize,
    new_end: usize,
}

/// The inline content resolved to text plus Rune spans, in drawn coordinates.
struct Built {
    text: String,
    spans: Vec<Span>,
    strikes: Vec<Range<usize>>,
    links: Vec<(Range<usize>, String, Option<String>)>,
    /// Pressable inline pictures: drawn bytes, `image:{block}:{object}`, alt.
    picture_links: Vec<(Range<usize>, String, String)>,
    uncopied: Vec<Range<usize>>,
    /// Paint-only highlights in drawn bytes: the range, its kind and the index
    /// of the caller's [`crate::Highlight`] it came from.
    highlights: Vec<(Range<usize>, HighlightKind, usize)>,
}

impl<'a> Renderer<'a> {
    /// The text element for `inline`.
    pub(crate) fn inline(&self, inline: &Inline, opts: InlineOptions<'_>) -> El {
        let built = self.build_inline(inline, opts.strip_tasks);
        let identity = self.note_text(opts.key, &built.text);
        let mut e = rich(built.text, built.spans).font(opts.style).color(opts.color).exact_lines().w_full();
        for range in built.strikes {
            e = e.strike(range.start, range.end, None);
        }
        for range in built.uncopied {
            e = e.uncopied(range);
        }
        let mut has_links = false;
        for (range, dest, title) in built.links {
            has_links = true;
            let mut link = Link::new(range, dest);
            if let Some(title) = title.filter(|t| !t.is_empty()) {
                link = link.help(title);
            }
            e = e.link(link);
        }
        // V-01 for a text-span picture: the picture itself is the preview button.
        for (range, dest, label) in built.picture_links {
            has_links = true;
            e = e.link(Link::new(range, dest).role("button").label(label));
        }
        if !built.highlights.is_empty() {
            e = self.painted(e, built.highlights);
        }
        if self.opts.selectable {
            e = e.selectable(true).selection_color(self.theme.selection);
        }
        e = e.copy_breaks(opts.breaks.0, opts.breaks.1);
        if let Some(prefix) = opts.copy_prefix {
            e = e.copy_prefix(prefix);
        }
        if let Some(key) = identity.as_deref().or(opts.key) {
            e = e.key(key);
        }
        if has_links && let Some(wire) = &self.opts.wires.link {
            e = wire(e);
        }
        e
    }

    /// Resolves placeholders and remaps every styled range.
    fn build_inline(&self, inline: &Inline, strip_tasks: bool) -> Built {
        let mut reps: Vec<Rep> = Vec::new();
        for math in &inline.math {
            let max_width = (self.opts.width - 8.0).max(16.0);
            let image = if math.display {
                self.opts.math.display(&math.tex, self.theme.body.size)
            } else {
                self.opts.math.inline(&math.tex, self.theme.body.size)
            };
            let image = image.map(|image| {
                let drawn = if math.display {
                    fit(image, max_width, max_width.max(48.0))
                } else {
                    fit(image, max_width, self.theme.inline_image_max_height)
                };
                // OI-037: a reader hears the TeX, never the object marker (patch 0003).
                drawn.label(lmdv_i18n::render(&self.theme.labels.math, &[("tex", &math.tex)]))
            });
            let mut rep = Rep {
                start: math.range.start,
                end: math.range.end,
                text: String::new(),
                placeholder: None,
                image,
                source: self.formula_source(inline, math.range.start, math.range.end, &math.tex),
                chip: false,
                muted: false,
                uncopied: Vec::new(),
                link: None,
                picture: None,
                new_start: 0,
                new_end: 0,
            };
            match (&rep.image, math.display) {
                (Some(_), false) => {
                    rep.text.push(lmdv_md::OBJECT_CHAR);
                    rep.placeholder = Some(0);
                    rep.uncopied.push(0..3);
                }
                (Some(_), true) => {
                    // M-17: an inline display formula takes a line of its own
                    // without splitting the paragraph.
                    rep.text.push(HARD_BREAK);
                    rep.text.push(lmdv_md::OBJECT_CHAR);
                    rep.text.push(HARD_BREAK);
                    rep.placeholder = Some(1);
                    rep.uncopied.push(0..1);
                    rep.uncopied.push(4..5);
                }
                (None, display) => {
                    if display {
                        rep.text.push(HARD_BREAK);
                        rep.uncopied.push(0..1);
                    }
                    rep.text.push_str(&math.tex);
                    if display {
                        let at = rep.text.len();
                        rep.text.push(HARD_BREAK);
                        rep.uncopied.push(at..at + 1);
                    }
                    rep.chip = true;
                }
            }
            reps.push(rep);
        }
        for (object_index, object) in inline.objects.iter().enumerate() {
            if matches!(object.kind, InlineObjectKind::LineBreak) {
                continue; // text already holds its newline.
            }
            let mut rep = Rep {
                start: object.range.start,
                end: object.range.end.max(object.range.start),
                text: String::new(),
                placeholder: None,
                image: None,
                source: self.object_source(inline, object),
                chip: false,
                muted: false,
                uncopied: Vec::new(),
                link: None,
                picture: Some(object_index),
                new_start: 0,
                new_end: 0,
            };
            match &object.kind {
                InlineObjectKind::TaskCheckbox { checked } => {
                    if !strip_tasks {
                        rep.text.push(if *checked { '\u{2611}' } else { '\u{2610}' });
                    }
                }
                InlineObjectKind::Image => {
                    let node = object.html.as_ref();
                    let src = node.and_then(|n| self.node_src(n)).unwrap_or_default();
                    let alt = node.and_then(|n| n.attr("alt")).unwrap_or_default();
                    let title = node.and_then(|n| n.attr("title")).unwrap_or_default();
                    if !self.image_visible(src) {
                        rep.text.clear();
                    } else if let Some(image) = self.opts.images.resolve(src, alt, title) {
                        let [width, height] = image.size().unwrap_or([16, 9]);
                        let ratio = width as f32 / height.max(1) as f32;
                        let mut draw_h = self.theme.inline_image_max_height.max(self.theme.body.size * 1.2);
                        let mut draw_w = draw_h * ratio;
                        let max_width = (self.opts.width - 8.0).max(16.0);
                        if draw_w > max_width {
                            draw_w = max_width;
                            draw_h = draw_w / ratio.max(0.001);
                        }
                        rep.text.push(lmdv_md::OBJECT_CHAR);
                        rep.placeholder = Some(0);
                        rep.image = Some(InlineImage::new(image.pixels().clone(), draw_w, draw_h).label(alt));
                        rep.uncopied.push(0..3);
                    } else {
                        // M-25: with no alt text the placeholder shows the
                        // source address, exactly as a block image does.
                        let fallback = if alt.is_empty() { src } else { alt };
                        rep.text.push_str(fallback);
                        rep.muted = true;
                    }
                }
                InlineObjectKind::Picture => {
                    if let Some(node) = object.html.as_ref().and_then(|n| self.best_picture_candidate(n)) {
                        let src = self.node_src(node).unwrap_or_default();
                        let alt = node.attr("alt").unwrap_or_default();
                        let title = node.attr("title").unwrap_or_default();
                        if !self.image_visible(src) {
                            rep.text.clear();
                        } else if let Some(image) = self.opts.images.resolve(src, alt, title) {
                            let [width, height] = image.size().unwrap_or([16, 9]);
                            let ratio = width as f32 / height.max(1) as f32;
                            let draw_h = self.theme.inline_image_max_height.max(self.theme.body.size * 1.2);
                            let draw_w = (draw_h * ratio).min((self.opts.width - 8.0).max(16.0));
                            rep.text.push(lmdv_md::OBJECT_CHAR);
                            rep.placeholder = Some(0);
                            rep.image = Some(InlineImage::new(image.pixels().clone(), draw_w, draw_h).label(alt));
                            rep.uncopied.push(0..3);
                        } else {
                            // M-25: no alt falls back to the chosen source.
                            let fallback = if alt.is_empty() { src } else { alt };
                            rep.text.push_str(fallback);
                            rep.muted = true;
                        }
                    } else {
                        rep.text.push_str(&self.theme.labels.image);
                        rep.muted = true;
                    }
                }
                InlineObjectKind::Video | InlineObjectKind::Audio => {
                    let node = object.html.as_ref();
                    let src = node.and_then(|n| self.node_src(n)).unwrap_or_default();
                    let tag = if matches!(object.kind, InlineObjectKind::Video) { "video" } else { "audio" };
                    if src.is_empty() {
                        rep.text.push_str(tag);
                    } else {
                        rep.text.push_str(&format!("{tag}: {src}"));
                        rep.link = Some(crate::media_link(src));
                    }
                    rep.muted = true;
                }
                InlineObjectKind::Details | InlineObjectKind::Unknown => {
                    rep.text.push_str(&object.html.as_ref().map(|n| n.text()).unwrap_or_default());
                }
                InlineObjectKind::LineBreak => {}
            }
            if rep.start == rep.end && rep.text.is_empty() {
                continue;
            }
            reps.push(rep);
        }
        reps.sort_by_key(|rep| rep.start);
        reps.retain(|rep| rep.start <= inline.text.len() && rep.end <= inline.text.len());

        // Compose the drawn text and remember where each replacement landed.
        let mut text = String::with_capacity(inline.text.len());
        let mut cursor = 0usize;
        for rep in &mut reps {
            if rep.start < cursor {
                // The parser reports an inline `<picture>` as both its `<img>`
                // and an empty picture object; draw the first one only.
                continue;
            }
            if rep.start > cursor {
                text.push_str(&inline.text[cursor..rep.start]);
            }
            rep.new_start = text.len();
            text.push_str(&rep.text);
            rep.new_end = text.len();
            cursor = rep.end.max(cursor);
        }
        if cursor < inline.text.len() {
            text.push_str(&inline.text[cursor..]);
        }

        let mut spans = Vec::new();
        let mut strikes = Vec::new();
        for span in &inline.spans {
            if matches!(span.style, InlineStyle::Plain | InlineStyle::Link) {
                continue;
            }
            let start = drawn_byte(&reps, span.range.start, false);
            let end = drawn_byte(&reps, span.range.end, true);
            if start >= end {
                continue;
            }
            match span.style {
                InlineStyle::Strike => strikes.push(start..end),
                InlineStyle::Strong => spans.push(Span::new(start, end).weight(600.0)),
                InlineStyle::Em => spans.push(Span::new(start, end).italic()),
                InlineStyle::Code => spans
                    .push(Span::new(start, end).mono().size(self.theme.body.size * 0.9).bg(self.theme.code_bg, 2.0)),
                InlineStyle::Mark => spans.push(Span::new(start, end).bg(self.theme.mark, 2.0)),
                InlineStyle::Sub => spans
                    .push(Span::new(start, end).size(self.theme.body.size * 0.72).rise(-self.theme.body.size * 0.18)),
                InlineStyle::Sup => spans
                    .push(Span::new(start, end).size(self.theme.body.size * 0.72).rise(self.theme.body.size * 0.35)),
                InlineStyle::Kbd => spans
                    .push(Span::new(start, end).mono().size(self.theme.body.size * 0.85).bg(self.theme.code_bg, 1.5)),
                InlineStyle::Underline => spans.push(Span::new(start, end).underline(self.theme.text)),
                InlineStyle::Plain | InlineStyle::Link => {}
            }
        }

        let mut links = Vec::with_capacity(inline.links.len());
        for link in &inline.links {
            let start = drawn_byte(&reps, link.range.start, false);
            let end = drawn_byte(&reps, link.range.end, true);
            if start >= end {
                continue;
            }
            let in_code = inline.spans.iter().any(|span| {
                span.style == InlineStyle::Code
                    && span.range.start <= link.range.start
                    && link.range.end <= span.range.end
            });
            let mut paint = Span::new(start, end).color(self.theme.accent);
            if !in_code {
                paint = paint.underline(self.theme.link_underline);
            }
            spans.push(paint);
            links.push((start..end, link.dest.clone(), link.title.clone()));
        }

        let mut uncopied = Vec::new();
        let mut picture_links: Vec<(Range<usize>, String, String)> = Vec::new();
        let pictures_pressable = self.opts.wires.link.is_some();
        let block_index = self.nested_block();
        let block_key = self.doc.block_key(block_index).map(str::to_owned).unwrap_or_else(|| format!("b{block_index}"));
        for rep in &reps {
            if let Some(offset) = rep.placeholder {
                let start = rep.new_start + offset;
                if let Some(image) = &rep.image {
                    spans.push(Span::new(start, start + 3).image(image.clone()));
                    if pictures_pressable && let Some(object) = rep.picture {
                        picture_links.push((
                            start..start + 3,
                            format!("image:{block_key}:{object}"),
                            image.label.as_deref().unwrap_or_default().to_owned(),
                        ));
                    }
                } else {
                    uncopied.push(start..start + 3);
                }
            } else if !rep.text.is_empty() {
                let start = rep.new_start;
                let end = rep.new_end;
                if rep.chip {
                    spans.push(
                        Span::new(start, end).mono().size(self.theme.body.size * 0.9).bg(self.theme.code_bg, 2.0),
                    );
                } else if rep.muted {
                    spans.push(Span::new(start, end).color(self.theme.faint));
                }
                if let Some(dest) = &rep.link {
                    spans.push(Span::new(start, end).color(self.theme.accent).underline(self.theme.link_underline));
                    links.push((start..end, dest.clone(), None));
                }
            }
            for range in &rep.uncopied {
                uncopied.push(rep.new_start + range.start..rep.new_start + range.end);
            }
        }

        // A parser bug must not take the frame down (M-34): drop ranges that do
        // not fit the text that is actually laid out.
        let len = text.len();
        spans.retain(|span| (span.start as usize) < (span.end as usize) && (span.end as usize) <= len);
        strikes.retain(|range| range.start < range.end && range.end <= len);
        links.retain(|(range, _, _)| range.start < range.end && range.end <= len);
        picture_links.retain(|(range, _, _)| range.start < range.end && range.end <= len);
        uncopied.retain(|range| range.start <= range.end && range.end <= len);
        // Highlight work is skipped entirely unless the app asked for it.
        let highlights =
            if self.opts.highlights.is_empty() { Vec::new() } else { self.inline_highlights(inline, &reps, &text) };
        Built { text, spans, strikes, links, picture_links, uncopied, highlights }
    }

    /// The source span a formula replaced: its delimiters and its TeX.
    ///
    /// The parser's map ties the placeholder to the opening delimiter and leaves
    /// the TeX itself unmapped, so a highlight over the formula's body would
    /// paint nothing (OI-127). The closing delimiter is checked before the span
    /// is trusted; otherwise the gap up to the next mapped run stands in.
    fn formula_source(&self, inline: &Inline, rendered: usize, rendered_end: usize, tex: &str) -> Option<Range<usize>> {
        let start = inline.source_map.source_of(rendered)?;
        let rest = self.doc.source.get(start..)?;
        let delimiter = if rest.starts_with("$$") {
            2
        } else if rest.starts_with('$') {
            1
        } else {
            return None;
        };
        let end = start.checked_add(delimiter * 2 + tex.len())?;
        let slice = self.doc.source.get(start..end)?;
        let closing = if delimiter == 2 { "$$" } else { "$" };
        if slice.ends_with(closing) && slice.len() == delimiter * 2 + tex.len() {
            return Some(start..end);
        }
        // A formula whose TeX the scanner trimmed still spans from its opening
        // delimiter to the mapped run after it; that gap is the whole formula.
        let end = source_after(&inline.source_map, rendered_end).unwrap_or(start);
        (start < end).then_some(start..end)
    }

    /// The source span an inline object replaced, its Markdown/HTML syntax included.
    fn object_source(&self, inline: &Inline, object: &lmdv_md::InlineObject) -> Option<Range<usize>> {
        use lmdv_md::InlineObjectKind;
        let start = inline.source_map.source_of(object.range.start)?;
        match object.kind {
            InlineObjectKind::Image => {
                let rest = self.doc.source.get(start..)?;
                if rest.starts_with("![") {
                    markdown_image_end(rest).map(|end| start..start + end)
                } else if rest.starts_with('<') {
                    rest.find('>').map(|end| start..start + end + 1)
                } else {
                    let end = source_after(&inline.source_map, object.range.end).unwrap_or(start);
                    (start < end).then_some(start..end)
                }
            }
            InlineObjectKind::TaskCheckbox { .. } => Some(start..start + 3),
            _ => {
                let end = source_after(&inline.source_map, object.range.end).unwrap_or(start);
                (start < end).then_some(start..end)
            }
        }
    }

    /// Paints `parts` as paint-only [`rune::TextHighlight`]s over a text element.
    ///
    /// Highlights never reshape or split the text, so selection, copy and line
    /// breaks stay exactly as they were without them. Each highlight is keyed by
    /// the caller's index, which makes the current match scrollable.
    pub(crate) fn painted(&self, e: El, parts: Vec<(Range<usize>, HighlightKind, usize)>) -> El {
        e.text_highlights(parts.into_iter().map(|(range, kind, index)| {
            TextHighlight::new(range, self.highlight_color(kind)).key(highlight_key(index)).pad(2.0)
        }))
    }

    /// Maps the app's source ranges through this inline's source map and the
    /// drawn replacement list, then resolves overlaps to one paint each.
    ///
    /// Besides the source map's monotone segments, a range over an inline
    /// object's own syntax (a formula's TeX, an image's alt text or URL, a task
    /// marker) paints the object's drawn replacement, and a range over a link's
    /// destination paints the link's label: those bytes draw nothing themselves
    /// (OI-127).
    fn inline_highlights(
        &self,
        inline: &Inline,
        reps: &[Rep],
        text: &str,
    ) -> Vec<(Range<usize>, HighlightKind, usize)> {
        let mut parts = Vec::new();
        for (index, highlight) in self.opts.highlights.iter().enumerate() {
            if highlight.range.is_empty() {
                continue;
            }
            let mut pieces: Vec<Range<usize>> = Vec::new();
            if let Some(rendered) = rendered_range(&inline.source_map, highlight.range.clone())
                && let Some(drawn) = drawn_range(reps, rendered)
            {
                pieces.push(trim_breaks(text, drawn));
            }
            for rep in reps {
                if let Some(source) = &rep.source
                    && overlaps(&highlight.range, source)
                {
                    pieces.push(trim_breaks(text, rep.new_start..rep.new_end));
                }
            }
            for link in &inline.links {
                let Some(label) = inline.source_map.source_range(link.range.clone()) else { continue };
                let Some(after) = source_after(&inline.source_map, link.range.end) else { continue };
                if label.end < after && overlaps(&highlight.range, &(label.end..after)) {
                    pieces.push(trim_breaks(text, link.range.clone()));
                }
            }
            parts.extend(
                pieces
                    .into_iter()
                    .filter(|piece| piece.start < piece.end && piece.end <= text.len())
                    .map(|piece| (piece, highlight.kind, index)),
            );
        }
        overlay(parts, text.len())
    }

    /// The `<source>`/`<img>` of a `<picture>` that matches the appearance: a
    /// `media` query mentioning `dark`/`light` picks its scheme (M-29),
    /// `(prefers-color-scheme: …)` picks the scheme, anything else is taken.
    pub(crate) fn best_picture_candidate<'n>(&self, node: &'n HtmlNode) -> Option<&'n HtmlNode> {
        let mut fallback = None;
        for child in &node.children {
            let lmdv_md::HtmlChild::Node(child) = child else { continue };
            match child.tag {
                HtmlTag::Source => {
                    if self.source_matches(child.attr("media")) {
                        return Some(child);
                    }
                }
                HtmlTag::Img => fallback = Some(child),
                _ => {}
            }
        }
        fallback
    }

    fn source_matches(&self, media: Option<&str>) -> bool {
        let Some(media) = media else { return true };
        let media = media.to_ascii_lowercase();
        if media.contains("dark") {
            return self.theme.dark;
        }
        if media.contains("light") {
            return !self.theme.dark;
        }
        true
    }

    /// An element's source, choosing the `srcset` candidate for the display
    /// scale: `src`, or the chosen candidate.
    fn node_src<'n>(&self, node: &'n HtmlNode) -> Option<&'n str> {
        node.attr("src")
            .filter(|src| !src.is_empty())
            .or_else(|| crate::block::best_candidate(&node.srcset, self.opts.scale).map(|c| c.url.as_str()))
    }

    /// The accessible label of a list item: its own text, sub-lists excluded,
    /// trimmed and capped at 120 characters (M-07).
    pub(crate) fn item_label(&self, blocks: &[lmdv_md::Block]) -> String {
        let mut text = String::new();
        for block in blocks {
            match block {
                lmdv_md::Block::Para(inline) | lmdv_md::Block::Heading { inline, .. } => {
                    text.extend(inline.text.chars().filter(|c| *c != lmdv_md::OBJECT_CHAR));
                    text.push(' ');
                }
                lmdv_md::Block::Code { text: code, .. } => {
                    text.push_str(code);
                    text.push(' ');
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for word in text.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
        if out.chars().count() > 120 {
            let cut: String = out.chars().take(119).collect();
            format!("{}…", cut.trim_end())
        } else {
            out
        }
    }
}

/// The drawn byte for `byte` of [`Inline::text`], where a replacement (a
/// formula, an image, a failed formula's TeX) maps to its whole drawn span.
///
/// An exclusive range end that lands exactly on a replacement's first byte
/// stops before it: a highlight of the prose before a formula must not paint
/// the formula's object glyph as well.
fn drawn_byte(reps: &[Rep], byte: usize, at_end: bool) -> usize {
    let mut delta = 0isize;
    for rep in reps {
        if byte < rep.start {
            break;
        }
        if byte == rep.start {
            return rep.new_start;
        }
        if byte >= rep.end {
            delta = rep.new_end as isize - rep.end as isize;
            continue;
        }
        return if at_end { rep.new_end } else { rep.new_start };
    }
    (byte as isize + delta).max(0) as usize
}

/// The drawn range for a rendered range of [`Inline::text`].
fn drawn_range(reps: &[Rep], rendered: Range<usize>) -> Option<Range<usize>> {
    let start = drawn_byte(reps, rendered.start, false);
    let end = drawn_byte(reps, rendered.end, true);
    (start < end).then_some(start..end)
}

/// The rendered range that came from `source`, from the inline source map's
/// monotone segments.
///
/// A Markdown marker between two mapped runs is not in the map, so a source
/// range that covers it simply spans the runs on either side. A range over only
/// unmapped source bytes (a marker, the formula's TeX after its placeholder)
/// maps to nothing and is ignored.
pub(crate) fn rendered_range(map: &lmdv_md::SourceMap, source: Range<usize>) -> Option<Range<usize>> {
    let mut start = usize::MAX;
    let mut end = 0;
    for segment in &map.segments {
        let length = segment.rendered.len();
        let from = source.start.max(segment.source);
        let to = source.end.min(segment.source + length);
        if from >= to {
            continue;
        }
        start = start.min(segment.rendered.start + (from - segment.source));
        end = end.max(segment.rendered.start + (to - segment.source));
    }
    (start < end).then_some(start..end)
}

/// Removes hard line breaks from the edges of a painted range: they take no
/// width, and a display formula replaces them with its own line.
fn trim_breaks(text: &str, mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end && text.as_bytes()[range.start] == b'\n' {
        range.start += 1;
    }
    while range.start < range.end && text.as_bytes()[range.end - 1] == b'\n' {
        range.end -= 1;
    }
    range
}

/// Whether two source ranges share a byte.
fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// The source byte after drawn byte `rendered`: the next mapped run's first
/// source byte, or the inline's mapped end.
///
/// A Markdown link's destination and an object's syntax sit between two mapped
/// runs and are not in the map themselves; this is the end of that gap.
fn source_after(map: &lmdv_md::SourceMap, rendered: usize) -> Option<usize> {
    if let Some(source) = map.source_of(rendered) {
        return Some(source);
    }
    if let Some(next) = map.segments.iter().find(|segment| segment.rendered.start >= rendered) {
        return Some(next.source);
    }
    map.segments.last().map(|segment| segment.source + segment.rendered.len())
}

/// The byte length of a Markdown image starting at `source`, its `!` included.
///
/// The parser reports the whole `![alt](destination "title")` span, not just the
/// first character the source map knows, so a highlight over any part of it can
/// paint the picture (OI-127). Nested brackets in the alt and parentheses in the
/// destination are balanced; a backslash escapes the next byte.
fn markdown_image_end(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    if !source.starts_with("![") {
        return None;
    }
    let mut depth = 0usize;
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b'\\' => index += 1,
            _ => {}
        }
        index += 1;
    }
    if depth != 0 || *bytes.get(index)? != b']' || *bytes.get(index + 1)? != b'(' {
        return None;
    }
    let mut depth = 1usize;
    let mut at = index + 2;
    while at < bytes.len() {
        match bytes[at] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            b'\\' => at += 1,
            _ => {}
        }
        at += 1;
    }
    None
}

/// Splits overlapping paints into non-overlapping segments, each painted with
/// the strongest kind that covers it; touching pieces of one highlight merge so
/// a match broken by a style run is one rounded paint, while another match keeps
/// its own [`crate::highlight_key`].
pub(crate) fn overlay(
    mut parts: Vec<(Range<usize>, HighlightKind, usize)>,
    len: usize,
) -> Vec<(Range<usize>, HighlightKind, usize)> {
    parts.retain(|(range, _, _)| range.start < range.end && range.end <= len);
    if parts.is_empty() {
        return parts;
    }
    let mut edges: Vec<usize> = parts.iter().flat_map(|(range, _, _)| [range.start, range.end]).collect();
    edges.sort_unstable();
    edges.dedup();
    let mut out: Vec<(Range<usize>, HighlightKind, usize)> = Vec::new();
    for pair in edges.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let mut strongest: Option<(HighlightKind, usize)> = None;
        for (range, kind, index) in &parts {
            if range.start > start || range.end < end {
                continue;
            }
            let better = strongest.is_none_or(|(best, best_index)| {
                kind.priority() > best.priority() || (kind.priority() == best.priority() && *index < best_index)
            });
            if better {
                strongest = Some((*kind, *index));
            }
        }
        let Some((kind, index)) = strongest else { continue };
        if let Some(last) = out.last_mut()
            && last.0.end == start
            && last.1 == kind
            && last.2 == index
        {
            last.0.end = end;
        } else {
            out.push((start..end, kind, index));
        }
    }
    out
}
