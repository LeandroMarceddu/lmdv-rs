//! The allowed HTML subset (M-21–M-23, M-27, M-29).
//!
//! `lmdv-md` has already applied GitHub's tagfilter and the attribute allowlist
//! and produced a plain [`lmdv_md::HtmlNode`] tree; this module maps that tree
//! to native elements. It is not a browser: there is no CSS engine, and a tag
//! with no native equivalent keeps its children as text.
//!
//! Media is a Linux placeholder (M-27): a card naming the source, with an
//! “Open externally” action, because Rune has no media backend at this rev.

use std::f32::consts::{FRAC_PI_2, TAU};

use lmdv_md::{
    Align, HtmlChild, HtmlNode, HtmlTag, ImageBlock, Inline, InlineLink, InlineObject, InlineObjectKind, InlineSpan,
    InlineStyle, SrcSetCandidate, Table,
};
use rune::shape::{FillRule, LineCap, Path, Shape, SvgPathSink, parse_svg_path};
use rune::{Color, El, col, el, row, text};

use crate::block::Renderer;
use crate::inline::InlineOptions;

impl Renderer<'_> {
    /// A block-level HTML fragment.
    pub(crate) fn html_block(&self, node: &HtmlNode, block: usize) -> El {
        let key = self.doc.block_key(block).map(|_| "html".to_owned()).unwrap_or_else(|| format!("html-{block}"));
        match node.tag {
            HtmlTag::Fragment | HtmlTag::Div | HtmlTag::P => self.html_children(node, &key, block),
            HtmlTag::Details => self.html_details(node, &key),
            HtmlTag::Table => self.html_table(node),
            HtmlTag::Ul | HtmlTag::Ol => self.html_list(node),
            HtmlTag::Blockquote => self.html_quote(node),
            HtmlTag::Pre => self.html_pre(node),
            HtmlTag::Img => self.html_image(node, block),
            HtmlTag::Picture => match self.best_picture_candidate(node).filter(|n| n.tag == HtmlTag::Img) {
                Some(img) => self.image_block(&self.image_block_from(img)),
                None => self.html_children(node, &key, block),
            },
            HtmlTag::Svg => self.html_svg(node, &key),
            HtmlTag::Video => self.html_media(node, "video"),
            HtmlTag::Audio => self.html_media(node, "audio"),
            _ => {
                // An inline-ish element on its own line still reads as a paragraph.
                let inline = self.inline_from_node(node);
                self.inline(&inline, InlineOptions::new(self.theme.body, self.theme.text))
            }
        }
    }

    /// Children in order: runs of inline content become one paragraph, block
    /// children are drawn as blocks.
    fn html_children(&self, node: &HtmlNode, key: &str, block: usize) -> El {
        // L-12: an image map's `<map>` is metadata, and an `<img usemap>` may
        // come before it in the same fragment. Collect the block's maps once,
        // before any picture is drawn.
        self.ensure_maps(node, block);
        let mut outer = col().key(key).w_full();
        let mut pending = Inline::default();
        let mut blocks: Vec<El> = Vec::new();
        for (index, child) in node.children.iter().enumerate() {
            match child {
                HtmlChild::Text(text) => push_text(&mut pending, text),
                HtmlChild::Node(child) if is_block_tag(child.tag) => {
                    flush_inline(self, &mut pending, &mut blocks);
                    let part = format!("part-{index}");
                    blocks.push(col().key(part).w_full().child(self.html_block(child, block)));
                }
                HtmlChild::Node(child) => {
                    let mut stack = Vec::new();
                    push_node(&mut pending, child, &mut stack);
                }
            }
        }
        flush_inline(self, &mut pending, &mut blocks);
        for (index, block) in blocks.into_iter().enumerate() {
            let mt = if index == 0 { 0.0 } else { 8.0 };
            outer = outer.child(col().w_full().mt(mt).child(block));
        }
        outer
    }

    /// `<details>`: a compact card. Its body shows when the app reports the key
    /// open, or initially when the source wrote `open` (OI-026). A blank line
    /// no longer cuts the card's body off: the parser keeps the whole
    /// `<details>` span in one HTML node.
    fn html_details(&self, node: &HtmlNode, block_key: &str) -> El {
        let base = format!("details:{block_key}:{:x}", node_hash(node));
        // The state is a set of open keys. A default-open card uses a separate
        // `:closed` key so the first press really closes it and the next opens
        // it again, without needing the app to pre-seed its state.
        let default_open = node.attr("open").is_some();
        let (key, open) = if default_open {
            let closed = format!("{base}:closed");
            let open = !self.opts.open_for(&closed);
            (closed, open)
        } else {
            let open = self.opts.open_for(&base);
            (base, open)
        };
        let summary = node.children.iter().find_map(|child| match child {
            HtmlChild::Node(child) if child.tag == HtmlTag::Summary => Some(child.text()),
            _ => None,
        });
        let title = summary.clone().unwrap_or_else(|| self.theme.labels.details.clone());
        let label = if title.is_empty() { self.theme.labels.details.clone() } else { title };
        let title_text = text(label.clone())
            .font(self.theme.small)
            .color(self.theme.text)
            .link(rune::Link::new(0..label.len(), crate::details_toggle(&key)));
        let mut title_text = title_text;
        if let Some(wire) = &self.opts.wires.link {
            title_text = wire(title_text);
        }
        let mut card = col()
            .key("details")
            .role("note")
            .label(label)
            .expanded(open)
            .w_full()
            .radius(self.theme.radius)
            .fill(self.theme.code_fill)
            .border(0.5, self.theme.code_border)
            .border_layout()
            .px(12.0)
            .py(8.0)
            .child(title_text);
        if open {
            let mut inline = Inline::default();
            let mut stack = Vec::new();
            for child in &node.children {
                match child {
                    HtmlChild::Node(child) if child.tag == HtmlTag::Summary => {}
                    HtmlChild::Node(child) => push_node(&mut inline, child, &mut stack),
                    HtmlChild::Text(text) => push_text(&mut inline, text),
                }
            }
            if !inline.is_empty() {
                card = card.child(self.inline(&inline, InlineOptions::new(self.theme.body, self.theme.text)).pt(6.0));
            }
        }
        card
    }

    /// An HTML table mapped onto the document table renderer.
    fn html_table(&self, node: &HtmlNode) -> El {
        let mut rows: Vec<Vec<&HtmlNode>> = Vec::new();
        collect_rows(node, &mut rows);
        let mut head: Vec<Inline> = Vec::new();
        let mut body: Vec<Vec<Inline>> = Vec::new();
        let mut aligns: Vec<Align> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let cells: Vec<Inline> = row.iter().map(|cell| self.inline_from_node(cell)).collect();
            if index == 0 && (row.iter().any(|cell| cell.tag == HtmlTag::Th) || head.is_empty()) {
                aligns = row.iter().map(|cell| html_align(cell.attr("align"))).collect();
                head = cells;
            } else {
                body.push(cells);
            }
        }
        let table = Table { align: aligns, head, rows: body, source: 0..0, line: 0 };
        self.table(&table)
    }

    /// A `<ul>`/`<ol>`: a marker column and each item's inline content.
    fn html_list(&self, node: &HtmlNode) -> El {
        let ordered = node.tag == HtmlTag::Ol;
        let start = node.attr("start").and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
        let mut list = col().key("list").role("list").w_full();
        let mut index = 0usize;
        for child in &node.children {
            let HtmlChild::Node(item) = child else { continue };
            if item.tag != HtmlTag::Li {
                continue;
            }
            let marker = if ordered { format!("{}.", start + index as u64) } else { "•".to_owned() };
            let inline = self.inline_from_node(item);
            let items =
                node.children.iter().filter(|c| matches!(c, HtmlChild::Node(n) if n.tag == HtmlTag::Li)).count();
            list = list.child(
                row()
                    .key(format!("i{index}"))
                    .role("listitem")
                    .position_in_set(index + 1, items)
                    .w_full()
                    .gap(4.0)
                    .mt(if index == 0 { 0.0 } else { 2.0 })
                    .child(
                        text(marker)
                            .font(self.theme.body)
                            .color(self.theme.muted)
                            .w(self.theme.list_indent - 4.0)
                            .align(rune::Align::End)
                            .a11y_hidden(),
                    )
                    .child(
                        self.inline(
                            &inline,
                            InlineOptions::new(self.theme.body, self.theme.text)
                                .breaks(if index == 0 { 2 } else { 1 }, 1)
                                .copy_prefix(if ordered {
                                    format!("{}. ", start + index as u64)
                                } else {
                                    "- ".to_owned()
                                }),
                        )
                        .grow(1.0)
                        .shrink(),
                    ),
            );
            index += 1;
        }
        list
    }

    fn html_quote(&self, node: &HtmlNode) -> El {
        let inline = self.inline_from_node(node);
        let body = self.inline(&inline, InlineOptions::new(self.theme.body, self.theme.text));
        row()
            .key("quote")
            .role("blockquote")
            .w_full()
            .child(col().w(3.0).h_full().fill(self.theme.quote_rule))
            .child(col().key("body").grow(1.0).shrink().pl(14.0).child(body))
    }

    fn html_pre(&self, node: &HtmlNode) -> El {
        let lang = node.children.iter().find_map(|child| match child {
            HtmlChild::Node(child) if child.tag == HtmlTag::Code => child
                .attr("class")
                .and_then(|class| class.split_whitespace().find_map(|part| part.strip_prefix("language-")))
                .map(str::to_owned),
            _ => None,
        });
        self.code_block(lang.as_deref(), &node.text(), "code", None)
    }

    /// An `<img>`, with its image map's `<area>` hit boxes when it names one
    /// (L-12).
    ///
    /// Without a map, or without an [`AreaWire`](crate::AreaWire), this is the
    /// ordinary block picture. With both, the picture keeps its media wire and
    /// transparent link boxes are pinned over it at the areas' scaled geometry;
    /// `circle` and `poly` get the box around their points (Rune has no shaped
    /// hit region), and the exact source geometry rides on
    /// [`AreaRef`](crate::AreaRef).
    fn html_image(&self, node: &HtmlNode, block: usize) -> El {
        let image = self.image_block_from(node);
        // An image map is a safe-subset feature only when the picture, the map
        // and a handler all exist. A missing picture draws its placeholder, and
        // a hit box over it would open a link for a picture nobody sees.
        let map = node
            .attr("usemap")
            .map(str::trim)
            .and_then(|name| name.strip_prefix('#'))
            .filter(|name| !name.is_empty())
            .and_then(|name| self.map_areas(name, block));
        let (Some((map_name, areas)), Some(wire)) = (map, self.opts.wires.area.as_ref()) else {
            return self.image_block(&image);
        };
        let (src, alt, title) = self.image_source(&image);
        let Some((drawn, box_)) = self.drawn_image(src, alt, title) else {
            return self.image_block(&image);
        };
        let block_key = self.doc.block_key(block).map(str::to_owned).unwrap_or_else(|| format!("b{block}"));
        let mut frame = el().key("image-map").w(box_.width).h(box_.height).child(drawn);
        for (index, area) in areas.iter().enumerate() {
            let Some(href) = area.attr("href").filter(|href| !href.is_empty()) else { continue };
            let shape = area_shape(area.attr("shape"));
            let coords = area_coords(area.attr("coords"));
            let hit = area_hit_box(shape, &coords, box_.natural, box_.width, box_.height);
            if hit[2] <= 0.0 || hit[3] <= 0.0 {
                continue;
            }
            let label = area
                .attr("alt")
                .filter(|alt| !alt.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| href.to_owned());
            let reference = crate::AreaRef {
                block: block_key.clone(),
                map: map_name.clone(),
                dest: href.to_owned(),
                alt: label.clone(),
                shape,
                coords,
                hit_box: hit,
            };
            let hit_box = el()
                .key(format!("area-{index}"))
                .absolute(hit[0], hit[1])
                .size(hit[2], hit[3])
                .role("link")
                .label(label)
                .focusable(true)
                .cursor(rune::Cursor::Pointer);
            frame = frame.child(wire(hit_box, &reference));
        }
        frame
    }

    /// An inline `<svg>` fragment (L-12, OI-195).
    ///
    /// The parser keeps the safe subset: `<a>`/`xlink:href` links (both stored
    /// as `href`), any text, and geometry/presentation attributes of the
    /// supported shapes. Shapes paint through `El::shape` inside a labelled
    /// group; link text stays below the drawing so its runs keep their clicks.
    /// An SVG with no drawable shapes or text stays a labelled group rather
    /// than an empty box.
    fn html_svg(&self, node: &HtmlNode, key: &str) -> El {
        let shape = svg_shape(node, self.theme.text);
        let label = "SVG".to_owned();
        let mut group = col().key(format!("{key}-svg")).role("group").label(label).w_full();
        if shape.is_empty() {
            let inline = self.inline_from_node(node);
            if inline.text.trim().is_empty() {
                return group.h(0.0);
            }
            return group.child(self.inline(&inline, InlineOptions::new(self.theme.body, self.theme.text)));
        }
        let [width, height] = svg_size(node);
        group = group.child(col().key("drawing").w(width).h(height).shape(shape));
        let inline = self.inline_from_node(node);
        if !inline.text.trim().is_empty() {
            group = group.child(
                col()
                    .key("links")
                    .w(width)
                    .child(self.inline(&inline, InlineOptions::new(self.theme.body, self.theme.text)))
                    .mt(2.0),
            );
        }
        group
    }

    /// Collects the block's `<map>` nodes once per block (L-12).
    fn ensure_maps(&self, node: &HtmlNode, block: usize) {
        let mut maps = self.html_maps.borrow_mut();
        if maps.0 == block && !maps.1.is_empty() {
            return;
        }
        maps.0 = block;
        maps.1.clear();
        collect_maps(node, &mut maps.1);
    }

    /// The named map's name and `<area>` children, from the block's maps.
    fn map_areas(&self, name: &str, block: usize) -> Option<(String, Vec<HtmlNode>)> {
        let maps = self.html_maps.borrow();
        if maps.0 != block {
            return None;
        }
        let map = maps.1.iter().find(|map| map.attr("name").is_some_and(|candidate| candidate == name))?;
        let areas = map
            .children
            .iter()
            .filter_map(|child| match child {
                HtmlChild::Node(child) if child.tag == HtmlTag::Area => Some(child.clone()),
                _ => None,
            })
            .collect();
        Some((name.to_owned(), areas))
    }

    /// A video/audio placeholder card with an “Open externally” action.
    fn html_media(&self, node: &HtmlNode, kind: &str) -> El {
        let src = node_src(node).unwrap_or_default();
        let poster = node.attr("poster").unwrap_or_default();
        let label = if src.is_empty() { format!("<{kind}>") } else { src.to_owned() };
        let mut card = col()
            .key("media")
            .role("group")
            .label(format!("{kind}: {label}"))
            .w_full()
            .radius(self.theme.radius)
            .fill(self.theme.code_fill)
            .border(0.5, self.theme.code_border)
            .border_layout()
            .px(12.0)
            .py(10.0)
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .child(text(format!("{kind}: {label}")).font(self.theme.body).color(self.theme.text).grow(1.0))
                    .child(self.action_chip(&self.theme.labels.open_externally, crate::media_link(src))),
            );
        if !src.is_empty() {
            card = card.child(self.action_chip(&self.theme.labels.open_externally, crate::media_link(src)).mt(6.0));
        }
        if !poster.is_empty() {
            card = card.child(text(format!("Poster: {poster}")).font(self.theme.small).color(self.theme.faint).pt(4.0));
        }
        card
    }

    /// The synthetic image block for an `<img>`/`<picture>`.
    fn image_block_from(&self, node: &HtmlNode) -> ImageBlock {
        ImageBlock {
            src: node_src(node).unwrap_or_default().to_owned(),
            alt: node.attr("alt").unwrap_or_default().to_owned(),
            title: node.attr("title").map(str::to_owned),
            srcset: node
                .srcset
                .iter()
                .map(|candidate| SrcSetCandidate {
                    url: candidate.url.clone(),
                    descriptor: candidate.descriptor.clone(),
                })
                .collect(),
            picture: Vec::new(),
            source: 0..0,
            line: 0,
        }
    }

    /// An [`Inline`] for an HTML subtree, so HTML prose shares the Markdown
    /// renderer's styles, links and selection.
    fn inline_from_node(&self, node: &HtmlNode) -> Inline {
        let mut inline = Inline::default();
        let mut stack = Vec::new();
        push_node(&mut inline, node, &mut stack);
        inline
    }
}

