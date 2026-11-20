//! The generated parts of a document: the front-matter card (M-12), GitHub
//! alert callouts (M-19), the footnote section (M-11) and the `[TOC]`
//! navigation (M-20).

use lmdv_md::{Block, Callout, CalloutKind, FmValue, FrontMatter};
use rune::{El, Icon, Span, TextHighlight, TextStyle, col, icon, rich, row, text};

use crate::block::Renderer;
use crate::inline::overlay;
use crate::{FOOTNOTES_KEY, FRONT_MATTER_KEY, TOC_KEY};

/// A `NOTE` card's icon: the information mark.
static NOTE: Icon = Icon {
    name: "lmdv-note",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.4" stroke="currentColor" stroke-width="1.3" fill="none"/><path d="M8 7.2v4.1" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><circle cx="8" cy="4.7" r=".8" fill="currentColor"/>"#,
};

/// A `TIP` card's icon: a light bulb.
static TIP: Icon = Icon {
    name: "lmdv-tip",
    view: 16.0,
    body: r#"<path d="M6 12.4h4M6.6 10.2h2.8M8 1.6a4.6 4.6 0 0 0-2.7 8.3c.4.3.6.7.6 1.2h4.2c0-.5.2-.9.6-1.2A4.6 4.6 0 0 0 8 1.6Z" stroke="currentColor" stroke-width="1.25" fill="none" stroke-linejoin="round"/>"#,
};

/// An `IMPORTANT` card's icon: a raised hand's exclamation (a circled mark).
static IMPORTANT: Icon = Icon {
    name: "lmdv-important",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.4" stroke="currentColor" stroke-width="1.3" fill="none"/><path d="M8 4.4v4.4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><circle cx="8" cy="11.4" r=".85" fill="currentColor"/>"#,
};

/// A `WARNING` card's icon: a triangle with a mark.
static WARNING: Icon = Icon {
    name: "lmdv-warning",
    view: 16.0,
    body: r#"<path d="M8 2.1 14.3 13.2H1.7Z" stroke="currentColor" stroke-width="1.25" fill="none" stroke-linejoin="round"/><path d="M8 6.2v3.4" stroke="currentColor" stroke-width="1.35" stroke-linecap="round"/><circle cx="8" cy="11.4" r=".8" fill="currentColor"/>"#,
};

/// A `CAUTION` card's icon: an octagon with a mark.
static CAUTION: Icon = Icon {
    name: "lmdv-caution",
    view: 16.0,
    body: r#"<path d="M5.4 1.9h5.2l3.5 3.5v5.2l-3.5 3.5H5.4L1.9 10.6V5.4Z" stroke="currentColor" stroke-width="1.25" fill="none" stroke-linejoin="round"/><path d="M8 4.6v3.6" stroke="currentColor" stroke-width="1.35" stroke-linecap="round"/><circle cx="8" cy="10.9" r=".8" fill="currentColor"/>"#,
};

/// The icon of `kind`.
pub(crate) fn callout_icon(kind: CalloutKind) -> &'static Icon {
    match kind {
        CalloutKind::Note => &NOTE,
        CalloutKind::Tip => &TIP,
        CalloutKind::Important => &IMPORTANT,
        CalloutKind::Warning => &WARNING,
        CalloutKind::Caution => &CAUTION,
    }
}

/// The paint index of `kind` in [`DocTheme`](crate::DocTheme::callouts).
pub(crate) fn callout_index(kind: CalloutKind) -> usize {
    match kind {
        CalloutKind::Note => 0,
        CalloutKind::Tip => 1,
        CalloutKind::Important => 2,
        CalloutKind::Warning => 3,
        CalloutKind::Caution => 4,
    }
}

