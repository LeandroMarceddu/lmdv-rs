//! Inline events to [`Inline`]: styles, links, formulas, objects, autolinks,
//! footnote references, inline HTML and the rendered-to-source map.

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{Event, Tag, TagEnd};

use crate::html;
use crate::model::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, HtmlNode, HtmlTag, Inline, InlineLink, InlineMath, InlineObject,
    InlineObjectKind, InlineSpan, InlineStyle, MathSpan, OBJECT_CHAR, SourceMap,
};
use crate::text::line_of;

/// State shared by every inline builder in one document parse.
pub(crate) struct InlineContext<'a> {
    /// The transformed source the events borrow from.
    pub source: &'a str,
    /// Line starts of the original source.
    pub starts: &'a [u32],
    /// The formulas found by the scanner.
    pub math: &'a [MathSpan],
    /// Placeholder offset to index in [`InlineContext::math`].
    pub math_by_start: &'a HashMap<usize, usize>,
    /// Every footnote label that has a definition.
    pub definitions: &'a std::collections::HashSet<String>,
    /// Assigned footnote numbers, by first reference.
    pub footnote_numbers: HashMap<String, usize>,
    /// The first reference's source range, per footnote label.
    pub first_refs: HashMap<String, Range<usize>>,
    /// The next footnote number.
    pub next_footnote: usize,
    /// Problems found while building inline content.
    pub diagnostics: Vec<Diagnostic>,
}

impl InlineContext<'_> {
    fn number_for(&mut self, label: &str) -> usize {
        if let Some(number) = self.footnote_numbers.get(label) {
            return *number;
        }
        let number = self.next_footnote;
        self.next_footnote += 1;
        self.footnote_numbers.insert(label.to_owned(), number);
        number
    }
}

/// Inline content plus the text markdown-it-anchor would slug.
pub(crate) struct BuiltInline {
    /// The inline content.
    pub inline: Inline,
    /// Heading text: text and inline code only, never math or image alt.
    pub slug_text: String,
}

/// Build the inline content of one paragraph, heading or table cell.
pub(crate) fn build<'a>(events: &[(Event<'a>, Range<usize>)], ctx: &mut InlineContext<'_>) -> BuiltInline {
    // The inline's own source end lets `source_range` clamp an endpoint that
    // typographic quote expansion pushes past it (OI-145).
    let source_end = events.iter().map(|(_, range)| range.end).max().unwrap_or(0);
    let mut builder = Builder {
        text: String::new(),
        spans: Vec::new(),
        links: Vec::new(),
        math: Vec::new(),
        objects: Vec::new(),
        source_map: SourceMap::new(),
        slug_text: String::new(),
        styles: Vec::new(),
        html_styles: Vec::new(),
        links_open: Vec::new(),
        html_links: Vec::new(),
        html_objects: Vec::new(),
        drop_depth: 0,
        image: None,
        suppressed_until: None,
        ctx,
    };
    builder.source_map.set_source_end(source_end);
    for (event, range) in events {
        builder.event(event, range);
    }
    builder.finish()
}

struct StyleFrame {
    style: InlineStyle,
    start: usize,
}

struct HtmlStyleFrame {
    name: String,
    style: Option<InlineStyle>,
    start: usize,
}

struct LinkFrame {
    dest: String,
    title: Option<String>,
    start: usize,
}

struct ImageFrame {
    src: String,
    title: Option<String>,
    alt: String,
    source: usize,
}

struct HtmlObjectFrame {
    name: String,
    kind: InlineObjectKind,
    node: HtmlNode,
    start: usize,
}

struct Builder<'a, 'b> {
    text: String,
    spans: Vec<InlineSpan>,
    links: Vec<InlineLink>,
    math: Vec<InlineMath>,
    objects: Vec<InlineObject>,
    source_map: SourceMap,
    slug_text: String,
    styles: Vec<StyleFrame>,
    html_styles: Vec<HtmlStyleFrame>,
    links_open: Vec<LinkFrame>,
    html_links: Vec<LinkFrame>,
    html_objects: Vec<HtmlObjectFrame>,
    drop_depth: usize,
    image: Option<ImageFrame>,
    suppressed_until: Option<usize>,
    ctx: &'b mut InlineContext<'a>,
}