fn is_block_tag(tag: HtmlTag) -> bool {
    matches!(
        tag,
        HtmlTag::Div
            | HtmlTag::P
            | HtmlTag::Details
            | HtmlTag::Table
            | HtmlTag::Ul
            | HtmlTag::Ol
            | HtmlTag::Blockquote
            | HtmlTag::Pre
            | HtmlTag::Img
            | HtmlTag::Picture
            | HtmlTag::Svg
            | HtmlTag::Video
            | HtmlTag::Audio
    )
}

/// Appends an HTML child to `inline`, with the style `stack` in effect.
fn push_node(inline: &mut Inline, node: &HtmlNode, stack: &mut Vec<InlineStyle>) {
    use HtmlTag::*;
    let start = inline.text.len();
    match node.tag {
        Br => {
            inline.text.push('\n');
            inline.objects.push(InlineObject {
                range: inline.text.len()..inline.text.len(),
                kind: InlineObjectKind::LineBreak,
                html: None,
            });
        }
        Img => {
            let start = inline.text.len();
            inline.text.push(lmdv_md::OBJECT_CHAR);
            inline.objects.push(InlineObject {
                range: start..inline.text.len(),
                kind: InlineObjectKind::Image,
                html: Some(node.clone()),
            });
        }
        Picture => {
            let start = inline.text.len();
            inline.text.push(lmdv_md::OBJECT_CHAR);
            inline.objects.push(InlineObject {
                range: start..inline.text.len(),
                kind: InlineObjectKind::Picture,
                html: Some(node.clone()),
            });
        }
        Video | Audio => {
            let start = inline.text.len();
            inline.text.push(lmdv_md::OBJECT_CHAR);
            inline.objects.push(InlineObject {
                range: start..inline.text.len(),
                kind: if node.tag == Video { InlineObjectKind::Video } else { InlineObjectKind::Audio },
                html: Some(node.clone()),
            });
        }
        Details => {
            let start = inline.text.len();
            inline.text.push(lmdv_md::OBJECT_CHAR);
            inline.objects.push(InlineObject {
                range: start..inline.text.len(),
                kind: InlineObjectKind::Details,
                html: Some(node.clone()),
            });
        }
        A => {
            for child in &node.children {
                push_child(inline, child, stack);
            }
            let end = inline.text.len();
            if end > start {
                let dest = node.attr("href").unwrap_or_default().to_owned();
                inline.spans.push(InlineSpan { range: start..end, style: InlineStyle::Link });
                inline.links.push(InlineLink { range: start..end, dest, title: node.attr("title").map(str::to_owned) });
            }
        }
        // L-12: an image map and its areas are metadata drawn by `html_image`;
        // inline they contribute no text.
        Map | Area => {}
        _ => {
            if let Some(style) = style_of(node.tag) {
                stack.push(style);
            }
            for child in &node.children {
                push_child(inline, child, stack);
            }
            if let Some(style) = style_of(node.tag) {
                stack.pop();
                let end = inline.text.len();
                if end > start {
                    inline.spans.push(InlineSpan { range: start..end, style });
                }
            }
        }
    }
}

