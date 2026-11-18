//! The generated parts of a document: the front-matter card (M-12), GitHub
//! alert callouts (M-19), the footnote section (M-11) and the `[TOC]`
//! navigation (M-20).

use lmdv_md::{Block, Callout, CalloutKind, FmValue, FrontMatter};
use rune::{El, Icon, Span, TextStyle, col, icon, rich, row, text};

use crate::block::Renderer;
use crate::theme::DocTheme;
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

/// The paint index of `kind` in [`DocTheme::callouts`].
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
    let keys = front.entries.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>().join(" · ");
    let open = r.opts.open_for(FRONT_MATTER_KEY);
    let title = r.theme.labels.front_matter.clone();
    let summary_label = if keys.is_empty() { title.clone() } else { format!("{title} · {keys}") };
    let mut summary = rich(summary_label.clone(), vec![Span::new(0, title.len()).weight(600.0).color(r.theme.text)])
        .font(r.theme.small)
        .color(r.theme.muted)
        .link(rune::Link::new(0..summary_label.len(), crate::details_toggle(FRONT_MATTER_KEY)));
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
            for (key, value) in &front.entries {
                table = table.child(entry_row(r.theme, key, value));
            }
            card = card.child(table);
        } else {
            card = card.child(
                rich(front.raw.clone(), Vec::new()).key("raw").font(r.theme.code).color(r.theme.text).pt(8.0).w_full(),
            );
        }
    }
    card
}

/// One key/value row of the front-matter table; lists become chips.
fn entry_row(theme: &DocTheme, key: &str, value: &FmValue) -> El {
    let mut right = row().gap(4.0).flex_wrap().grow(1.0);
    match value {
        FmValue::Scalar(value) => {
            right = right.child(text(value.clone()).font(theme.small).color(theme.text));
        }
        FmValue::List(items) => {
            for item in items {
                right = right.child(
                    text(item.clone())
                        .font(theme.small)
                        .color(theme.text)
                        .px(5.0)
                        .py(1.0)
                        .radius(3.0)
                        .fill(theme.code_bg),
                );
            }
        }
    }
    row()
        .key(format!("entry-{key}"))
        .w_full()
        .py(3.0)
        .gap(12.0)
        .items_start()
        .child(text(key.to_owned()).font(theme.small).color(theme.muted).w(110.0).shrink())
        .child(right)
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
    if r.opts.selectable {
        e = e.selectable(true).selection_color(r.theme.selection).copy_breaks(1, 0);
    }
    if let Some(wire) = &r.opts.wires.link {
        e = wire(e);
    }
    e
}
