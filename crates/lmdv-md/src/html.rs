//! GitHub's tagfilter and the allowed HTML subset.
//!
//! Raw HTML is parsed with html5ever into a throwaway tree, then converted to
//! [`HtmlNode`] with an allowlist: unknown tags keep their children as text,
//! `script`/`style`/`iframe`/`object`/`embed`/`noscript` drop the element and
//! its content, and no attribute outside the security list survives.

use std::borrow::Cow;

use html5ever::interface::QualName;
use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, local_name, ns, parse_fragment};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::model::{HtmlChild, HtmlNode, HtmlTag, ImageBlock, PictureSource, SrcSetCandidate};

/// GitHub's tagfilter set: these tags change how the HTML parser reads the
/// rest of the page, so they are escaped to text instead (M-21).
const FILTERED: &[&str] =
    &["title", "textarea", "style", "xmp", "iframe", "noembed", "noframes", "script", "plaintext", "noscript"];

/// Elements whose content is dropped entirely (§5.4).
const DROPPED: &[&str] = &["script", "style", "iframe", "object", "embed", "noscript", "template"];

/// SVG elements mapped to a native `Shape` (L-12 / OI-195). They are kept in
/// the allowed tree as a marked `<span>` because the frozen `HtmlTag` enum has
/// no geometry variants; the marker is an internal attribute, never a URL.
const SVG_SHAPES: &[&str] = &["rect", "circle", "ellipse", "line", "polyline", "polygon", "path", "g"];
pub(crate) const SVG_SHAPE_ATTR: &str = "data-lmdv-shape";

/// Geometry and presentation attributes that may survive on an SVG shape.
/// Event handlers, URLs other than `href`/`xlink:href`, and arbitrary style are
/// still refused; long path/point lists are capped below.
const SVG_ATTRS: &[&str] = &[
    "x",
    "y",
    "width",
    "height",
    "rx",
    "ry",
    "cx",
    "cy",
    "r",
    "x1",
    "y1",
    "x2",
    "y2",
    "points",
    "d",
    "transform",
    "fill",
    "stroke",
    "stroke-width",
    "fill-rule",
    "opacity",
    "fill-opacity",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-dasharray",
    "viewbox",
    "preserveaspectratio",
    "color",
];

/// Escape the `<` of GitHub-filtered tags.
pub(crate) fn tagfilter(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let mut name_start = index + 1;
        if bytes.get(name_start) == Some(&b'/') {
            name_start += 1;
        }
        let mut name_end = name_start;
        while name_end < bytes.len() && bytes[name_end].is_ascii_alphabetic() {
            name_end += 1;
        }
        let name = &text[name_start..name_end];
        let terminated =
            bytes.get(name_end).is_none_or(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'));
        if !name.is_empty() && terminated && FILTERED.iter().any(|tag| tag.eq_ignore_ascii_case(name)) {
            let target = out.get_or_insert_with(|| String::with_capacity(text.len() + 16));
            target.push_str(&text[copied..index]);
            target.push_str("&lt;");
            copied = index + 1;
        }
        index = name_end.max(index + 1);
    }
    match out {
        Some(mut target) => {
            target.push_str(&text[copied..]);
            Cow::Owned(target)
        }
        None => Cow::Borrowed(text),
    }
}

/// Whether a tag belongs to the tagfilter set.
pub(crate) fn is_filtered_tag(name: &str) -> bool {
    FILTERED.iter().any(|tag| tag.eq_ignore_ascii_case(name))
}

/// Parse a raw HTML block into the allowed subset.
pub(crate) fn parse_block(raw: &str) -> Option<HtmlNode> {
    let filtered = tagfilter(raw);
    let dom = parse_fragment(
        RcDom::default(),
        ParseOpts::default(),
        QualName::new(None, ns!(html), local_name!("div")),
        Vec::new(),
        false,
    )
    .one(filtered.into_owned());
    let children = fragment_children(&dom);
    let converted = convert_children(&children);
    let meaningful = converted.iter().any(|child| match child {
        HtmlChild::Node(_) => true,
        HtmlChild::Text(text) => !text.trim().is_empty(),
    });
    if !meaningful {
        return None;
    }
    Some(HtmlNode::fragment(converted))
}

