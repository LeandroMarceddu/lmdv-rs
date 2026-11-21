//! The Rune-free document model produced by [`crate::parse`].
//!
//! Everything here is plain data: `Send`, `Sync` and serializable, so the
//! parser can run on a worker thread and goldens can be written as JSON.
//! Positions are UTF-8 byte offsets into the LF-normalized source and lines
//! are 0-based, matching the original viewer's `data-source-line`.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The character the parser writes in place of a formula or an object.
///
/// It is three bytes long (U+FFFC), so replacing the first three bytes of a
/// math span keeps the transformed source byte-identical in length.
pub const OBJECT_CHAR: char = '\u{FFFC}';

/// A parsed Markdown document.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MdDoc {
    /// The original LF-normalized source the document was parsed from.
    pub source: Arc<str>,
    /// The document revision the parse was made from; 0 for [`crate::parse`].
    pub revision: u64,
    /// Top-level blocks, in document order.
    pub blocks: Vec<Block>,
    /// One entry per entry in [`MdDoc::blocks`].
    pub block_spans: Vec<BlockSpan>,
    /// Headings in document order, for the outline, anchors and `[TOC]`.
    pub headings: Vec<Heading>,
    /// Footnote definitions that are referenced, numbered by first reference.
    pub footnotes: Vec<Footnote>,
    /// The leading YAML front matter, when the document has one.
    pub front_matter: Option<FrontMatter>,
    /// Non-fatal problems found while parsing.
    pub diagnostics: Vec<Diagnostic>,
    /// Byte offset of every line start, including line 0 at offset 0.
    pub line_starts: Arc<[u32]>,
}

impl MdDoc {
    /// The 0-based line containing `byte`.
    pub fn line_of(&self, byte: usize) -> usize {
        match self.line_starts.binary_search(&(byte.min(u32::MAX as usize) as u32)) {
            Ok(line) => line,
            Err(next) => next.saturating_sub(1),
        }
    }

    /// The number of lines in the source, including the empty line after a
    /// trailing newline.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The stable key for the top-level block at `index` (`b0`, `b1`, …).
    pub fn block_key(&self, index: usize) -> Option<&str> {
        self.block_spans.get(index).map(|span| span.key.as_str())
    }

    /// Every code block's text, in the order the article draws it (M-13).
    ///
    /// The renderer numbers each code block's copy action by its position
    /// here, so a document may hold any number of code blocks — top level,
    /// nested in a quote or a list item, inside raw HTML's `<pre>`, or in a
    /// footnote definition — and each one copies its own text.
    pub fn code_texts(&self) -> Vec<Cow<'_, str>> {
        let mut texts = Vec::new();
        for block in &self.blocks {
            push_code_texts(block, &mut texts);
        }
        // The generated footnote section is drawn after every block.
        for footnote in &self.footnotes {
            for block in &footnote.blocks {
                push_code_texts(block, &mut texts);
            }
        }
        texts
    }

    /// How many code blocks the article draws before top-level block `index`
    /// (M-13).
    ///
    /// The renderer numbers a block's code blocks from this count, so a
    /// memoised block keeps the destinations it was built with.
    pub fn code_count_before(&self, index: usize) -> usize {
        self.blocks.iter().take(index).map(count_code_texts).sum()
    }

    /// How many code blocks top-level block `index` holds (M-13).
    pub fn code_count_in(&self, index: usize) -> usize {
        self.blocks.get(index).map_or(0, count_code_texts)
    }

    /// The stable key for a heading (`h0`, `h1`, …).
    pub fn heading_key(index: usize) -> String {
        format!("h{index}")
    }

    /// The top-level block containing a source byte, or the last block for
    /// an offset at or past the end. Used by `--line`, click-to-source and
    /// footnote backlinks.
    pub fn block_index_at(&self, source: usize) -> Option<usize> {
        if self.block_spans.is_empty() {
            return None;
        }
        if source >= self.source.len() {
            return Some(self.block_spans.len() - 1);
        }
        let index = self.block_spans.partition_point(|span| span.source.end <= source);
        self.block_spans
            .get(index)
            .filter(|span| span.source.start <= source)
            .map(|_| index)
            .or_else(|| index.checked_sub(1).filter(|index| self.block_spans[*index].source.contains(&source)))
    }

    /// Resolve an in-document fragment (without `#`) to a heading index.
    ///
    /// Current GitHub ids, the pre-1.15.1 legacy ids and the original name of
    /// a renamed reserved id all resolve. A percent-encoded fragment is tried
    /// both as written and decoded, because old links use `%26` and friends.
    pub fn resolve_heading(&self, fragment: &str) -> Option<usize> {
        let fragment = fragment.strip_prefix('#').unwrap_or(fragment);
        if let Some(index) = self.find_heading(fragment) {
            return Some(index);
        }
        let decoded = percent_encoding::percent_decode_str(fragment).decode_utf8_lossy();
        if decoded != fragment { self.find_heading(&decoded) } else { None }
    }

    fn find_heading(&self, fragment: &str) -> Option<usize> {
        self.headings.iter().position(|heading| {
            heading.id == fragment
                || heading.legacy_id.as_deref() == Some(fragment)
                || heading.anchor.as_deref() == Some(fragment)
        })
    }
}