fn push_child(inline: &mut Inline, child: &HtmlChild, stack: &[InlineStyle]) {
    match child {
        HtmlChild::Text(text) => push_text(inline, text),
        HtmlChild::Node(node) => {
            let mut stack = stack.to_vec();
            push_node(inline, node, &mut stack);
        }
    }
}

fn push_text(inline: &mut Inline, text: &str) {
    if !text.is_empty() {
        inline.text.push_str(text);
    }
}

fn style_of(tag: HtmlTag) -> Option<InlineStyle> {
    use HtmlTag::*;
    Some(match tag {
        Strong | B => InlineStyle::Strong,
        Em | I => InlineStyle::Em,
        Del | S => InlineStyle::Strike,
        Code => InlineStyle::Code,
        Mark => InlineStyle::Mark,
        Sub => InlineStyle::Sub,
        Sup => InlineStyle::Sup,
        Kbd => InlineStyle::Kbd,
        U | Ins => InlineStyle::Underline,
        _ => return None,
    })
}

fn flush_inline(renderer: &Renderer<'_>, pending: &mut Inline, blocks: &mut Vec<El>) {
    if pending.text.trim().is_empty() && pending.objects.is_empty() {
        *pending = Inline::default();
        return;
    }
    let inline = std::mem::take(pending);
    blocks.push(renderer.inline(&inline, InlineOptions::new(renderer.theme.body, renderer.theme.text)));
}

