//! The allowed HTML subset (M-21–M-23, M-27, M-29).
//!
//! `lmdv-md` has already applied GitHub's tagfilter and the attribute allowlist
//! and produced a plain [`lmdv_md::HtmlNode`] tree; this module maps that tree
//! to native elements. It is not a browser: there is no CSS engine, and a tag
//! with no native equivalent keeps its children as text.
//!
//! Media is a Linux placeholder (M-27): a card naming the source, with an
//! “Open externally” action, because Rune has no media backend at this rev.

use lmdv_md::{
    Align, HtmlChild, HtmlNode, HtmlTag, ImageBlock, Inline, InlineLink, InlineObject, InlineObjectKind, InlineSpan,
    InlineStyle, SrcSetCandidate, Table,
};
use rune::{El, col, el, row, text};

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
    /// open; the body blocks around it stay ordinary document blocks because a
    /// blank line ends the HTML block in the parser's model.
    fn html_details(&self, node: &HtmlNode, block_key: &str) -> El {
        let key = format!("details:{block_key}:{:x}", node_hash(node));
        let open = self.opts.open_for(&key);
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

    /// An inline `<svg>` fragment (L-12).
    ///
    /// The parser keeps only the safe subset: its `<a>`/`xlink:href` links (both
    /// stored as `href`) and any text, with shapes and scripts dropped. They
    /// draw as one group, so the link runs keep their own clicks; an SVG with
    /// no drawable text stays a labelled placeholder rather than an empty box.
    fn html_svg(&self, node: &HtmlNode, key: &str) -> El {
        let inline = self.inline_from_node(node);
        let label = "SVG".to_owned();
        if inline.text.trim().is_empty() {
            return col().key(format!("{key}-svg")).role("group").label(label).w_full().h(0.0);
        }
        col()
            .key(format!("{key}-svg"))
            .role("group")
            .label(label)
            .w_full()
            .child(self.inline(&inline, InlineOptions::new(self.theme.body, self.theme.text)))
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