/// The part of a written URL that names the file: the query dropped and the
/// fragment kept (M-24).
///
/// An image's `#gh-dark-mode-only` is a theme hint the renderer reads from the
/// source as written, while `?raw=1` is a fetch hint that has no meaning for a
/// local file. Callers that resolve a path strip the query first and then split
/// the fragment themselves.
pub fn url_path_and_fragment(url: &str) -> (&str, Option<&str>) {
    let (rest, fragment) = match url.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (url, None),
    };
    let path = rest.split_once('?').map_or(rest, |(path, _)| path);
    (path, fragment)
}

/// How many code blocks one block holds, in drawing order (M-13).
fn count_code_texts(block: &Block) -> usize {
    match block {
        Block::Code { .. } => 1,
        Block::List { items, .. } => items.iter().flatten().map(count_code_texts).sum(),
        Block::Quote { blocks, .. } => blocks.iter().map(count_code_texts).sum(),
        Block::HtmlBlock(node) => count_html_code_texts(node),
        _ => 0,
    }
}

/// How many `<pre>` code blocks an HTML subtree draws (M-13).
fn count_html_code_texts(node: &HtmlNode) -> usize {
    match node.tag {
        HtmlTag::Pre => 1,
        HtmlTag::Fragment | HtmlTag::Div | HtmlTag::P => node
            .children
            .iter()
            .filter_map(|child| match child {
                HtmlChild::Node(child) => Some(count_html_code_texts(child)),
                HtmlChild::Text(_) => None,
            })
            .sum(),
        _ => 0,
    }
}

/// Every code block inside `block`, in drawing order (M-13).
fn push_code_texts<'a>(block: &'a Block, texts: &mut Vec<Cow<'a, str>>) {
    match block {
        Block::Code { text, .. } => texts.push(Cow::Borrowed(text)),
        Block::List { items, .. } => {
            for item in items {
                for block in item {
                    push_code_texts(block, texts);
                }
            }
        }
        Block::Quote { blocks, .. } => {
            for block in blocks {
                push_code_texts(block, texts);
            }
        }
        Block::HtmlBlock(node) => push_html_code_texts(node, texts),
        _ => {}
    }
}

/// Every `<pre>` an HTML subtree draws as a code block, in drawing order (M-13).
///
/// The renderer reaches `<pre>` only through a fragment, `div` or `p` whose
/// children are laid out as blocks; a table, list, details card or quote draws
/// its body inline, so a `<pre>` inside one is not a code block and is not
/// counted (which keeps the copy numbering in step with the article).
fn push_html_code_texts<'a>(node: &'a HtmlNode, texts: &mut Vec<Cow<'a, str>>) {
    match node.tag {
        HtmlTag::Pre => texts.push(Cow::Owned(node.text())),
        HtmlTag::Fragment | HtmlTag::Div | HtmlTag::P => {
            for child in &node.children {
                if let HtmlChild::Node(child) = child {
                    push_html_code_texts(child, texts);
                }
            }
        }
        _ => {}
    }
}