/// The children parsed inside the fragment's synthetic `html` root.
fn fragment_children(dom: &RcDom) -> Vec<Handle> {
    let document = dom.document.children.borrow();
    let Some(root) = document.first() else { return Vec::new() };
    root.children.borrow().clone()
}

fn convert_children(children: &[Handle]) -> Vec<HtmlChild> {
    convert_children_in(children, false)
}

fn convert_children_in(children: &[Handle], in_svg: bool) -> Vec<HtmlChild> {
    let mut out = Vec::new();
    for child in children {
        out.extend(stacker::maybe_grow(64 * 1024, 2 * 1024 * 1024, || convert_node_in(child, in_svg)));
    }
    out
}

fn convert_node_in(node: &Handle, in_svg: bool) -> Vec<HtmlChild> {
    match &node.data {
        NodeData::Text { contents } => {
            let text = contents.borrow().to_string();
            if text.is_empty() { Vec::new() } else { vec![HtmlChild::Text(text)] }
        }
        NodeData::Element { name, attrs, .. } => {
            let name = name.local.as_ref();
            if DROPPED.iter().any(|tag| tag.eq_ignore_ascii_case(name)) {
                return Vec::new();
            }
            let raw_attrs: Vec<(String, String)> =
                attrs.borrow().iter().map(|attr| (attr.name.local.to_string(), attr.value.to_string())).collect();
            // Inside `<svg>`, shapes keep their geometry and presentation
            // attributes in a marked span. The marker is internal and the same
            // URL policy as every other element still applies to `href`.
            if in_svg && SVG_SHAPES.iter().any(|tag| tag.eq_ignore_ascii_case(&name.to_ascii_lowercase())) {
                let shape = name.to_ascii_lowercase();
                let mut kept = vec![(SVG_SHAPE_ATTR.to_owned(), shape.clone())];
                kept.extend(filter_svg_attrs(raw_attrs));
                let nested = matches!(shape.as_str(), "g" | "a");
                let children = convert_children_in(&node.children.borrow(), nested);
                return vec![HtmlChild::Node(HtmlNode {
                    tag: HtmlTag::Span,
                    attrs: kept,
                    children,
                    srcset: Vec::new(),
                })];
            }
            let Some(tag) = allowed_tag(name) else {
                // Unknown tag: keep its children as text, as GitHub does.
                return convert_children_in(&node.children.borrow(), in_svg);
            };
            let (attrs, srcset) = filter_attrs(tag, raw_attrs);
            let children = convert_children_in(&node.children.borrow(), in_svg || tag == HtmlTag::Svg);
            vec![HtmlChild::Node(HtmlNode { tag, attrs, children, srcset })]
        }
        _ => Vec::new(),
    }
}

/// The SVG subset's attribute filter. `href` follows the ordinary safe-URL
/// policy, numbers and colours are bounded, and everything else is refused.
fn filter_svg_attrs(attrs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut kept = Vec::new();
    for (name, value) in attrs {
        let name = name.to_ascii_lowercase();
        if name == "href" || name == "xlink:href" {
            let name = if name == "xlink:href" { "href".to_owned() } else { name };
            if let Some(url) = sanitize_url(HtmlTag::Svg, &name, &value) {
                kept.push((name, url));
            }
            continue;
        }
        if !SVG_ATTRS.contains(&name.as_str()) {
            continue;
        }
        let cap = if matches!(name.as_str(), "d" | "points") { 4096 } else { 128 };
        if value.len() > cap {
            continue;
        }
        kept.push((name, value));
    }
    kept
}