impl Builder<'_, '_> {
    fn event(&mut self, event: &Event<'_>, range: &Range<usize>) {
        match event {
            Event::Text(text) => self.text_event(text, range),
            Event::Code(code) => {
                if let Some(image) = &mut self.image {
                    image.alt.push_str(code);
                    return;
                }
                if self.drop_depth > 0 {
                    return;
                }
                let start = self.text.len();
                let leading = self
                    .ctx
                    .source
                    .get(range.clone())
                    .map(|text| text.bytes().take_while(|byte| *byte == b'`').count())
                    .unwrap_or(0);
                self.append_raw(code, range.start + leading);
                self.spans.push(InlineSpan { range: start..self.text.len(), style: InlineStyle::Code });
                self.slug_text.push_str(code);
            }
            Event::SoftBreak => {
                if !self.suppressed(range.start) {
                    self.append_raw(" ", range.start);
                }
            }
            Event::HardBreak => {
                if !self.suppressed(range.start) {
                    let start = self.text.len();
                    self.append_raw("\n", range.start);
                    self.objects.push(InlineObject {
                        range: start..self.text.len(),
                        kind: InlineObjectKind::LineBreak,
                        html: None,
                    });
                }
            }
            Event::Start(tag) => self.start_tag(tag, range),
            Event::End(tag) => self.end_tag(tag),
            Event::FootnoteReference(label) => self.footnote_reference(label, range),
            Event::TaskListMarker(checked) => {
                let start = self.text.len();
                self.append_raw("\u{FFFC}", range.start);
                self.objects.push(InlineObject {
                    range: start..self.text.len(),
                    kind: InlineObjectKind::TaskCheckbox { checked: *checked },
                    html: None,
                });
            }
            Event::InlineHtml(raw) => self.inline_html(raw, range),
            Event::Html(raw) => {
                self.append_raw(raw, range.start);
                self.slug_text.push_str(raw);
            }
            Event::Rule | Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }

    fn start_tag(&mut self, tag: &Tag<'_>, range: &Range<usize>) {
        match tag {
            Tag::Emphasis => self.push_style(InlineStyle::Em),
            Tag::Strong => self.push_style(InlineStyle::Strong),
            Tag::Strikethrough => self.push_style(InlineStyle::Strike),
            Tag::Link { link_type, dest_url, title, .. } => {
                let dest = if *link_type == pulldown_cmark::LinkType::Email && !dest_url.contains(':') {
                    format!("mailto:{dest_url}")
                } else {
                    dest_url.to_string()
                };
                self.links_open.push(LinkFrame {
                    dest,
                    title: (!title.is_empty()).then(|| title.to_string()),
                    start: self.text.len(),
                });
            }
            Tag::Image { dest_url, title, .. } => {
                self.image = Some(ImageFrame {
                    src: dest_url.to_string(),
                    title: (!title.is_empty()).then(|| title.to_string()),
                    alt: String::new(),
                    source: range.start,
                });
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: &TagEnd) {
        match tag {
            TagEnd::Emphasis => self.pop_style(InlineStyle::Em),
            TagEnd::Strong => self.pop_style(InlineStyle::Strong),
            TagEnd::Strikethrough => self.pop_style(InlineStyle::Strike),
            TagEnd::Link => {
                if let Some(frame) = self.links_open.pop() {
                    self.push_link(frame);
                }
            }
            TagEnd::Image => {
                if let Some(frame) = self.image.take() {
                    let mut attrs = vec![("src".to_owned(), frame.src), ("alt".to_owned(), frame.alt)];
                    if let Some(title) = frame.title {
                        attrs.push(("title".to_owned(), title));
                    }
                    let node = HtmlNode { tag: HtmlTag::Img, attrs, children: Vec::new(), srcset: Vec::new() };
                    let start = self.text.len();
                    self.append_raw("\u{FFFC}", frame.source);
                    self.objects.push(InlineObject {
                        range: start..self.text.len(),
                        kind: InlineObjectKind::Image,
                        html: Some(node),
                    });
                }
            }
            _ => {}
        }
    }

    fn push_style(&mut self, style: InlineStyle) {
        self.styles.push(StyleFrame { style, start: self.text.len() });
    }

    fn pop_style(&mut self, style: InlineStyle) {
        if let Some(index) = self.styles.iter().rposition(|frame| frame.style == style) {
            let frame = self.styles.remove(index);
            let start = frame.start.min(self.text.len());
            if start < self.text.len() {
                self.spans.push(InlineSpan { range: start..self.text.len(), style });
            }
        }
    }

    fn push_link(&mut self, frame: LinkFrame) {
        let start = frame.start.min(self.text.len());
        let range = start..self.text.len();
        if range.is_empty() {
            return;
        }
        self.spans.push(InlineSpan { range: range.clone(), style: InlineStyle::Link });
        self.links.push(InlineLink { range, dest: frame.dest, title: frame.title });
    }

    fn text_event(&mut self, text: &str, range: &Range<usize>) {
        if let Some(image) = &mut self.image {
            // Alt text is plain text: drop formula/object placeholders rather
            // than leaking their control characters into the alt.
            for ch in text.chars() {
                if ch != OBJECT_CHAR && ch != crate::math::SHORT_OBJECT && ch != crate::math::PADDING_OBJECT {
                    image.alt.push(ch);
                }
            }
            return;
        }
        if self.drop_depth > 0 {
            return;
        }
        // Drop the blanked remainder of a formula before the next real text.
        let mut text = text;
        if let Some(until) = self.suppressed_until {
            if range.end <= until {
                return;
            }
            if range.start < until {
                let skip = (until - range.start).min(text.len());
                text = &text[skip..];
            }
            self.suppressed_until = None;
        }
        if text.is_empty() {
            return;
        }
        let before = self.text.len();
        // Each object character in the decoded text corresponds, in order, to
        // an object character in the event's source slice. Pairing by source
        // position keeps a literal U+FFFC from stealing a later formula.
        let positions: Vec<usize> = self
            .ctx
            .source
            .get(range.clone())
            .map(|slice| {
                slice
                    .char_indices()
                    .filter(|(_, ch)| *ch == OBJECT_CHAR || *ch == crate::math::SHORT_OBJECT)
                    .map(|(offset, _)| range.start + offset)
                    .collect()
            })
            .unwrap_or_default();
        let mut occurrence = 0;
        let mut local = 0;
        while local < text.len() {
            let Some((index, object)) = next_object(text, local) else { break };
            if object == crate::math::PADDING_OBJECT {
                local = index + object.len_utf8();
                continue;
            }
            self.append_segment(&text[local..index], range.start + local);
            let mut next = index + object.len_utf8();
            let object_source = positions.get(occurrence).copied();
            occurrence += 1;
            if let Some(source_start) = object_source
                && let Some(math) =
                    self.ctx.math_by_start.get(&source_start).copied().and_then(|index| self.ctx.math.get(index))
            {
                let start = self.text.len();
                self.append_raw(&OBJECT_CHAR.to_string(), source_start);
                self.math.push(InlineMath {
                    range: start..self.text.len(),
                    tex: math.tex.clone(),
                    display: math.display,
                });
                // Drop the spaces and line breaks that padded the formula in
                // the transformed source, both inside and after this event.
                let padding = math.source.end.saturating_sub(source_start + object.len_utf8());
                let skip = padding.min(text.len() - next);
                next += skip;
                self.suppressed_until = (padding > skip).then_some(math.source.end);
            } else {
                // A literal object character the author wrote: keep it.
                let mut buffer = [0u8; 4];
                self.append_raw(object.encode_utf8(&mut buffer), range.start + index);
            }
            local = next;
        }
        self.append_segment(&text[local..], range.start + local);
        let added = self.text.len() - before;
        if added > 0 {
            self.linkify(before, added);
        }
    }

    fn suppressed(&mut self, source: usize) -> bool {
        if let Some(until) = self.suppressed_until {
            if source < until {
                return true;
            }
            self.suppressed_until = None;
        }
        false
    }

    fn append_segment(&mut self, text: &str, source: usize) {
        if text.is_empty() {
            return;
        }
        let mut last = 0;
        for (index, ch) in text.char_indices() {
            if ch != '"' && ch != '\'' {
                continue;
            }
            if index > last {
                let plain = &text[last..index];
                self.append_raw(plain, source + last);
                self.slug_text.push_str(plain);
            }
            let next = text[index + ch.len_utf8()..].chars().next();
            let previous = self.text.chars().next_back();
            let curled = curled_quote(ch, previous, next);
            let mut buffer = [0u8; 4];
            self.append_raw(curled.encode_utf8(&mut buffer), source + index);
            self.slug_text.push(curled);
            last = index + ch.len_utf8();
        }
        if last < text.len() {
            let tail = &text[last..];
            self.append_raw(tail, source + last);
            self.slug_text.push_str(tail);
        }
    }

    fn append_raw(&mut self, text: &str, source: usize) {
        let start = self.text.len();
        self.text.push_str(text);
        self.source_map.push(start..self.text.len(), source);
    }

    fn linkify(&mut self, start: usize, length: usize) {
        if !self.links_open.is_empty() || !self.html_links.is_empty() {
            return;
        }
        let Some(segment) = self.text.get(start..start + length) else { return };
        for (range, dest) in find_links(segment) {
            let range = start + range.start..start + range.end;
            self.spans.push(InlineSpan { range: range.clone(), style: InlineStyle::Link });
            self.links.push(InlineLink { range, dest, title: None });
        }
    }

    fn footnote_reference(&mut self, label: &str, range: &Range<usize>) {
        if !self.ctx.definitions.contains(label) {
            let start = self.text.len();
            let literal = format!("[^{label}]");
            self.append_raw(&literal, range.start);
            self.slug_text.push_str(&literal);
            self.spans.push(InlineSpan { range: start..self.text.len(), style: InlineStyle::Plain });
            self.ctx.diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                code: DiagnosticCode::UndefinedFootnote,
                message: format!("footnote `{label}` has no definition"),
                line: Some(line_of(self.ctx.starts, range.start)),
                source: Some(range.clone()),
            });
            return;
        }
        let number = self.ctx.number_for(label);
        self.ctx.first_refs.entry(label.to_owned()).or_insert_with(|| range.clone());
        let text = number.to_string();
        let start = self.text.len();
        self.append_raw(&text, range.start);
        let span_range = start..self.text.len();
        self.spans.push(InlineSpan { range: span_range.clone(), style: InlineStyle::Sup });
        self.spans.push(InlineSpan { range: span_range.clone(), style: InlineStyle::Link });
        self.links.push(InlineLink { range: span_range, dest: format!("fn:{label}"), title: None });
    }

    fn inline_html(&mut self, raw: &str, range: &Range<usize>) {
        if self.drop_depth > 0 {
            if let Some(tag) = html::parse_tag(raw)
                && tag.closing
                && is_dropped(&tag.name)
            {
                self.drop_depth -= 1;
            }
            return;
        }
        if html::is_filtered_tag_name(raw) {
            self.append_raw(raw, range.start);
            self.slug_text.push_str(raw);
            return;
        }
        let Some(tag) = html::parse_tag(raw) else { return };
        if tag.closing {
            self.close_html(&tag.name);
            return;
        }
        let Some(allowed) = html::allowed_tag(&tag.name) else {
            // `object` has content to drop; `embed` and other void tags do
            // not, so text after them must survive.
            if matches!(tag.name.as_str(), "object" | "template" | "script" | "style" | "iframe" | "noscript") {
                self.drop_depth += 1;
            }
            return;
        };
        let (attrs, srcset) = html::filter_attrs(allowed, tag.attrs.clone());
        let node = HtmlNode { tag: allowed, attrs, children: Vec::new(), srcset };
        match allowed {
            HtmlTag::Br => {
                let start = self.text.len();
                self.append_raw("\n", range.start);
                self.objects.push(InlineObject {
                    range: start..self.text.len(),
                    kind: InlineObjectKind::LineBreak,
                    html: Some(node),
                });
            }
            HtmlTag::Img => {
                let start = self.text.len();
                self.append_raw(&OBJECT_CHAR.to_string(), range.start);
                self.objects.push(InlineObject {
                    range: start..self.text.len(),
                    kind: InlineObjectKind::Image,
                    html: Some(node),
                });
            }
            HtmlTag::Kbd
            | HtmlTag::Sub
            | HtmlTag::Sup
            | HtmlTag::Mark
            | HtmlTag::U
            | HtmlTag::S
            | HtmlTag::Del
            | HtmlTag::Ins
            | HtmlTag::Code
            | HtmlTag::Strong
            | HtmlTag::B
            | HtmlTag::Em
            | HtmlTag::I => {
                let style = style_for_tag(allowed);
                self.html_styles.push(HtmlStyleFrame {
                    name: tag.name.clone(),
                    style: Some(style),
                    start: self.text.len(),
                });
            }
            HtmlTag::A => {
                if let Some(href) = node.attr("href").filter(|href| !href.is_empty()) {
                    self.html_links.push(LinkFrame {
                        dest: href.to_owned(),
                        title: node.attr("title").map(str::to_owned),
                        start: self.text.len(),
                    });
                } else {
                    self.html_styles.push(HtmlStyleFrame {
                        name: tag.name.clone(),
                        style: None,
                        start: self.text.len(),
                    });
                }
            }
            HtmlTag::Span | HtmlTag::Abbr => {
                self.html_styles.push(HtmlStyleFrame { name: tag.name.clone(), style: None, start: self.text.len() });
            }
            HtmlTag::Details | HtmlTag::Picture | HtmlTag::Video | HtmlTag::Audio => {
                let kind = match allowed {
                    HtmlTag::Details => InlineObjectKind::Details,
                    HtmlTag::Picture => InlineObjectKind::Picture,
                    HtmlTag::Video => InlineObjectKind::Video,
                    _ => InlineObjectKind::Audio,
                };
                self.html_objects.push(HtmlObjectFrame { name: tag.name.clone(), kind, node, start: self.text.len() });
            }
            _ => {}
        }
    }

    fn close_html(&mut self, name: &str) {
        if let Some(index) = self.html_objects.iter().rposition(|frame| frame.name == name) {
            let frame = self.html_objects.remove(index);
            self.objects.push(InlineObject {
                range: frame.start..self.text.len(),
                kind: frame.kind,
                html: Some(frame.node),
            });
            return;
        }
        if name == "a"
            && let Some(frame) = self.html_links.pop()
        {
            self.push_link(frame);
            return;
        }
        if let Some(index) = self.html_styles.iter().rposition(|frame| frame.name == name) {
            let frame = self.html_styles.remove(index);
            if let Some(style) = frame.style {
                let start = frame.start.min(self.text.len());
                if start < self.text.len() {
                    self.spans.push(InlineSpan { range: start..self.text.len(), style });
                }
            }
        }
    }

    fn finish(mut self) -> BuiltInline {
        while let Some(frame) = self.styles.pop() {
            let start = frame.start.min(self.text.len());
            if start < self.text.len() {
                self.spans.push(InlineSpan { range: start..self.text.len(), style: frame.style });
            }
        }
        while let Some(frame) = self.html_styles.pop() {
            if let Some(style) = frame.style {
                let start = frame.start.min(self.text.len());
                if start < self.text.len() {
                    self.spans.push(InlineSpan { range: start..self.text.len(), style });
                }
            }
        }
        while let Some(frame) = self.links_open.pop() {
            self.push_link(frame);
        }
        while let Some(frame) = self.html_links.pop() {
            self.push_link(frame);
        }
        BuiltInline {
            inline: Inline {
                text: self.text,
                spans: self.spans,
                links: self.links,
                math: self.math,
                objects: self.objects,
                source_map: self.source_map,
            },
            slug_text: self.slug_text,
        }
    }
}

/// The next object character (U+FFFC, a short-formula sentinel or a filler).
fn next_object(text: &str, from: usize) -> Option<(usize, char)> {
    let mut best: Option<(usize, char)> = None;
    for object in [OBJECT_CHAR, crate::math::SHORT_OBJECT, crate::math::PADDING_OBJECT] {
        if let Some(offset) = text[from..].find(object) {
            let candidate = (from + offset, object);
            if best.is_none_or(|current| candidate.0 < current.0) {
                best = Some(candidate);
            }
        }
    }
    best
}

fn is_dropped(name: &str) -> bool {
    matches!(name, "object" | "embed" | "script" | "style" | "iframe" | "noscript")
}

fn style_for_tag(tag: HtmlTag) -> InlineStyle {
    match tag {
        HtmlTag::Kbd => InlineStyle::Kbd,
        HtmlTag::Sub => InlineStyle::Sub,
        HtmlTag::Sup => InlineStyle::Sup,
        HtmlTag::Mark => InlineStyle::Mark,
        HtmlTag::S | HtmlTag::Del => InlineStyle::Strike,
        HtmlTag::Ins | HtmlTag::U => InlineStyle::Underline,
        HtmlTag::Code => InlineStyle::Code,
        HtmlTag::Strong | HtmlTag::B => InlineStyle::Strong,
        HtmlTag::Em | HtmlTag::I => InlineStyle::Em,
        _ => InlineStyle::Plain,
    }
}

/// Curl one quote the way markdown-it's typographer does with replacements off.
fn curled_quote(quote: char, previous: Option<char>, next: Option<char>) -> char {
    if quote == '\'' {
        if previous.is_some_and(|ch| ch.is_alphanumeric()) && next.is_some_and(|ch| ch.is_alphanumeric()) {
            '\u{2019}'
        } else if opens_quote(previous) {
            '\u{2018}'
        } else {
            '\u{2019}'
        }
    } else if opens_quote(previous) {
        '\u{201c}'
    } else {
        '\u{201d}'
    }
}

fn opens_quote(previous: Option<char>) -> bool {
    previous.is_none_or(|ch| ch.is_whitespace() || "([{<—–".contains(ch))
}

/// Find `http(s)://`, `www.` and e-mail addresses in plain text.
fn find_links(text: &str) -> Vec<(Range<usize>, String)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !text.is_char_boundary(index) {
            index += 1;
            continue;
        }
        let rest = &text[index..];
        if let Some(scheme) = ["https://", "http://"]
            .iter()
            .find(|scheme| rest.get(..scheme.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme)))
        {
            let boundary = index == 0 || !is_url_word(bytes[index - 1]);
            if boundary {
                let end = url_end(bytes, index + scheme.len());
                if end > index + scheme.len() {
                    found.push((index..end, text[index..end].to_owned()));
                    index = end;
                    continue;
                }
            }
        }
        if rest.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("www.")) {
            let boundary = index == 0 || !matches!(bytes[index - 1], b'.' | b'/' | b'\\' | b'@' | b'-');
            let host_end = url_end(bytes, index + 4);
            let host = &text[index + 4..host_end];
            let has_tld = host.rsplit_once('.').is_some_and(|(_, tld)| tld.len() >= 2);
            if boundary && has_tld {
                found.push((index..host_end, format!("http://{}", &text[index..host_end])));
                index = host_end;
                continue;
            }
        }
        if bytes[index] == b'@'
            && let Some(range) = email_at(text, index)
        {
            let address = text[range.clone()].to_owned();
            index = range.end;
            found.push((range, format!("mailto:{address}")));
            continue;
        }
        index += 1;
    }
    found
}