/// Where a top-level block lives in the source.
///
/// `line` is inclusive, `end_line` is exclusive; both are 0-based.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockSpan {
    /// A key stable within this document revision: `b{index}`.
    pub key: String,
    /// The source byte range of the block, end exclusive.
    pub source: Range<usize>,
    /// The first source line of the block.
    pub line: usize,
    /// The line after the last source line of the block.
    pub end_line: usize,
}

/// A heading with its GitHub id and the pre-1.15.1 legacy id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heading {
    /// Heading level, 1..=6.
    pub level: u8,
    /// The GitHub slug, deduplicated (`a`, `a-1`, `a-2`, …).
    pub id: String,
    /// The old `markdown-it-anchor` id, when it differs from [`Heading::id`].
    pub legacy_id: Option<String>,
    /// The name the heading was written with when a reserved app id forced
    /// the id to `user-content-<name>` (the original `data-anchor` value).
    pub anchor: Option<String>,
    /// Plain text of the heading, without Markdown markers.
    pub text: String,
    /// Index into [`MdDoc::blocks`] of the heading block.
    pub block: usize,
    /// The 0-based source line of the heading.
    pub line: usize,
    /// Whether the heading belongs in a `[TOC]` (levels 1..=4).
    pub in_toc: bool,
}

impl Heading {
    /// Whether this heading is included in a table of contents.
    pub fn toc_level(&self) -> bool {
        (1..=4).contains(&self.level)
    }
}

/// A footnote definition that at least one reference points at.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footnote {
    /// The label written between `[^` and `]`.
    pub label: String,
    /// 1-based number, assigned by first reference.
    pub number: usize,
    /// The definition's body blocks.
    pub blocks: Vec<Block>,
    /// The source byte range of the whole definition, end exclusive.
    pub source: Range<usize>,
    /// The source range of the first `[^label]` reference, for the backlink.
    pub first_ref: Option<Range<usize>>,
}

/// A parsed block.
///
/// `MathBlock` and `HtmlBlock` are named this way in the frozen model.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Block {
    /// A paragraph of inline content.
    Para(Inline),
    /// A heading; `heading` indexes [`MdDoc::headings`].
    Heading {
        /// Index into [`MdDoc::headings`].
        heading: usize,
        /// The heading's inline content.
        inline: Inline,
    },
    /// An indented or fenced code block.
    Code {
        /// The first word of the fence info string, as written.
        lang: Option<String>,
        /// The code, without the fence and without one trailing newline.
        text: String,
        /// The source byte range, end exclusive.
        source: Range<usize>,
    },
    /// A bullet or ordered list.
    List {
        /// Whether the list is ordered.
        ordered: bool,
        /// The first number of an ordered list (1 for bullet lists).
        start: u64,
        /// Whether the list is tight (no blank lines between its blocks).
        tight: bool,
        /// Each item's blocks.
        items: Vec<Vec<Block>>,
    },
    /// A block quote, optionally a GitHub alert.
    Quote {
        /// The quote body, parsed normally.
        blocks: Vec<Block>,
        /// Set when the quote starts with `[!NOTE]` and friends.
        callout: Option<Callout>,
    },
    /// A GFM table.
    Table(Table),
    /// A thematic break.
    Rule,
    /// A ` ```mermaid ` fence.
    Mermaid {
        /// The diagram source.
        source: String,
        /// The 0-based line of the opening fence.
        line: usize,
    },
    /// Display math, from a `$$` block or a ` ```math ` fence.
    MathBlock {
        /// The TeX source.
        tex: String,
        /// The 0-based source line of the formula.
        line: usize,
    },
    /// Raw HTML converted to the allowed subset.
    HtmlBlock(HtmlNode),
    /// A block-level Markdown image.
    Image(ImageBlock),
    /// The generated footnote section; always the last block when present.
    FootnoteDefs,
    /// A `[TOC]` placeholder; the renderer builds the nav from later headings.
    Toc,
}