/// Map a tag name to the allowlist.
pub(crate) fn allowed_tag(name: &str) -> Option<HtmlTag> {
    Some(match name.to_ascii_lowercase().as_str() {
        "a" => HtmlTag::A,
        "abbr" => HtmlTag::Abbr,
        "area" => HtmlTag::Area,
        "b" => HtmlTag::B,
        "blockquote" => HtmlTag::Blockquote,
        "br" => HtmlTag::Br,
        "code" => HtmlTag::Code,
        "del" => HtmlTag::Del,
        "details" => HtmlTag::Details,
        "div" => HtmlTag::Div,
        "em" => HtmlTag::Em,
        "i" => HtmlTag::I,
        "img" => HtmlTag::Img,
        "ins" => HtmlTag::Ins,
        "kbd" => HtmlTag::Kbd,
        "li" => HtmlTag::Li,
        "mark" => HtmlTag::Mark,
        "map" => HtmlTag::Map,
        "ol" => HtmlTag::Ol,
        "p" => HtmlTag::P,
        "picture" => HtmlTag::Picture,
        "pre" => HtmlTag::Pre,
        "s" => HtmlTag::S,
        "source" => HtmlTag::Source,
        "span" => HtmlTag::Span,
        "strong" => HtmlTag::Strong,
        "sub" => HtmlTag::Sub,
        "summary" => HtmlTag::Summary,
        "sup" => HtmlTag::Sup,
        "svg" => HtmlTag::Svg,
        "table" => HtmlTag::Table,
        "tbody" => HtmlTag::Tbody,
        "td" => HtmlTag::Td,
        "tfoot" => HtmlTag::Tfoot,
        "th" => HtmlTag::Th,
        "thead" => HtmlTag::Thead,
        "tr" => HtmlTag::Tr,
        "u" => HtmlTag::U,
        "ul" => HtmlTag::Ul,
        "video" => HtmlTag::Video,
        "audio" => HtmlTag::Audio,
        "track" => HtmlTag::Track,
        _ => return None,
    })
}

/// Apply the attribute allowlist and pull `srcset` out into candidates.
pub(crate) fn filter_attrs(
    tag: HtmlTag,
    attrs: impl IntoIterator<Item = (String, String)>,
) -> (Vec<(String, String)>, Vec<SrcSetCandidate>) {
    const ALLOWED: &[&str] = &[
        "href",
        "src",
        "alt",
        "title",
        "width",
        "height",
        "media",
        "type",
        "start",
        "colspan",
        "rowspan",
        "align",
        "poster",
        "controls",
        "loop",
        "muted",
        "preload",
        "kind",
        "srclang",
        "label",
        "id",
        "class",
        "usemap",
        "name",
        "shape",
        "coords",
        "open",
        "viewbox",
        "preserveaspectratio",
    ];
    let mut kept = Vec::new();
    let mut srcset = Vec::new();
    for (name, value) in attrs {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "srcset" if matches!(tag, HtmlTag::Img | HtmlTag::Source) => {
                // Defense in depth: resolution refuses unsafe candidates too,
                // but none may outlive the attribute filter (OI-146). The
                // descriptor and candidate shape stay as written.
                srcset = parse_srcset(&value)
                    .into_iter()
                    .filter_map(|candidate| {
                        // M-26: a network or device candidate is off limits and is
                        // dropped here, not left for the resolver. A local spelling
                        // (including `///abs`) stays as written.
                        if is_off_limits_url(&candidate.url) {
                            return None;
                        }
                        let url = sanitize_url(tag, "src", &candidate.url)?;
                        Some(SrcSetCandidate { url, descriptor: candidate.descriptor })
                    })
                    .collect();
            }
            "class" => {
                let classes: Vec<&str> = value
                    .split_whitespace()
                    .filter(|class| matches!(*class, "gh-dark-mode-only" | "gh-light-mode-only"))
                    .collect();
                if !classes.is_empty() {
                    kept.push(("class".to_owned(), classes.join(" ")));
                }
            }
            "id" => {
                let value = value.trim();
                if !value.is_empty() {
                    let namespaced = if value.starts_with("user-content-") {
                        value.to_owned()
                    } else {
                        format!("user-content-{value}")
                    };
                    kept.push(("id".to_owned(), namespaced));
                }
            }
            "href" | "src" | "poster" | "xlink:href" => {
                // L-12: SVG's `xlink:href` names the same destination as
                // `href`, so both spellings reach the inline link parser and a
                // future link menu under one name.
                let name = if name == "xlink:href" { "href".to_owned() } else { name };
                if let Some(url) = sanitize_url(tag, &name, &value) {
                    kept.push((name, url));
                }
            }
            _ if ALLOWED.contains(&name.as_str()) => kept.push((name, value)),
            _ => {}
        }
    }
    (kept, srcset)
}