/// The front-matter card: a summary of the keys, and the flat table when open.
pub(crate) fn front_matter(r: &Renderer<'_>, front: &FrontMatter) -> El {
    let sources = entry_sources(r, front);
    let keys = front.entries.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>().join(" · ");
    let open = r.opts.open_for(FRONT_MATTER_KEY);
    let title = r.theme.labels.front_matter.clone();
    let summary_label = if keys.is_empty() { title.clone() } else { format!("{title} · {keys}") };
    // The card has no source map of its own: a key's own source bytes paint
    // its occurrence in the summary while the card is closed (OI-127).
    let mut summary_marks = Vec::new();
    let separator = " · ".len();
    let mut key_at = title.len() + separator;
    for ((key, _), source) in front.entries.iter().zip(&sources) {
        for mark in painted_spans(r, key.len(), Some(source.key.clone())) {
            let range = mark.range();
            summary_marks.push(mark.with_range(key_at + range.start..key_at + range.end));
        }
        key_at += key.len() + separator;
    }
    let mut summary = rich(summary_label.clone(), vec![Span::new(0, title.len()).weight(600.0).color(r.theme.text)])
        .font(r.theme.small)
        .color(r.theme.muted)
        .link(rune::Link::new(0..summary_label.len(), crate::details_toggle(FRONT_MATTER_KEY)));
    if !summary_marks.is_empty() {
        summary = summary.text_highlights(summary_marks);
    }
    if let Some(wire) = &r.opts.wires.link {
        summary = wire(summary);
    }
    let mut card = col()
        .key(FRONT_MATTER_KEY)
        .role("note")
        .label(title)
        .expanded(open)
        .w_full()
        .radius(r.theme.radius)
        .fill(r.theme.code_fill)
        .border(0.5, r.theme.code_border)
        .border_layout()
        .px(12.0)
        .py(10.0)
        .child(summary);
    if open {
        if front.valid {
            let mut table = col().key("entries").w_full().pt(8.0);
            for (index, (key, value)) in front.entries.iter().enumerate() {
                table = table.child(entry_row(r, key, value, sources.get(index)));
            }
            card = card.child(table);
        } else {
            let source = front_raw_source(r, front);
            let raw =
                rich(front.raw.clone(), Vec::new()).key("raw").font(r.theme.code).color(r.theme.text).pt(8.0).w_full();
            card = card.child(raw.text_highlights(painted_spans(r, front.raw.len(), source)));
        }
    }
    card
}

/// The source span each front-matter entry was written in.
///
/// `key` is the key's bytes in the source; `values` are the drawn value bytes,
/// one for a scalar and one per list item. The card is generated chrome with no
/// source map, so these spans are what let a `--line` flash or a find match
/// paint a key or a value (OI-127).
#[derive(Clone, Debug, Default)]
struct EntrySource {
    key: std::ops::Range<usize>,
    values: Vec<std::ops::Range<usize>>,
}

/// Where the front matter's keys and values sit in the document source.
///
/// The walk mirrors the parser's flat subset: one `key: value` line per entry,
/// a `[a, b]` inline list, or indented `- item` lines. A value the walk cannot
/// locate is left unpainted instead of guessed at.
fn entry_sources(r: &Renderer<'_>, front: &FrontMatter) -> Vec<EntrySource> {
    if !front.valid || front.raw.is_empty() {
        return Vec::new();
    }
    let Some(base) = front_matter_base(r, front) else { return Vec::new() };
    let lines: Vec<&str> = front.raw.split('\n').collect();
    let mut offsets = Vec::with_capacity(lines.len());
    let mut at = base;
    for line in &lines {
        offsets.push(at);
        at += line.len() + 1;
    }
    let mut out = Vec::with_capacity(front.entries.len());
    let mut entry = 0usize;
    let mut index = 0usize;
    while index < lines.len() && entry < front.entries.len() {
        let line = lines[index];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            index += 1;
            continue;
        }
        let Some(colon) = line.find(':') else {
            index += 1;
            continue;
        };
        let key = line[..colon].trim_end();
        let key_span = offsets[index]..offsets[index] + key.len();
        let rest = line[colon + 1..].trim();
        index += 1;
        let mut values = Vec::new();
        match front.entries.get(entry).map(|(_, value)| value) {
            Some(FmValue::List(items)) if rest.is_empty() || rest == "|" || rest == ">" => {
                let mut found = 0usize;
                while index < lines.len() && found < items.len() {
                    let item_line = lines[index];
                    let Some(item) = list_item_text(item_line) else { break };
                    let from = item_line.find('-').map_or(0, |dash| dash + 1);
                    let Some(pos) = item_line[from..].find(item) else { break };
                    let start = offsets[index] + from + pos;
                    values.push(start..start + item.len());
                    found += 1;
                    index += 1;
                }
            }
            Some(FmValue::List(items)) => {
                let mut cursor = 0usize;
                for item in items {
                    let Some(pos) = line[colon + 1 + cursor..].find(item.as_str()) else { break };
                    let start = offsets[index - 1] + colon + 1 + cursor + pos;
                    values.push(start..start + item.len());
                    cursor += pos + item.len();
                }
            }
            Some(FmValue::Scalar(value)) => {
                if !value.is_empty()
                    && let Some(pos) = line[colon + 1..].find(value.as_str())
                {
                    let start = offsets[index - 1] + colon + 1 + pos;
                    values.push(start..start + value.len());
                }
            }
            None => break,
        }
        out.push(EntrySource { key: key_span, values });
        entry += 1;
    }
    out
}