fn collect_rows<'a>(node: &'a HtmlNode, rows: &mut Vec<Vec<&'a HtmlNode>>) {
    match node.tag {
        HtmlTag::Tr => {
            let cells = node
                .children
                .iter()
                .filter_map(|child| match child {
                    HtmlChild::Node(cell) if matches!(cell.tag, HtmlTag::Td | HtmlTag::Th) => Some(cell),
                    _ => None,
                })
                .collect();
            rows.push(cells);
        }
        _ => {
            for child in &node.children {
                if let HtmlChild::Node(child) = child {
                    collect_rows(child, rows);
                }
            }
        }
    }
}

/// A stable hash of a subtree's tag, attributes and text, for state keys that
/// must survive a re-render but keep two sibling cards apart.
fn node_hash(node: &HtmlNode) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    fn walk(node: &HtmlNode, h: &mut std::collections::hash_map::DefaultHasher) {
        node.tag.name().hash(h);
        for (key, value) in &node.attrs {
            key.hash(h);
            value.hash(h);
        }
        for child in &node.children {
            match child {
                HtmlChild::Text(text) => text.hash(h),
                HtmlChild::Node(node) => walk(node, h),
            }
        }
    }
    walk(node, &mut h);
    h.finish()
}

/// An element's source: `src`, or the first `srcset` candidate when it has no
/// `src` (as `<source>` usually is).
fn node_src(node: &HtmlNode) -> Option<&str> {
    node.attr("src").filter(|src| !src.is_empty()).or_else(|| node.srcset.first().map(|c| c.url.as_str()))
}