/// A parsed table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// One alignment per column.
    pub align: Vec<Align>,
    /// Header cells.
    pub head: Vec<Inline>,
    /// Body rows, each with one inline per column.
    pub rows: Vec<Vec<Inline>>,
    /// The source byte range, end exclusive.
    pub source: Range<usize>,
    /// The 0-based line of the header row.
    pub line: usize,
}

/// Column alignment from the GFM delimiter row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    /// No explicit alignment.
    #[default]
    None,
    /// `:---`
    Left,
    /// `:---:`
    Center,
    /// `---:`
    Right,
}

/// The inline content of a paragraph, heading or table cell.
///
/// [`Inline::text`] is what is drawn: soft line breaks are spaces, hard
/// breaks are `\n`, and every formula, image and task checkbox is one
/// [`OBJECT_CHAR`] (three bytes). The side tables point into `text`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inline {
    /// The rendered text.
    pub text: String,
    /// Styled runs (strong, emphasis, code, links, …).
    pub spans: Vec<InlineSpan>,
    /// Link destinations and their rendered ranges.
    pub links: Vec<InlineLink>,
    /// Formulas, with the range of their placeholder in [`Inline::text`].
    pub math: Vec<InlineMath>,
    /// Images, line breaks and task checkboxes.
    pub objects: Vec<InlineObject>,
    /// Rendered byte to source byte mapping.
    pub source_map: SourceMap,
}

impl Inline {
    /// Whether the inline content draws nothing.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.objects.is_empty()
    }
}

/// A styled run of [`Inline::text`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineSpan {
    /// The byte range in [`Inline::text`], end exclusive.
    pub range: Range<usize>,
    /// The style applied over the range.
    pub style: InlineStyle,
}

/// The styles the parser produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InlineStyle {
    /// No visual style (used for literal fallbacks).
    Plain,
    /// Strong emphasis.
    Strong,
    /// Emphasis.
    Em,
    /// GFM strikethrough.
    Strike,
    /// Inline code.
    Code,
    /// A link.
    Link,
    /// `<mark>`.
    Mark,
    /// `<sub>`.
    Sub,
    /// `<sup>`.
    Sup,
    /// `<kbd>`.
    Kbd,
    /// `<u>` or `<ins>`.
    Underline,
}

/// A link and the rendered text it covers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineLink {
    /// The byte range in [`Inline::text`], end exclusive.
    pub range: Range<usize>,
    /// The destination as written (relative, `#fragment`, `fn:<label>`…).
    pub dest: String,
    /// The optional title.
    pub title: Option<String>,
}

/// A formula found by the math scanner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineMath {
    /// The byte range of the placeholder in [`Inline::text`].
    pub range: Range<usize>,
    /// The TeX source, without delimiters.
    pub tex: String,
    /// Whether the formula is display math (`$$` or a math fence).
    pub display: bool,
}

/// Something that needs special layout in an inline run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineObject {
    /// The byte range in [`Inline::text`] (empty for a line break).
    pub range: Range<usize>,
    /// What the object is.
    pub kind: InlineObjectKind,
    /// The HTML node it came from, when it came from HTML.
    pub html: Option<HtmlNode>,
}

/// The kinds of inline objects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InlineObjectKind {
    /// A hard line break.
    LineBreak,
    /// An image (`![alt](src)` or `<img>`).
    Image,
    /// A task-list checkbox, live in viewer mode.
    TaskCheckbox {
        /// Whether the source marker is `[x]`.
        checked: bool,
    },
    /// An inline `<details>` (rendered as a card when the renderer can).
    Details,
    /// An inline `<picture>`.
    Picture,
    /// An inline `<video>`.
    Video,
    /// An inline `<audio>`.
    Audio,
    /// Any other allowed inline HTML element needing an object.
    Unknown,
}