/// The front matter's `raw` text inside the document source.
fn front_matter_base(r: &Renderer<'_>, front: &FrontMatter) -> Option<usize> {
    let source = r.doc.source.as_ref();
    let head_end = r.doc.block_spans.first().map_or(source.len(), |span| span.source.start);
    let head = source.get(..head_end)?;
    let base = head.find(front.raw.as_str())?;
    // The block's own draw begins with the opening fence; a raw string that
    // repeats in a later block must not be painted as front matter.
    (base < head_end).then_some(base)
}

/// The source span of a raw, invalid front-matter block.
fn front_raw_source(r: &Renderer<'_>, front: &FrontMatter) -> Option<std::ops::Range<usize>> {
    let base = front_matter_base(r, front)?;
    (!front.raw.is_empty()).then_some(base..base + front.raw.len())
}

/// The text after a `-` list marker, trimmed, when `line` is a list item.
fn list_item_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    if trimmed.len() == line.len() {
        return None;
    }
    let rest = trimmed.strip_prefix('-')?;
    Some(rest.trim_start_matches([' ', '\t']))
}

/// One key/value row of the front-matter table; lists become chips.
fn entry_row(r: &Renderer<'_>, key: &str, value: &FmValue, source: Option<&EntrySource>) -> El {
    let theme = r.theme;
    let mut right = row().gap(4.0).flex_wrap().grow(1.0);
    match value {
        FmValue::Scalar(value) => {
            let element = text(value.clone()).font(theme.small).color(theme.text).text_highlights(painted_spans(
                r,
                value.len(),
                source.and_then(|s| s.values.first().cloned()),
            ));
            right = right.child(element);
        }
        FmValue::List(items) => {
            for (index, item) in items.iter().enumerate() {
                let element = text(item.clone())
                    .font(theme.small)
                    .color(theme.text)
                    .px(5.0)
                    .py(1.0)
                    .radius(3.0)
                    .fill(theme.code_bg)
                    .text_highlights(painted_spans(r, item.len(), source.and_then(|s| s.values.get(index).cloned())));
                right = right.child(element);
            }
        }
    }
    row()
        .key(format!("entry-{key}"))
        .w_full()
        .py(3.0)
        .gap(12.0)
        .items_start()
        .child(
            text(key.to_owned()).font(theme.small).color(theme.muted).w(110.0).shrink().text_highlights(painted_spans(
                r,
                key.len(),
                source.map(|s| s.key.clone()),
            )),
        )
        .child(right)
}

/// The paint-only spans over `len` drawn bytes whose source starts at `source`.
///
/// The drawn text is copied byte for byte from that source (a front-matter key or
/// value), so a highlight's overlap maps by subtraction; `overlay` then resolves
/// several highlights to one paint each (OI-127).
fn painted_spans(r: &Renderer<'_>, len: usize, source: Option<std::ops::Range<usize>>) -> Vec<TextHighlight> {
    let Some(source) = source.filter(|source| !source.is_empty() && !r.opts.highlights.is_empty()) else {
        return Vec::new();
    };
    let mut parts = Vec::new();
    for (index, highlight) in r.opts.highlights.iter().enumerate() {
        let start = highlight.range.start.max(source.start);
        let end = highlight.range.end.min(source.end);
        if start >= end {
            continue;
        }
        let from = start - source.start;
        let to = (end - source.start).min(len);
        if from < to {
            parts.push((from..to, highlight.kind, index));
        }
    }
    overlay(parts, len)
        .into_iter()
        .map(|(range, kind, index)| {
            TextHighlight::new(range, r.highlight_color(kind)).key(crate::highlight_key(index)).pad(2.0)
        })
        .collect()
}