fn is_url_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

/// The end of a URL starting after its scheme, trailing punctuation removed.
fn url_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() && !matches!(bytes[end], b'<' | b'>' | b'"' | b'\'') {
        end += 1;
    }
    trim_url_end(bytes, start, end)
}

fn trim_url_end(bytes: &[u8], start: usize, mut end: usize) -> usize {
    while let Some(last) = end.checked_sub(1).filter(|index| *index >= start) {
        let byte = bytes[last];
        let trim = match byte {
            b'.' | b',' | b';' | b':' | b'!' | b'?' => true,
            b')' => count_byte(bytes, start, end, b'(') < count_byte(bytes, start, end, b')'),
            b']' => count_byte(bytes, start, end, b'[') < count_byte(bytes, start, end, b']'),
            b'}' => count_byte(bytes, start, end, b'{') < count_byte(bytes, start, end, b'}'),
            _ => false,
        };
        if trim {
            end -= 1;
        } else {
            break;
        }
    }
    end
}

fn count_byte(bytes: &[u8], start: usize, end: usize, needle: u8) -> usize {
    bytes[start.min(end)..end].iter().filter(|byte| **byte == needle).count()
}

fn email_at(text: &str, at: usize) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    let mut start = at;
    while start > 0
        && (bytes[start - 1].is_ascii_alphanumeric() || matches!(bytes[start - 1], b'.' | b'_' | b'%' | b'+' | b'-'))
    {
        start -= 1;
    }
    if start == at {
        return None;
    }
    let mut end = at + 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'.' | b'-')) {
        end += 1;
    }
    while end > at + 1 && matches!(bytes[end - 1], b'.' | b'-') {
        end -= 1;
    }
    let domain = &text[at + 1..end];
    let (_, tld) = domain.rsplit_once('.')?;
    if tld.len() < 2 || !tld.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return None;
    }
    let boundary = start == 0 || !matches!(bytes[start - 1], b'.' | b'_' | b'%' | b'+' | b'-');
    if !boundary {
        return None;
    }
    if bytes.get(end).is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'-') {
        return None;
    }
    Some(start..end)
}