/// A math span recorded by the scanner, including its delimiters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MathSpan {
    /// The source range that was replaced, delimiters included, end exclusive.
    pub source: Range<usize>,
    /// The TeX source, without delimiters.
    pub tex: String,
    /// Whether the formula is display math.
    pub display: bool,
    /// The 0-based line of the opening delimiter.
    pub line: usize,
}

/// The result of the math pre-pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathScan {
    /// The source with every formula replaced by a same-length placeholder.
    pub text: String,
    /// The spans that were replaced, in source order.
    pub spans: Vec<MathSpan>,
    /// Where each span's placeholder was written. It is normally the span
    /// start; a one-byte sentinel is used when U+FFFC would overwrite a
    /// newline (the rendered text still uses U+FFFC).
    pub placeholders: Vec<usize>,
}

/// Rendered byte to source byte mapping, as monotone linear segments.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMap {
    /// Ordered, non-overlapping segments.
    pub segments: Vec<SourceSegment>,
    /// The source end of the inline this map belongs to, when the parser knows
    /// it. Typographic quote expansion maps one source byte onto three
    /// rendered bytes (`a'` → `a’`), so a linear endpoint can name bytes the
    /// source never had; [`SourceMap::source_range`] stops there. It is not
    /// serialized: a map read back from a golden has no bound and keeps the
    /// plain linear behavior.
    #[serde(skip)]
    source_end: usize,
}

/// One monotone segment: rendered bytes map linearly onto source bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSegment {
    /// The rendered byte range in the inline text.
    pub rendered: Range<usize>,
    /// The source byte the first rendered byte came from.
    pub source: usize,
}

impl SourceMap {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `rendered` came from source offset `source`.
    ///
    /// Adjacent segments with a continuous source are coalesced, so the map
    /// stays small for ordinary text.
    pub fn push(&mut self, rendered: Range<usize>, source: usize) {
        if rendered.is_empty() {
            return;
        }
        if let Some(last) = self.segments.last_mut()
            && last.rendered.end == rendered.start
            && last.source + last.rendered.len() == source
        {
            last.rendered.end = rendered.end;
            return;
        }
        self.segments.push(SourceSegment { rendered, source });
    }

    /// The source byte for a rendered byte, when it is mapped.
    pub fn source_of(&self, rendered: usize) -> Option<usize> {
        let index = self.segment_index(rendered)?;
        let segment = &self.segments[index];
        Some(segment.source + (rendered - segment.rendered.start))
    }

    /// The index of the segment that maps a rendered byte.
    fn segment_index(&self, rendered: usize) -> Option<usize> {
        let index = match self.segments.binary_search_by(|segment| segment.rendered.start.cmp(&rendered)) {
            Ok(index) => index,
            Err(0) => return None,
            Err(next) => next - 1,
        };
        (rendered < self.segments[index].rendered.end).then_some(index)
    }

    /// Record the source end of the inline this map describes.
    pub(crate) fn set_source_end(&mut self, end: usize) {
        self.source_end = end;
    }

    /// The first rendered byte that came from `source`.
    pub fn rendered_of(&self, source: usize) -> Option<usize> {
        self.segments.iter().find_map(|segment| {
            let len = segment.rendered.len();
            (source >= segment.source && source < segment.source + len)
                .then_some(segment.rendered.start + (source - segment.source))
        })
    }

    /// The source range for a rendered byte range, when both ends are mapped.
    pub fn source_range(&self, rendered: Range<usize>) -> Option<Range<usize>> {
        if rendered.is_empty() {
            return self.source_of(rendered.start).map(|source| source..source);
        }
        let start = self.source_of(rendered.start)?;
        let index = self.segment_index(rendered.end - 1)?;
        let end = self.source_of(rendered.end - 1)? + 1;
        // A typographic quote is expanded from one source byte to three
        // rendered bytes, so the linear endpoint can land past the byte the
        // quote came from (and, at the end of the document, past the source).
        // Stop at the segment's source end: the next segment starts there, or
        // the inline's own end when this is the last segment (OI-145).
        let limit = self.segments.get(index + 1).map_or(self.source_end, |next| next.source);
        let end = if limit == 0 { end } else { end.min(limit).max(start) };
        Some(start..end)
    }