/// A GitHub alert: a tinted card with an icon, a title and ordinary blocks.
pub(crate) fn callout(r: &Renderer<'_>, callout: &Callout, blocks: &[Block]) -> El {
    let index = callout_index(callout.kind);
    let paint = r.theme.callouts[index];
    let title = callout.title.clone().unwrap_or_else(|| r.theme.labels.callouts[index].clone());
    let mut body = col().key("body").w_full().pl(12.0).pt(2.0);
    for (i, block) in blocks.iter().enumerate() {
        let (top, bottom) = r.margins(block);
        let top = if i == 0 { 0.0 } else { top };
        body = body.child(
            col()
                .w_full()
                .mt(top)
                .mb(if i + 1 == blocks.len() { 0.0 } else { bottom })
                .child(r.draw_block(block, r.nested_block())),
        );
    }
    col()
        .key("callout")
        .role("note")
        .label(title.clone())
        .w_full()
        .radius(r.theme.radius)
        .fill(paint.fill)
        .border_sides([0.0, 0.0, 0.0, 3.0], paint.accent)
        .border_layout()
        .py(8.0)
        .pr(12.0)
        .child(
            row()
                .key("title")
                .items_center()
                .gap(8.0)
                .pb(6.0)
                .pl(12.0)
                .child(icon(callout_icon(callout.kind), 16.0).color(paint.accent))
                .child(text(title).font(TextStyle { size: 15.0, weight: 600.0, ..r.theme.body }).color(paint.accent)),
        )
        .child(body)
}

/// The generated footnote section: a rule, then each referenced definition with
/// a backlink to its first reference (M-11).
pub(crate) fn footnotes(r: &Renderer<'_>) -> El {
    let mut section = col().key(FOOTNOTES_KEY).role("region").label(r.theme.labels.footnotes.clone()).w_full();
    if r.doc.footnotes.is_empty() {
        return section;
    }
    section = section.child(crate::horizontal_rule(r.theme.rule).mt(8.0).mb(10.0));
    let mut list = col().key("list").role("list").w_full();
    for footnote in &r.doc.footnotes {
        let label = crate::footnote_key(&footnote.label);
        let mut body = col().key("body").grow(1.0).shrink();
        for (i, block) in footnote.blocks.iter().enumerate() {
            let (top, bottom) = r.margins(block);
            let top = if i == 0 { 0.0 } else { top.max(6.0) };
            body = body.child(col().w_full().mt(top).mb(bottom).child(r.draw_block(block, r.nested_block())));
        }
        let back = r.action_chip(&r.theme.labels.footnote_back, format!("fnref:{}", footnote.label));
        let entry_label = lmdv_i18n::render(&r.theme.labels.footnote, &[("n", &footnote.number)]);
        list = list.child(
            row()
                .key(label)
                .role("listitem")
                .label(entry_label)
                .w_full()
                .gap(6.0)
                .items_start()
                .pt(6.0)
                .child(
                    text(format!("{}.", footnote.number))
                        .font(r.theme.small)
                        .color(r.theme.muted)
                        .pt(4.0)
                        .a11y_hidden(),
                )
                .child(body)
                .child(back.mt(2.0)),
        );
    }
    section.child(list)
}

/// The `[TOC]` placeholder: a nested navigation of the headings after it (M-20).
pub(crate) fn toc(r: &Renderer<'_>, block: usize) -> El {
    let heads: Vec<(usize, u8, &str)> = r
        .doc
        .headings
        .iter()
        .enumerate()
        .filter(|(_, heading)| heading.in_toc && heading.block > block)
        .map(|(index, heading)| (index, heading.level, heading.text.as_str()))
        .collect();
    let top = heads.iter().map(|(_, level, _)| *level).min().unwrap_or(1);
    let mut nav = col().key(TOC_KEY).role("navigation").label(r.theme.labels.toc.clone()).w_full().gap(4.0);
    if heads.is_empty() {
        return nav;
    }
    let nodes = toc_tree(&heads, top);
    for (depth, index, label) in nodes {
        nav = nav.child(toc_entry(r, depth, index, label));
    }
    nav
}