fn html_align(attr: Option<&str>) -> Align {
    match attr.map(str::to_ascii_lowercase).as_deref() {
        Some("center") => Align::Center,
        Some("right") => Align::Right,
        _ => Align::None,
    }
}

/// Every `<map>` in a fragment, in document order (L-12).
fn collect_maps(node: &HtmlNode, maps: &mut Vec<HtmlNode>) {
    if node.tag == HtmlTag::Map {
        maps.push(node.clone());
    }
    for child in &node.children {
        if let HtmlChild::Node(child) = child {
            collect_maps(child, maps);
        }
    }
}

/// Marker the parser writes on SVG shape nodes. lmdv-md keeps the name
/// internal so it cannot collide with an HTML attribute in the safe subset.
const SVG_SHAPE_ATTR: &str = "data-lmdv-shape";

/// The drawn size of an `<svg>`: its `width`/`height`, or the viewBox size, or
/// SVG's 300×150 default, all bounded so a document cannot reserve a huge box.
fn svg_size(node: &HtmlNode) -> [f32; 2] {
    let view = svg_view_box(node.attr("viewbox"));
    let width = svg_dimension(node.attr("width"), view.map(|box_| box_[2]).unwrap_or(300.0));
    let height = svg_dimension(node.attr("height"), view.map(|box_| box_[3]).unwrap_or(150.0));
    [width.clamp(1.0, 4096.0), height.clamp(1.0, 4096.0)]
}