/// Whether a URL names another computer or a device instead of a local path
/// (M-26): `//host/share`, a UNC spelling, or a Windows drive.
///
/// A `///abs` spelling is an ordinary absolute path, as in `links.rs`.
fn is_off_limits_url(url: &str) -> bool {
    if (url.starts_with("//") && !url.starts_with("///")) || url.starts_with(r"\\") {
        return true;
    }
    let bytes = url.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn sanitize_url(tag: HtmlTag, name: &str, value: &str) -> Option<String> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("javascript:") || lower.starts_with("vbscript:") {
        return None;
    }
    if lower.starts_with("data:") {
        let image_slot = matches!(tag, HtmlTag::Img | HtmlTag::Source | HtmlTag::Video | HtmlTag::Audio);
        if !(image_slot && name == "src" && lower.starts_with("data:image/")) {
            return None;
        }
    }
    Some(value.to_owned())
}

/// HTML-spec-like `srcset` candidates: the URL ends at whitespace, trailing
/// commas end a candidate, and commas may appear inside a URL (M-26).
pub(crate) fn parse_srcset(value: &str) -> Vec<SrcSetCandidate> {
    let mut candidates = Vec::new();
    let mut rest = value.trim_start();
    while !rest.is_empty() {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(',') {
            rest = after;
            continue;
        }
        let url_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let mut url = &rest[..url_end];
        let trailing = url.bytes().rev().take_while(|byte| *byte == b',').count();
        if trailing > 0 {
            url = &url[..url.len() - trailing];
            if !url.is_empty() {
                candidates.push(SrcSetCandidate { url: url.to_owned(), descriptor: None });
            }
            rest = &rest[url_end..];
            continue;
        }
        let after = &rest[url_end..];
        match after.find(',') {
            Some(comma) => {
                let descriptor = after[..comma].trim();
                if !url.is_empty() {
                    candidates.push(SrcSetCandidate {
                        url: url.to_owned(),
                        descriptor: (!descriptor.is_empty()).then(|| descriptor.to_owned()),
                    });
                }
                rest = &after[comma + 1..];
            }
            None => {
                let descriptor = after.trim();
                if !url.is_empty() {
                    candidates.push(SrcSetCandidate {
                        url: url.to_owned(),
                        descriptor: (!descriptor.is_empty()).then(|| descriptor.to_owned()),
                    });
                }
                rest = "";
            }
        }
    }
    candidates
}

/// Turn an HTML `<img>` or `<picture>` into the block image model.
pub(crate) fn image_from_html(node: &HtmlNode) -> Option<ImageBlock> {
    let (img, picture) = match node.tag {
        HtmlTag::Img => (node, None),
        HtmlTag::Picture => {
            let mut img = None;
            for child in &node.children {
                if let HtmlChild::Node(node) = child
                    && node.tag == HtmlTag::Img
                {
                    img = Some(node);
                }
            }
            (img?, Some(node))
        }
        _ => return None,
    };
    let mut sources = Vec::new();
    if let Some(picture) = picture {
        for child in &picture.children {
            if let HtmlChild::Node(node) = child
                && node.tag == HtmlTag::Source
            {
                sources.push(PictureSource {
                    media: node.attr("media").map(str::to_owned),
                    mime: node.attr("type").map(str::to_owned),
                    srcset: node.srcset.clone(),
                });
            }
        }
    }
    Some(ImageBlock {
        src: img.attr("src").unwrap_or_default().to_owned(),
        alt: img.attr("alt").unwrap_or_default().to_owned(),
        title: img.attr("title").map(str::to_owned),
        srcset: img.srcset.clone(),
        picture: sources,
        source: 0..0,
        line: 0,
    })
}