/// Flattens balanced nesting into `(depth, heading index, text)` rows.
///
/// A skipped heading level (an `h4` straight after an `h2`) nests one deeper
/// rather than opening empty list levels; the original page opens one `ul` per
/// skipped level (`renderer.ts:786-830`), a purely visual difference.
fn toc_tree<'a>(heads: &[(usize, u8, &'a str)], top: u8) -> Vec<(usize, usize, &'a str)> {
    let mut out = Vec::with_capacity(heads.len());
    let mut ancestors: Vec<u8> = Vec::new();
    for (index, level, label) in heads {
        let level = (*level).max(top);
        while ancestors.last().is_some_and(|last| level <= *last) {
            ancestors.pop();
        }
        out.push((ancestors.len(), *index, *label));
        ancestors.push(level);
    }
    out
}

fn toc_entry(r: &Renderer<'_>, depth: usize, heading: usize, label: &str) -> El {
    let dest = format!("#{}", r.doc.headings[heading].id);
    let mut e = text(label.to_owned())
        .key(format!("toc-{heading}"))
        .font(r.theme.body)
        .color(r.theme.accent)
        .exact_lines()
        .pl(depth as f32 * 16.0)
        .py(2.0)
        .w_full()
        .link(rune::Link::new(0..label.len(), dest));
    let marks = toc_marks(r, heading, label.len());
    if !marks.is_empty() {
        e = e.text_highlights(marks);
    }
    if r.opts.selectable {
        e = e.selectable(true).selection_color(r.theme.selection).copy_breaks(1, 0);
    }
    if let Some(wire) = &r.opts.wires.link {
        e = wire(e);
    }
    e
}

/// The paint over a generated TOC label whose heading's source a highlight covers.
///
/// The label is drawn from the heading's text, which is the only source it has:
/// mapping the highlight through the heading's own inline source map, then past
/// the object markers the TOC drops, paints the matching bytes (OI-127). Find
/// hits are skipped: the search counts one hit per heading and reveals the
/// heading, so painting the earlier TOC copy would move the reveal there; a
/// `--line` flash or a landing band still lights both drawn copies.
fn toc_marks(r: &Renderer<'_>, heading: usize, len: usize) -> Vec<TextHighlight> {
    if r.opts.highlights.is_empty() {
        return Vec::new();
    }
    let Some(Block::Heading { inline, .. }) = r.doc.blocks.get(r.doc.headings[heading].block) else {
        return Vec::new();
    };
    let mut parts = Vec::new();
    for (index, highlight) in r.opts.highlights.iter().enumerate() {
        if matches!(highlight.kind, crate::HighlightKind::Find | crate::HighlightKind::Current) {
            continue;
        }
        let Some(rendered) = crate::inline::rendered_range(&inline.source_map, highlight.range.clone()) else {
            continue;
        };
        let start = label_byte(&inline.text, rendered.start).min(len);
        let end = label_byte(&inline.text, rendered.end).min(len);
        if start < end {
            parts.push((start..end, highlight.kind, index));
        }
    }
    overlay(parts, len)
        .into_iter()
        .map(|(range, kind, index)| {
            TextHighlight::new(range, r.highlight_color(kind)).key(crate::highlight_key(index)).pad(2.0)
        })
        .collect()
}

/// A drawn byte in a TOC label: the heading text without the object markers the
/// label drops.
fn label_byte(text: &str, rendered: usize) -> usize {
    let limit = rendered.min(text.len());
    let mut out = 0usize;
    for (index, ch) in text.char_indices() {
        if index >= limit {
            break;
        }
        if ch == lmdv_md::OBJECT_CHAR {
            // A range inside the marker still names the object's own position.
            if limit < index + ch.len_utf8() {
                break;
            }
            continue;
        }
        if index + ch.len_utf8() > limit {
            break;
        }
        out += ch.len_utf8();
    }
    out
}