fn svg_dimension(value: Option<&str>, default: f32) -> f32 {
    value
        .and_then(|value| value.trim().trim_end_matches("px").parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(default)
}

fn svg_view_box(value: Option<&str>) -> Option<[f32; 4]> {
    let numbers = svg_numbers(value?, 4)?;
    let box_: [f32; 4] = numbers.try_into().ok()?;
    (box_[2] > 0.0 && box_[3] > 0.0).then_some(box_)
}

fn svg_numbers(value: &str, cap: usize) -> Option<Vec<f32>> {
    let mut out = Vec::new();
    for part in value.split(|ch: char| ch == ',' || ch.is_whitespace()) {
        if part.is_empty() {
            continue;
        }
        let Ok(number) = part.parse::<f32>() else { continue };
        if !number.is_finite() {
            continue;
        }
        out.push(number.clamp(-65535.0, 65535.0));
        if out.len() == cap {
            break;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A coordinate may be negative; a length may not.
fn svg_value(value: Option<&str>, default: f32) -> f32 {
    value
        .and_then(|value| value.trim().trim_end_matches("px").parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(-65535.0, 65535.0))
        .unwrap_or(default)
}

fn svg_num(node: &HtmlNode, name: &str, default: f32) -> f32 {
    svg_value(node.attr(name), default).max(0.0)
}

fn svg_coord(node: &HtmlNode, name: &str, default: f32) -> f32 {
    svg_value(node.attr(name), default)
}

/// Build every supported shape in an SVG subtree, in document order.
pub(crate) fn svg_shape(node: &HtmlNode, text: Color) -> Shape {
    let mut shape = Shape::new();
    if let Some(box_) = svg_view_box(node.attr("viewbox")) {
        shape = shape.view_box(box_[2], box_[3]);
    }
    let matrix = SvgMatrix::parse(node.attr("transform"));
    for child in &node.children {
        if let HtmlChild::Node(child) = child {
            svg_shape_node(child, matrix, text, &mut shape, 0);
        }
    }
    shape
}

fn svg_shape_node(node: &HtmlNode, parent: SvgMatrix, text: Color, shape: &mut Shape, depth: usize) {
    if depth > 12 {
        return;
    }
    let matrix = parent.compose(SvgMatrix::parse(node.attr("transform")));
    let current = svg_paint(node.attr("color"), Color::BLACK, text, 1.0).unwrap_or(text);
    let Some(name) = node.attr(SVG_SHAPE_ATTR) else {
        // An `<a>` or another container: its shapes still draw, and its link
        // runs stay in the text below the drawing for clicks (L-12).
        for child in &node.children {
            if let HtmlChild::Node(child) = child {
                svg_shape_node(child, matrix, current, shape, depth + 1);
            }
        }
        return;
    };
    if name == "g" {
        for child in &node.children {
            if let HtmlChild::Node(child) = child {
                svg_shape_node(child, matrix, current, shape, depth + 1);
            }
        }
        return;
    }
    let opacity = svg_num(node, "opacity", 1.0).min(1.0);
    let fill = if matches!(name, "line" | "polyline") {
        None
    } else {
        svg_paint(
            node.attr("fill").or(Some("black")),
            Color::BLACK,
            current,
            opacity * svg_num(node, "fill-opacity", 1.0),
        )
    };
    let stroke = svg_paint(node.attr("stroke"), Color::BLACK, current, opacity * svg_num(node, "stroke-opacity", 1.0));
    let width = svg_num(node, "stroke-width", 1.0).min(128.0);
    let cap = if node.attr("stroke-linecap").is_some_and(|value| value.eq_ignore_ascii_case("butt")) {
        LineCap::Butt
    } else {
        LineCap::Round
    };
    let rule = if node.attr("fill-rule").is_some_and(|value| value.eq_ignore_ascii_case("evenodd")) {
        FillRule::EvenOdd
    } else {
        FillRule::NonZero
    };
    match name {
        "rect" => {
            let (x, y) = (svg_coord(node, "x", 0.0), svg_coord(node, "y", 0.0));
            let (w, h) = (svg_num(node, "width", 0.0), svg_num(node, "height", 0.0));
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let corners = rect_points(x, y, w, h, matrix);
            if let Some(color) = fill {
                let rx = svg_num(node, "rx", 0.0).min(w / 2.0);
                let ry = svg_num(node, "ry", rx).min(h / 2.0);
                *shape = if matrix.is_identity() {
                    if rx > 0.0 && ry > 0.0 {
                        shape.clone().rounded_rect_corners([x, y, w, h], [rx, ry, rx, ry], color)
                    } else {
                        // An axis-aligned box snaps to device pixels like a
                        // native fill, so thin bars and gaps stay crisp.
                        shape.clone().rect([x, y, w, h], color)
                    }
                } else {
                    shape.clone().fill(corners.clone(), color)
                };
            }
            if let Some(color) = stroke {
                *shape = shape.clone().stroke_with(corners, true, width, cap, color);
            }
        }
        "circle" | "ellipse" => {
            let cx = svg_coord(node, "cx", 0.0);
            let cy = svg_coord(node, "cy", 0.0);
            let rx = svg_num(node, if name == "circle" { "r" } else { "rx" }, 0.0);
            let ry = if name == "circle" { rx } else { svg_num(node, "ry", rx) };
            if rx <= 0.0 || ry <= 0.0 {
                return;
            }
            let points = ellipse_points(cx, cy, rx, ry, matrix);
            if let Some(color) = fill {
                *shape = shape.clone().fill(points.clone(), color);
            }
            if let Some(color) = stroke {
                *shape = shape.clone().stroke_with(points, true, width, cap, color);
            }
        }
        "line" => {
            let a = matrix.apply([svg_coord(node, "x1", 0.0), svg_coord(node, "y1", 0.0)]);
            let b = matrix.apply([svg_coord(node, "x2", 0.0), svg_coord(node, "y2", 0.0)]);
            if let Some(color) = stroke {
                *shape = shape.clone().line(a, b, width, cap, color);
            }
        }
        "polyline" | "polygon" => {
            let Some(points) = svg_numbers(node.attr("points").unwrap_or_default(), 4096) else { return };
            let points: Vec<[f32; 2]> =
                points.as_chunks::<2>().0.iter().map(|pair| matrix.apply([pair[0], pair[1]])).collect();
            if name == "polygon"
                && let Some(color) = fill
            {
                *shape = shape.clone().fill(points.clone(), color);
            }
            if let Some(color) = stroke {
                *shape = shape.clone().stroke_with(points, name == "polygon", width, cap, color);
            }
        }
        "path" => {
            let d = node.attr("d").unwrap_or_default();
            if matrix.is_identity() {
                if let Some(color) = fill {
                    *shape = shape.clone().fill_svg_with(d, rule, color);
                }
                if let Some(color) = stroke {
                    *shape = shape.clone().stroke_svg(d, width, cap, color);
                }
            } else {
                let mut sink = TransformedPath::new(matrix);
                parse_svg_path(d, &mut sink);
                if let Some(color) = fill {
                    *shape = shape.clone().fill_path_with(&sink.path, rule, color);
                }
                if let Some(color) = stroke {
                    *shape = shape.clone().stroke_path(&sink.path, width, cap, color);
                }
            }
        }
        _ => {}
    }
}

/// A paint from an SVG colour attribute. `none`/`transparent` draw nothing;
/// `currentColor` and unrecognised spellings fall back to the document text
/// colour, so a document cannot fail to draw because of a CSS word we lack.
fn svg_paint(attr: Option<&str>, fallback: Color, current: Color, opacity: f32) -> Option<Color> {
    let value = attr?.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") || value.eq_ignore_ascii_case("transparent") {
        return None;
    }
    let base = if value.eq_ignore_ascii_case("currentcolor") {
        current
    } else if let Some(color) = rune::color::hex(value) {
        color
    } else if let Some(inner) = value.strip_prefix("rgb(").and_then(|rest| rest.strip_suffix(')')) {
        let channels: Vec<f32> = inner
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .filter_map(|part| part.trim_end_matches('%').parse::<f32>().ok())
            .take(3)
            .collect();
        if channels.len() == 3 && channels.iter().all(|value| (0.0..=255.0).contains(value)) {
            Color {
                r: rune::color::srgb_decode(channels[0] / 255.0),
                g: rune::color::srgb_decode(channels[1] / 255.0),
                b: rune::color::srgb_decode(channels[2] / 255.0),
                a: 1.0,
            }
        } else {
            fallback
        }
    } else {
        match value.to_ascii_lowercase().as_str() {
            "black" => Color::BLACK,
            "white" => Color::WHITE,
            "red" => Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 },
            "green" => Color { r: 0.0, g: 0.5, b: 0.0, a: 1.0 },
            "blue" => Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 },
            "gray" | "grey" => Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 },
            "yellow" => Color { r: 1.0, g: 1.0, b: 0.0, a: 1.0 },
            "orange" => Color { r: 1.0, g: 0.65, b: 0.0, a: 1.0 },
            "purple" => Color { r: 0.5, g: 0.0, b: 0.5, a: 1.0 },
            "cyan" | "aqua" => Color { r: 0.0, g: 1.0, b: 1.0, a: 1.0 },
            "magenta" | "fuchsia" => Color { r: 1.0, g: 0.0, b: 1.0, a: 1.0 },
            _ => fallback,
        }
    };
    Some(base.alpha((base.a * opacity.clamp(0.0, 1.0)).min(1.0)))
}