    /// Whether nothing is mapped.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

/// A GitHub alert parsed from a block quote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Callout {
    /// The alert kind.
    pub kind: CalloutKind,
    /// A custom title written after the marker; `None` means the localized default.
    pub title: Option<String>,
}

/// The five GitHub alert kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CalloutKind {
    /// `[!NOTE]`
    Note,
    /// `[!TIP]`
    Tip,
    /// `[!IMPORTANT]`
    Important,
    /// `[!WARNING]`
    Warning,
    /// `[!CAUTION]`
    Caution,
}

impl CalloutKind {
    /// The marker word (`NOTE`, `TIP`, …).
    pub fn marker(self) -> &'static str {
        match self {
            Self::Note => "NOTE",
            Self::Tip => "TIP",
            Self::Important => "IMPORTANT",
            Self::Warning => "WARNING",
            Self::Caution => "CAUTION",
        }
    }

    /// The English default title (the renderer localizes it instead).
    pub fn default_title(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }
}

/// The flat YAML front matter of a document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontMatter {
    /// The YAML between the fences, verbatim.
    pub raw: String,
    /// Flat entries, in source order; empty when [`FrontMatter::valid`] is false.
    pub entries: Vec<(String, FmValue)>,
    /// Whether `raw` fit the flat subset (key/value, inline arrays, `-` lists).
    pub valid: bool,
}

impl FrontMatter {
    /// The value of a key, when the front matter is valid.
    pub fn get(&self, key: &str) -> Option<&FmValue> {
        self.entries.iter().find(|(name, _)| name == key).map(|(_, value)| value)
    }
}

/// A front-matter value: a scalar or a flat list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FmValue {
    /// A single value.
    Scalar(String),
    /// An inline `[a, b]` array or a `- item` list.
    List(Vec<String>),
}

/// A non-fatal parse problem.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// How serious the problem is.
    pub severity: DiagnosticSeverity,
    /// A stable machine-readable kind.
    pub code: DiagnosticCode,
    /// A human-readable message (the app may localize by `code`).
    pub message: String,
    /// The 0-based source line, when known.
    pub line: Option<usize>,
    /// The source byte range, when known.
    pub source: Option<Range<usize>>,
}

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    /// Informational.
    Info,
    /// The document still renders, but something is off.
    Warning,
    /// Content could not be represented.
    Error,
}

/// Stable diagnostic kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    /// A footnote reference has no definition.
    UndefinedFootnote,
    /// A footnote label was defined more than once; the last one wins.
    DuplicateFootnote,
    /// An HTML element was dropped by the allowlist.
    UnsupportedHtml,
    /// A math candidate was too short to hold a placeholder.
    MathTooShort,
    /// The whole parse failed (the app keeps the previous document).
    Parse,
}

/// An allowed HTML element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HtmlTag {
    /// A fragment root (`Block::HtmlBlock`).
    #[default]
    Fragment,
    /// `<a>`
    A,
    /// `<abbr>`
    Abbr,
    /// `<b>`
    B,
    /// `<blockquote>`
    Blockquote,
    /// `<br>`
    Br,
    /// `<code>`
    Code,
    /// `<del>`
    Del,
    /// `<details>`
    Details,
    /// `<div>`
    Div,
    /// `<em>`
    Em,
    /// `<i>`
    I,
    /// `<img>`
    Img,
    /// `<ins>`
    Ins,
    /// `<kbd>`
    Kbd,
    /// `<li>`
    Li,
    /// `<mark>`
    Mark,
    /// `<ol>`
    Ol,
    /// `<p>`
    P,
    /// `<picture>`
    Picture,
    /// `<pre>`
    Pre,
    /// `<s>`
    S,
    /// `<source>`
    Source,
    /// `<span>`
    Span,
    /// `<strong>`
    Strong,
    /// `<sub>`
    Sub,
    /// `<summary>`
    Summary,
    /// `<sup>`
    Sup,
    /// `<table>`
    Table,
    /// `<tbody>`
    Tbody,
    /// `<td>`
    Td,
    /// `<tfoot>`
    Tfoot,
    /// `<th>`
    Th,
    /// `<thead>`
    Thead,
    /// `<tr>`
    Tr,
    /// `<u>`
    U,
    /// `<ul>`
    Ul,
    /// `<video>`
    Video,
    /// `<audio>`
    Audio,
    /// `<track>`
    Track,
}