/// Whether a raw inline tag belongs to the tagfilter set.
pub(crate) fn is_filtered_tag_name(raw: &str) -> bool {
    let inner = raw.strip_prefix('<').unwrap_or(raw);
    let inner = inner.strip_prefix('/').unwrap_or(inner);
    let name_end = inner.find(|ch: char| !ch.is_ascii_alphanumeric()).unwrap_or(inner.len());
    name_end > 0 && is_filtered_tag(&inner[..name_end])
}

/// One parsed inline HTML tag.
#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedTag {
    /// Lowercase tag name.
    pub name: String,
    /// A closing tag (`</kbd>`).
    pub closing: bool,
    /// Attributes, in source order.
    pub attrs: Vec<(String, String)>,
}

/// Parse one inline HTML tag; `None` for comments, doctypes and garbage.
pub(crate) fn parse_tag(raw: &str) -> Option<ParsedTag> {
    let inner = raw.strip_prefix('<')?.strip_suffix('>')?;
    let inner = inner.trim();
    if inner.is_empty() || inner.starts_with(['!', '?']) {
        return None;
    }
    let (closing, rest) = match inner.strip_prefix('/') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, inner),
    };
    let name_end = rest.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-')).unwrap_or(rest.len());
    if name_end == 0 {
        return None;
    }
    let name = rest[..name_end].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let mut index = name_end;
    let bytes = rest.as_bytes();
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let attr_start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() && !matches!(bytes[index], b'=' | b'>' | b'/')
        {
            index += 1;
        }
        if attr_start == index {
            index += 1;
            continue;
        }
        let attr_name = rest[attr_start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let mut value = String::new();
        if bytes.get(index) == Some(&b'=') {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if let Some(quote) = bytes.get(index).copied().filter(|byte| matches!(byte, b'"' | b'\'')) {
                index += 1;
                let value_start = index;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
                value = decode_entities(&rest[value_start..index]);
                index += usize::from(index < bytes.len());
            } else {
                let value_start = index;
                while index < bytes.len() && !bytes[index].is_ascii_whitespace() && bytes[index] != b'>' {
                    index += 1;
                }
                value = decode_entities(&rest[value_start..index]);
            }
        }
        attrs.push((attr_name, value));
    }
    Some(ParsedTag { name, closing, attrs })
}

fn decode_entities(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        let Some(semicolon) = rest.find(';') else { break };
        let entity = &rest[1..semicolon];
        match decode_entity(entity) {
            Some(decoded) => {
                out.push_str(&decoded);
                rest = &rest[semicolon + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Decode one entity body (the text between `&` and `;`), for attribute
/// values and for the source-map repair of entity-heavy inline runs (OI-025).
/// Unknown names are left for the caller to keep verbatim.
pub(crate) fn decode_entity(entity: &str) -> Option<String> {
    let ch = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "mdash" => '\u{2014}',
        "ndash" => '\u{2013}',
        "hellip" => '\u{2026}',
        "laquo" => '\u{ab}',
        "raquo" => '\u{bb}',
        "cent" => '\u{a2}',
        "pound" => '\u{a3}',
        "yen" => '\u{a5}',
        "euro" => '\u{20ac}',
        "sect" => '\u{a7}',
        "deg" => '\u{b0}',
        "plusmn" => '\u{b1}',
        "times" => '\u{d7}',
        "divide" => '\u{f7}',
        "bull" => '\u{2022}',
        "middot" => '\u{b7}',
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            char::from_u32(u32::from_str_radix(&entity[2..], 16).ok()?)?
        }
        _ if entity.starts_with('#') => char::from_u32(entity[1..].parse::<u32>().ok()?)?,
        _ => return None,
    };
    Some(ch.to_string())
}