fn rect_points(x: f32, y: f32, width: f32, height: f32, matrix: SvgMatrix) -> Vec<[f32; 2]> {
    vec![
        matrix.apply([x, y]),
        matrix.apply([x + width, y]),
        matrix.apply([x + width, y + height]),
        matrix.apply([x, y + height]),
    ]
}

fn ellipse_points(cx: f32, cy: f32, rx: f32, ry: f32, matrix: SvgMatrix) -> Vec<[f32; 2]> {
    let steps = ((rx.max(ry) / 2.0).sqrt() * 12.0).ceil().clamp(12.0, 72.0) as usize;
    (0..steps)
        .map(|index| {
            let angle = TAU * index as f32 / steps as f32;
            matrix.apply([cx + rx * angle.cos(), cy + ry * angle.sin()])
        })
        .collect()
}

/// The SVG transform subset: `matrix`, `translate`, `scale`, `rotate` and the
/// skews, composed in attribute order into one affine matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SvgMatrix {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl SvgMatrix {
    const IDENTITY: Self = Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    fn is_identity(self) -> bool {
        self == Self::IDENTITY
    }

    fn apply(self, point: [f32; 2]) -> [f32; 2] {
        [self.a * point[0] + self.c * point[1] + self.e, self.b * point[0] + self.d * point[1] + self.f]
    }

    /// `outer` after `inner`: the parent's transform wraps the child's.
    fn compose(self, inner: Self) -> Self {
        Self {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            e: self.a * inner.e + self.c * inner.f + self.e,
            f: self.b * inner.e + self.d * inner.f + self.f,
        }
    }

    fn parse(attr: Option<&str>) -> Self {
        let Some(attr) = attr else { return Self::IDENTITY };
        let bytes = attr.as_bytes();
        let mut result = Self::IDENTITY;
        let mut index = 0usize;
        while index < bytes.len().min(4096) {
            while index < bytes.len() && matches!(bytes[index], b' ' | b'\t' | b'\n' | b'\r' | b',') {
                index += 1;
            }
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
                index += 1;
            }
            if start == index || bytes.get(index) != Some(&b'(') {
                break;
            }
            let name = &attr[start..index];
            index += 1;
            let args_start = index;
            let mut depth = 1usize;
            while index < bytes.len() && depth > 0 {
                match bytes[index] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                index += 1;
            }
            let args = &attr[args_start..index.saturating_sub(1)];
            let numbers = svg_numbers(args, 8).unwrap_or_default();
            result = result.compose(Self::function(name, &numbers));
        }
        result
    }

    fn function(name: &str, n: &[f32]) -> Self {
        let at = |index: usize, default: f32| n.get(index).copied().unwrap_or(default);
        match name.to_ascii_lowercase().as_str() {
            "matrix" if n.len() >= 6 => {
                Self { a: at(0, 1.0), b: at(1, 0.0), c: at(2, 0.0), d: at(3, 1.0), e: at(4, 0.0), f: at(5, 0.0) }
            }
            "translate" => Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: at(0, 0.0), f: at(1, 0.0) },
            "scale" => {
                Self { a: at(0, 1.0), b: 0.0, c: 0.0, d: n.get(1).copied().unwrap_or(at(0, 1.0)), e: 0.0, f: 0.0 }
            }
            "rotate" if !n.is_empty() => {
                let radians = at(0, 0.0).to_radians();
                let (sin, cos) = radians.sin_cos();
                let rotation = Self { a: cos, b: sin, c: -sin, d: cos, e: 0.0, f: 0.0 };
                if n.len() >= 3 {
                    Self::translate(at(1, 0.0), at(2, 0.0))
                        .compose(rotation)
                        .compose(Self::translate(-at(1, 0.0), -at(2, 0.0)))
                } else {
                    rotation
                }
            }
            "skewx" => Self { a: 1.0, b: 0.0, c: at(0, 0.0).to_radians().tan(), d: 1.0, e: 0.0, f: 0.0 },
            "skewy" => Self { a: 1.0, b: at(0, 0.0).to_radians().tan(), c: 0.0, d: 1.0, e: 0.0, f: 0.0 },
            _ => Self::IDENTITY,
        }
    }

    fn translate(x: f32, y: f32) -> Self {
        Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: x, f: y }
    }
}

/// A path sink that applies a group transform to every point Rune's SVG path
/// parser emits. Arcs are sampled into short straight segments.
struct TransformedPath {
    path: Path,
    matrix: SvgMatrix,
    started: bool,
}

impl TransformedPath {
    fn new(matrix: SvgMatrix) -> Self {
        Self { path: Path::new(), matrix, started: false }
    }
}

impl SvgPathSink for TransformedPath {
    fn is_empty(&self) -> bool {
        !self.started
    }

    fn move_to(&mut self, point: [f32; 2]) {
        self.started = true;
        let path = std::mem::take(&mut self.path);
        self.path = path.move_to(self.matrix.apply(point));
    }

    fn line_to(&mut self, point: [f32; 2]) {
        let path = std::mem::take(&mut self.path);
        self.path = path.line_to(self.matrix.apply(point));
    }

    fn quad_to(&mut self, control: [f32; 2], point: [f32; 2]) {
        let path = std::mem::take(&mut self.path);
        self.path = path.quad_to(self.matrix.apply(control), self.matrix.apply(point));
    }

    fn curve_to(&mut self, first: [f32; 2], second: [f32; 2], point: [f32; 2]) {
        let path = std::mem::take(&mut self.path);
        self.path = path.cubic_to(self.matrix.apply(first), self.matrix.apply(second), self.matrix.apply(point));
    }