impl HtmlTag {
    /// The lowercase tag name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Fragment => "",
            Self::A => "a",
            Self::Abbr => "abbr",
            Self::B => "b",
            Self::Blockquote => "blockquote",
            Self::Br => "br",
            Self::Code => "code",
            Self::Del => "del",
            Self::Details => "details",
            Self::Div => "div",
            Self::Em => "em",
            Self::I => "i",
            Self::Img => "img",
            Self::Ins => "ins",
            Self::Kbd => "kbd",
            Self::Li => "li",
            Self::Mark => "mark",
            Self::Ol => "ol",
            Self::P => "p",
            Self::Picture => "picture",
            Self::Pre => "pre",
            Self::S => "s",
            Self::Source => "source",
            Self::Span => "span",
            Self::Strong => "strong",
            Self::Sub => "sub",
            Self::Summary => "summary",
            Self::Sup => "sup",
            Self::Table => "table",
            Self::Tbody => "tbody",
            Self::Td => "td",
            Self::Tfoot => "tfoot",
            Self::Th => "th",
            Self::Thead => "thead",
            Self::Tr => "tr",
            Self::U => "u",
            Self::Ul => "ul",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Track => "track",
        }
    }
}

/// One node of the allowed HTML subset.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HtmlNode {
    /// The element.
    pub tag: HtmlTag,
    /// Filtered attributes, in source order.
    pub attrs: Vec<(String, String)>,
    /// Children: elements and text.
    pub children: Vec<HtmlChild>,
    /// `srcset` candidates, parsed before the raw attribute was dropped.
    pub srcset: Vec<SrcSetCandidate>,
}

impl HtmlNode {
    /// A fragment root with children.
    pub fn fragment(children: Vec<HtmlChild>) -> Self {
        Self { tag: HtmlTag::Fragment, attrs: Vec::new(), children, srcset: Vec::new() }
    }

    /// An attribute value.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    /// The concatenated text of this node's subtree.
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        for child in &self.children {
            match child {
                HtmlChild::Text(text) => out.push_str(text),
                HtmlChild::Node(node) => node.collect_text(out),
            }
        }
    }
}

/// A child of an [`HtmlNode`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HtmlChild {
    /// A nested element.
    Node(HtmlNode),
    /// A text node.
    Text(String),
}

/// A block-level Markdown image.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageBlock {
    /// The image URL as written.
    pub src: String,
    /// The alt text.
    pub alt: String,
    /// The optional title.
    pub title: Option<String>,
    /// `srcset` candidates, when the image came from HTML.
    pub srcset: Vec<SrcSetCandidate>,
    /// `<picture><source>` candidates, when the image came from HTML.
    pub picture: Vec<PictureSource>,
    /// The source byte range, end exclusive.
    pub source: Range<usize>,
    /// The 0-based source line.
    pub line: usize,
}

/// One `srcset` candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcSetCandidate {
    /// The candidate URL as written.
    pub url: String,
    /// The descriptor (`1x`, `640w`, …), when present.
    pub descriptor: Option<String>,
}

/// One `<picture><source>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PictureSource {
    /// The `media` attribute.
    pub media: Option<String>,
    /// The `type` attribute (a MIME type).
    pub mime: Option<String>,
    /// The source's candidates.
    pub srcset: Vec<SrcSetCandidate>,
}