    #[allow(clippy::too_many_arguments)]
    fn arc_to(&mut self, from: [f32; 2], rx: f32, ry: f32, angle: f32, large: bool, sweep: bool, to: [f32; 2]) {
        for point in svg_arc_points(from, rx, ry, angle, large, sweep, to) {
            let path = std::mem::take(&mut self.path);
            self.path = path.line_to(self.matrix.apply(point));
        }
    }

    fn close(&mut self) {
        let path = std::mem::take(&mut self.path);
        self.path = path.close();
    }
}

/// Endpoint-to-centre arc parameterisation, sampled into points (the SVG spec's
/// algorithm, with radii scaled up when they cannot span the endpoints).
fn svg_arc_points(
    from: [f32; 2],
    rx: f32,
    ry: f32,
    angle: f32,
    large: bool,
    sweep: bool,
    to: [f32; 2],
) -> Vec<[f32; 2]> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx <= 0.0 || ry <= 0.0 || from == to {
        return vec![to];
    }
    let phi = angle.to_radians();
    let (cos, sin) = phi.sin_cos();
    let (dx, dy) = ((from[0] - to[0]) / 2.0, (from[1] - to[1]) / 2.0);
    let (x1, y1) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    let (mut rx2, mut ry2) = (rx * rx, ry * ry);
    let lambda = x1 * x1 / rx2 + y1 * y1 / ry2;
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
        (rx2, ry2) = (rx * rx, ry * ry);
    }
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let numerator = (rx2 * ry2 - rx2 * y1 * y1 - ry2 * x1 * x1).max(0.0);
    let denominator = (rx2 * y1 * y1 + ry2 * x1 * x1).max(1e-6);
    let coefficient = sign * (numerator / denominator).sqrt();
    let (cx1, cy1) = (coefficient * rx * y1 / ry, coefficient * -ry * x1 / rx);
    let cx = cos * cx1 - sin * cy1 + (from[0] + to[0]) / 2.0;
    let cy = sin * cx1 + cos * cy1 + (from[1] + to[1]) / 2.0;
    let start = ((y1 - cy1) / ry).atan2((x1 - cx1) / rx);
    let mut delta = ((-y1 - cy1) / ry).atan2((-x1 - cx1) / rx) - start;
    if sweep && delta < 0.0 {
        delta += TAU;
    } else if !sweep && delta > 0.0 {
        delta -= TAU;
    }
    let steps = (delta.abs() / (FRAC_PI_2 / 4.0)).ceil().clamp(2.0, 64.0) as usize;
    (1..=steps)
        .map(|index| {
            let theta = start + delta * index as f32 / steps as f32;
            let (cos_t, sin_t) = theta.sin_cos();
            [cos * rx * cos_t - sin * ry * sin_t + cx, sin * rx * cos_t + cos * ry * sin_t + cy]
        })
        .collect()
}

/// An `<area>`'s shape from its `shape` attribute (L-12).
fn area_shape(attr: Option<&str>) -> crate::AreaShape {
    match attr.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("rect") | Some("rectangle") => crate::AreaShape::Rect,
        Some("circle") | Some("circ") => crate::AreaShape::Circle,
        Some("poly") | Some("polygon") => crate::AreaShape::Poly,
        _ => crate::AreaShape::Default,
    }
}

/// An `<area>`'s `coords`, parsed as the comma-separated numbers HTML allows.
fn area_coords(attr: Option<&str>) -> Vec<f32> {
    attr.unwrap_or_default()
        .split(',')
        .filter_map(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .collect()
}

/// The drawn hit box of one `<area>` (L-12).
///
/// `coords` are in the image's natural pixels and scale with it. `rect` and
/// `default` are exact; `circle` and `poly` return the box around their points.
/// A malformed rect/circle/poly falls back to the whole picture rather than a
/// zero-size box.
fn area_hit_box(shape: crate::AreaShape, coords: &[f32], natural: [f32; 2], width: f32, height: f32) -> [f32; 4] {
    let scale_x = width / natural[0].max(1.0);
    let scale_y = height / natural[1].max(1.0);
    let clamp_x = |value: f32| value.clamp(0.0, width);
    let clamp_y = |value: f32| value.clamp(0.0, height);
    let boxed = |left: f32, top: f32, right: f32, bottom: f32| {
        let (left, right) = (clamp_x(left.min(right)), clamp_x(left.max(right)));
        let (top, bottom) = (clamp_y(top.min(bottom)), clamp_y(top.max(bottom)));
        [left, top, (right - left).max(0.0), (bottom - top).max(0.0)]
    };
    match shape {
        crate::AreaShape::Rect if coords.len() >= 4 => {
            boxed(coords[0] * scale_x, coords[1] * scale_y, coords[2] * scale_x, coords[3] * scale_y)
        }
        crate::AreaShape::Circle if coords.len() >= 3 => {
            let (cx, cy, radius) = (coords[0] * scale_x, coords[1] * scale_y, coords[2].abs());
            let radius_x = radius * scale_x;
            let radius_y = radius * scale_y;
            boxed(cx - radius_x, cy - radius_y, cx + radius_x, cy + radius_y)
        }
        crate::AreaShape::Poly if coords.len() >= 4 => {
            let mut left = f32::INFINITY;
            let mut top = f32::INFINITY;
            let mut right = f32::NEG_INFINITY;
            let mut bottom = f32::NEG_INFINITY;
            for pair in coords.as_chunks::<2>().0 {
                left = left.min(pair[0] * scale_x);
                top = top.min(pair[1] * scale_y);
                right = right.max(pair[0] * scale_x);
                bottom = bottom.max(pair[1] * scale_y);
            }
            boxed(left, top, right, bottom)
        }
        _ => [0.0, 0.0, width, height],
    }
}
