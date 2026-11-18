//! The event walk that builds [`Block`]s, headings, footnotes, tables and the
//! document-level source map.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::callout::CalloutSpan;
use crate::frontmatter;
use crate::heading::{Slugger, rename_reserved};
use crate::html;
use crate::inline::{self, InlineContext};
use crate::math;
use crate::model::{
    Align, Block, BlockSpan, Diagnostic, DiagnosticCode, DiagnosticSeverity, Footnote, FrontMatter, Heading, HtmlNode,
    ImageBlock, Inline, InlineObjectKind, MathSpan, MdDoc, OBJECT_CHAR, Table,
};
use crate::text::{Protected, line_of, line_starts};

/// Parse a document with revision 0.
pub fn parse(source: &str) -> MdDoc {
    parse_with_revision(source, 0)
}

/// Parse a document tagged with the revisions it came from.
pub fn parse_with_revision(source: &str, revision: u64) -> MdDoc {
    let starts = line_starts(source);
    let front = frontmatter::scan(source);
    let protected = protected_ranges(&front.text);
    let (callouts, after_callouts) = crate::callout::scan(&front.text, &protected);
    let math = math::scan(&after_callouts, &protected);
    assemble(
        source,
        &math.text,
        revision,
        &starts,
        front.front_matter,
        &protected,
        &callouts,
        &math.spans,
        &math.placeholders,
    )
}

/// The Markdown options the final parse uses: GFM tables, task lists,
/// strikethrough and footnotes. Callouts, math and typography are handled by
/// our own passes; GFM's alert option is not used because it rejects titles.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_FOOTNOTES
}

/// Ranges the math and callout scanners must not touch: code blocks, inline
/// code, and raw HTML blocks and inline tags.
pub(crate) fn protected_ranges(source: &str) -> Protected {
    let mut ranges = Vec::new();
    let mut code_start = None;
    let mut html_start = None;
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code_start = Some(range.start),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = code_start.take() {
                    ranges.push(start..range.end);
                }
            }
            Event::Start(Tag::HtmlBlock) => html_start = Some(range.start),
            Event::End(TagEnd::HtmlBlock) => {
                if let Some(start) = html_start.take() {
                    ranges.push(start..range.end);
                }
            }
            Event::Code(_) | Event::InlineHtml(_) => ranges.push(range),
            _ => {}
        }
    }
    Protected::new(ranges)
}

struct FootnoteEntry {
    blocks: Vec<Block>,
    source: Range<usize>,
}

struct BlockParser<'a> {
    events: &'a [(Event<'a>, Range<usize>)],
    source: &'a str,
    starts: &'a [u32],
    math: &'a [MathSpan],
    callouts: &'a [CalloutSpan],
    slugger: Slugger,
    headings: Vec<Heading>,
    footnotes: HashMap<String, FootnoteEntry>,
    diagnostics: Vec<Diagnostic>,
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    source: &str,
    transformed: &str,
    revision: u64,
    starts: &[u32],
    front_matter: Option<FrontMatter>,
    protected: &Protected,
    callouts: &[CalloutSpan],
    math: &[MathSpan],
    placeholders: &[usize],
) -> MdDoc {
    let events: Vec<(Event<'_>, Range<usize>)> = Parser::new_ext(transformed, options()).into_offset_iter().collect();
    let mut definitions = HashSet::new();
    for (event, _) in &events {
        if let Event::Start(Tag::FootnoteDefinition(label)) = event {
            definitions.insert(label.to_string());
        }
    }
    let math_by_start: HashMap<usize, usize> =
        placeholders.iter().copied().enumerate().map(|(index, placeholder)| (placeholder, index)).collect();

    let mut parser = BlockParser {
        events: &events,
        source: transformed,
        starts,
        math,
        callouts,
        slugger: Slugger::new(),
        headings: Vec::new(),
        footnotes: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let mut ctx = InlineContext {
        source: transformed,
        starts,
        math,
        math_by_start: &math_by_start,
        definitions: &definitions,
        footnote_numbers: HashMap::new(),
        first_refs: HashMap::new(),
        next_footnote: 1,
        diagnostics: Vec::new(),
    };

    let mut index = 0;
    let mut blocks = Vec::new();
    while index < parser.events.len() {
        let top = blocks.len();
        let before = index;
        if let Some(block) = parser.parse_one(&mut index, &mut ctx, 0, top, false) {
            blocks.push(block);
        } else if index == before {
            index += 1;
        }
    }

    let mut footnotes = Vec::new();
    let mut ordered: Vec<(usize, String)> =
        ctx.footnote_numbers.iter().map(|(label, number)| (*number, label.clone())).collect();
    ordered.sort_by_key(|(number, _)| *number);
    for (number, label) in ordered {
        if let Some(entry) = parser.footnotes.remove(&label) {
            footnotes.push(Footnote {
                first_ref: ctx.first_refs.get(&label).cloned(),
                label,
                number,
                blocks: entry.blocks,
                source: entry.source,
            });
        }
    }
    if !footnotes.is_empty() {
        let line = line_of(starts, source.len());
        blocks.push((Block::FootnoteDefs, source.len()..source.len(), line));
    }

    let mut block_spans = Vec::with_capacity(blocks.len());
    let mut plain_blocks = Vec::with_capacity(blocks.len());
    for (position, (block, range, line_hint)) in blocks.into_iter().enumerate() {
        let line = if range.is_empty() { line_hint } else { line_of(starts, range.start) };
        let end_line = if range.is_empty() { line } else { line_of(starts, range.end.saturating_sub(1)) + 1 };
        block_spans.push(BlockSpan { key: format!("b{position}"), source: range, line, end_line });
        plain_blocks.push(block);
    }

    let mut diagnostics = parser.diagnostics;
    diagnostics.extend(ctx.diagnostics);
    diagnostics.extend(undefined_footnote_diagnostics(transformed, starts, protected, &definitions));
    diagnostics.sort_by_key(|diagnostic| diagnostic.line.unwrap_or(usize::MAX));

    MdDoc {
        source: Arc::from(source),
        revision,
        blocks: plain_blocks,
        block_spans,
        headings: parser.headings,
        footnotes,
        front_matter,
        diagnostics,
        line_starts: Arc::from(starts.to_vec()),
    }
}

impl<'a> BlockParser<'a> {
    fn parse_one(
        &mut self,
        index: &mut usize,
        ctx: &mut InlineContext<'_>,
        depth: usize,
        top: usize,
        in_footnote: bool,
    ) -> Option<(Block, Range<usize>, usize)> {
        let (event, range) = self.events.get(*index)?.clone();
        if is_inline_event(&event) {
            let start = *index;
            while *index < self.events.len() && is_inline_event(&self.events[*index].0) {
                *index += 1;
            }
            let mut range = self.events[start].1.start..self.events[*index - 1].1.end;
            let built = inline::build(&self.events[start..*index], ctx);
            if built.inline.is_empty() {
                return None;
            }
            let mut line = line_of(self.starts, range.start);
            let block = self.paragraph_block(built.inline, &mut range, &mut line);
            return Some((block, range, line));
        }
        match event {
            Event::Start(Tag::Paragraph) => {
                let built = self.inline_until(index, TagEnd::Paragraph, ctx)?;
                let mut range = range.start..self.last_end(index);
                let mut line = line_of(self.starts, range.start);
                if built.inline.is_empty() {
                    return None;
                }
                let block = self.paragraph_block(built.inline, &mut range, &mut line);
                Some((block, range, line))
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let built = self.inline_until(index, TagEnd::Heading(level), ctx)?;
                let range = range.start..self.last_end(index);
                let line = line_of(self.starts, range.start);
                let heading = self.add_heading(level, built.slug_text.trim(), top, line, in_footnote);
                Some((Block::Heading { heading, inline: built.inline }, range, line))
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let end = self.find_end(*index, TagEnd::CodeBlock)?;
                let mut text = String::new();
                for (event, _) in &self.events[*index + 1..end] {
                    if let Event::Text(part) = event {
                        text.push_str(part);
                    }
                }
                let range = range.start..self.events[end].1.end;
                let line = line_of(self.starts, range.start);
                *index = end + 1;
                let text = text.strip_suffix('\n').unwrap_or(&text).to_owned();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().map(str::to_owned),
                    CodeBlockKind::Indented => None,
                };
                let block = match lang.as_deref() {
                    Some(name) if name.eq_ignore_ascii_case("mermaid") => Block::Mermaid { source: text, line },
                    Some(name) if name.eq_ignore_ascii_case("math") => {
                        Block::MathBlock { tex: text.trim().to_owned(), line }
                    }
                    _ => Block::Code { lang: lang.filter(|name| !name.is_empty()), text, source: range.clone() },
                };
                Some((block, range, line))
            }
            Event::Start(Tag::HtmlBlock) => {
                let end = self.find_end(*index, TagEnd::HtmlBlock)?;
                let mut raw = String::new();
                for (event, _) in &self.events[*index + 1..end] {
                    match event {
                        Event::Html(part) | Event::Text(part) => raw.push_str(part),
                        _ => {}
                    }
                }
                let range = range.start..self.events[end].1.end;
                let line = line_of(self.starts, range.start);
                *index = end + 1;
                let node = html::parse_block(&raw)?;
                let block = match html_block_image(&node) {
                    Some(mut image) => {
                        image.source = range.clone();
                        image.line = line;
                        Block::Image(image)
                    }
                    None => Block::HtmlBlock(node),
                };
                Some((block, range, line))
            }
            Event::Start(Tag::BlockQuote(_)) => {
                let end = self.find_end(*index, TagEnd::BlockQuote(None))?;
                let inner_depth = depth + 1;
                let mut inner_index = *index + 1;
                let mut inner = Vec::new();
                while inner_index < end {
                    let before = inner_index;
                    let step = stacker::maybe_grow(64 * 1024, 2 * 1024 * 1024, || {
                        self.parse_one(&mut inner_index, ctx, inner_depth, top, in_footnote)
                    });
                    if let Some(block) = step {
                        inner.push(block);
                    } else if inner_index == before {
                        inner_index += 1;
                    }
                }
                let range = range.start..self.events[end].1.end;
                let line = line_of(self.starts, range.start);
                *index = end + 1;
                let callout = self
                    .callouts
                    .iter()
                    .find(|callout| callout.line == line && callout.depth == inner_depth)
                    .map(|callout| callout.callout.clone());
                let blocks: Vec<Block> = inner.into_iter().map(|(block, _, _)| block).collect();
                Some((Block::Quote { blocks, callout }, range, line))
            }
            Event::Start(Tag::List(start)) => {
                let end = self.find_end(*index, TagEnd::List(start.is_some()))?;
                let mut inner_index = *index + 1;
                let mut items = Vec::new();
                let mut item_ranges: Vec<Range<usize>> = Vec::new();
                let mut item_block_ranges: Vec<Vec<Range<usize>>> = Vec::new();
                while inner_index < end {
                    match self.events.get(inner_index).map(|(event, _)| event) {
                        Some(Event::Start(Tag::Item)) => {
                            let item_end = self.find_end(inner_index, TagEnd::Item)?;
                            let item_range = self.events[inner_index].1.start..self.events[item_end].1.end;
                            let mut item_index = inner_index + 1;
                            let mut item = Vec::new();
                            let mut block_ranges = Vec::new();
                            while item_index < item_end {
                                let before = item_index;
                                let step = stacker::maybe_grow(64 * 1024, 2 * 1024 * 1024, || {
                                    self.parse_one(&mut item_index, ctx, depth, top, in_footnote)
                                });
                                if let Some(block) = step {
                                    block_ranges.push(block.1.clone());
                                    item.push(block);
                                } else if item_index == before {
                                    item_index += 1;
                                }
                            }
                            items.push(item.into_iter().map(|(block, _, _)| block).collect());
                            item_ranges.push(item_range);
                            item_block_ranges.push(block_ranges);
                            inner_index = item_end + 1;
                        }
                        Some(_) => inner_index += 1,
                        None => break,
                    }
                }
                let range = range.start..self.events[end].1.end;
                let line = line_of(self.starts, range.start);
                *index = end + 1;
                let tight =
                    !item_ranges.windows(2).any(|pair| gap_has_blank_line(self.source, pair[0].end, pair[1].start))
                        && !item_block_ranges.iter().any(|ranges| {
                            ranges.windows(2).any(|pair| gap_has_blank_line(self.source, pair[0].end, pair[1].start))
                        });
                Some((Block::List { ordered: start.is_some(), start: start.unwrap_or(1), tight, items }, range, line))
            }
            Event::Start(Tag::Table(aligns)) => {
                let end = self.find_end(*index, TagEnd::Table)?;
                let range = range.start..self.events[end].1.end;
                let line = line_of(self.starts, range.start);
                let table = self.parse_table(*index, end, aligns, ctx)?;
                let mut table = table;
                table.source = range.clone();
                table.line = line;
                *index = end + 1;
                Some((Block::Table(table), range, line))
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                let end = self.find_end(*index, TagEnd::FootnoteDefinition)?;
                let mut inner_index = *index + 1;
                let mut inner = Vec::new();
                while inner_index < end {
                    let before = inner_index;
                    let step = stacker::maybe_grow(64 * 1024, 2 * 1024 * 1024, || {
                        self.parse_one(&mut inner_index, ctx, depth, top, true)
                    });
                    if let Some(block) = step {
                        inner.push(block);
                    } else if inner_index == before {
                        inner_index += 1;
                    }
                }
                let source = range.start..self.events[end].1.end;
                *index = end + 1;
                let label = label.to_string();
                if self.footnotes.contains_key(&label) {
                    self.diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Warning,
                        code: DiagnosticCode::DuplicateFootnote,
                        message: format!("footnote `{label}` is defined more than once; the last one wins"),
                        line: Some(line_of(self.starts, source.start)),
                        source: Some(source.clone()),
                    });
                }
                let blocks = inner.into_iter().map(|(block, _, _)| block).collect();
                self.footnotes.insert(label, FootnoteEntry { blocks, source });
                None
            }
            Event::Rule => {
                *index += 1;
                let line = line_of(self.starts, range.start);
                Some((Block::Rule, range, line))
            }
            Event::End(_) => {
                *index += 1;
                None
            }
            _ => {
                *index += 1;
                None
            }
        }
    }

    /// Build an inline run from `index + 1` to the matching end event.
    fn inline_until(
        &mut self,
        index: &mut usize,
        end_tag: TagEnd,
        ctx: &mut InlineContext<'_>,
    ) -> Option<inline::BuiltInline> {
        let end = self.find_end(*index, end_tag)?;
        let built = inline::build(&self.events[*index + 1..end], ctx);
        *index = end + 1;
        Some(built)
    }

    fn parse_table(
        &mut self,
        start: usize,
        end: usize,
        aligns: Vec<Alignment>,
        ctx: &mut InlineContext<'_>,
    ) -> Option<Table> {
        let align = aligns
            .into_iter()
            .map(|alignment| match alignment {
                Alignment::None => Align::None,
                Alignment::Left => Align::Left,
                Alignment::Center => Align::Center,
                Alignment::Right => Align::Right,
            })
            .collect::<Vec<_>>();
        let mut head = Vec::new();
        let mut rows = Vec::new();
        let mut index = start + 1;
        while index < end {
            match self.events.get(index).map(|(event, _)| event) {
                Some(Event::Start(Tag::TableHead)) => {
                    let head_end = self.find_end(index, TagEnd::TableHead)?;
                    head = self.parse_cells(index + 1, head_end, ctx);
                    index = head_end + 1;
                }
                Some(Event::Start(Tag::TableRow)) => {
                    let row_end = self.find_end(index, TagEnd::TableRow)?;
                    rows.push(self.parse_cells(index + 1, row_end, ctx));
                    index = row_end + 1;
                }
                Some(_) => index += 1,
                None => break,
            }
        }
        let columns = align.len().max(head.len()).max(rows.iter().map(Vec::len).max().unwrap_or(0));
        let mut align = align;
        align.resize(columns, Align::None);
        head.resize_with(columns, Inline::default);
        for row in &mut rows {
            row.resize_with(columns, Inline::default);
        }
        Some(Table { align, head, rows, source: 0..0, line: 0 })
    }

    fn parse_cells(&mut self, start: usize, end: usize, ctx: &mut InlineContext<'_>) -> Vec<Inline> {
        let mut cells = Vec::new();
        let mut index = start;
        while index < end {
            match self.events.get(index).map(|(event, _)| event) {
                Some(Event::Start(Tag::TableCell)) => {
                    if let Some(cell_end) = self.find_end(index, TagEnd::TableCell) {
                        cells.push(inline::build(&self.events[index + 1..cell_end], ctx).inline);
                        index = cell_end + 1;
                    } else {
                        break;
                    }
                }
                Some(_) => index += 1,
                None => break,
            }
        }
        cells
    }

    fn paragraph_block(&mut self, inline: Inline, range: &mut Range<usize>, line: &mut usize) -> Block {
        if is_toc(&inline.text) {
            return Block::Toc;
        }
        let trimmed = inline.text.trim();
        if trimmed == OBJECT_CHAR.to_string() {
            if let Some(math) = inline.math.first()
                && inline.math.len() == 1
                && inline.objects.is_empty()
                && inline.links.is_empty()
            {
                let placeholder = inline.source_map.source_of(math.range.start).unwrap_or(range.start);
                if let Some(span) = self.math.iter().find(|span| span.display && span.source.contains(&placeholder)) {
                    *range = span.source.clone();
                    *line = span.line;
                    return Block::MathBlock { tex: span.tex.clone(), line: span.line };
                }
            }
            if let Some(object) = inline.objects.first()
                && inline.objects.len() == 1
                && object.kind == InlineObjectKind::Image
                && inline.links.is_empty()
                && let Some(node) = &object.html
                && let Some(mut image) = html::image_from_html(node)
            {
                image.source = range.clone();
                image.line = *line;
                return Block::Image(image);
            }
        }
        Block::Para(inline)
    }

    fn add_heading(&mut self, level: HeadingLevel, text: &str, top: usize, line: usize, in_footnote: bool) -> usize {
        let text = text.to_owned();
        let (id, legacy_id) = self.slugger.slug(&text);
        let (id, anchor) = rename_reserved(id);
        let index = self.headings.len();
        self.headings.push(Heading {
            level: level as u8,
            id,
            legacy_id,
            anchor,
            text,
            block: if in_footnote { usize::MAX } else { top },
            line,
            in_toc: !in_footnote && (level as u8) <= 4,
        });
        index
    }

    fn find_end(&self, start: usize, stop: TagEnd) -> Option<usize> {
        let mut depth = 0usize;
        let mut index = start + 1;
        while let Some((event, _)) = self.events.get(index) {
            match event {
                Event::Start(tag) if start_matches_end(tag, &stop) => depth += 1,
                Event::End(end) if same_end(end, &stop) => {
                    if depth == 0 {
                        return Some(index);
                    }
                    depth -= 1;
                }
                _ => {}
            }
            index += 1;
        }
        None
    }

    fn last_end(&self, index: &usize) -> usize {
        index.checked_sub(1).and_then(|previous| self.events.get(previous)).map_or(0, |(_, range)| range.end)
    }
}

fn is_inline_event(event: &Event<'_>) -> bool {
    match event {
        Event::Text(_)
        | Event::Code(_)
        | Event::InlineHtml(_)
        | Event::SoftBreak
        | Event::HardBreak
        | Event::FootnoteReference(_)
        | Event::TaskListMarker(_) => true,
        Event::Start(tag) => matches!(
            tag,
            Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::Superscript
                | Tag::Subscript
        ),
        Event::End(end) => matches!(
            end,
            TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image
                | TagEnd::Superscript
                | TagEnd::Subscript
        ),
        _ => false,
    }
}

fn same_end(actual: &TagEnd, expected: &TagEnd) -> bool {
    matches!(
        (actual, expected),
        (TagEnd::Paragraph, TagEnd::Paragraph)
            | (TagEnd::Heading(_), TagEnd::Heading(_))
            | (TagEnd::BlockQuote(_), TagEnd::BlockQuote(_))
            | (TagEnd::CodeBlock, TagEnd::CodeBlock)
            | (TagEnd::HtmlBlock, TagEnd::HtmlBlock)
            | (TagEnd::List(_), TagEnd::List(_))
            | (TagEnd::Item, TagEnd::Item)
            | (TagEnd::FootnoteDefinition, TagEnd::FootnoteDefinition)
            | (TagEnd::Table, TagEnd::Table)
            | (TagEnd::TableHead, TagEnd::TableHead)
            | (TagEnd::TableRow, TagEnd::TableRow)
            | (TagEnd::TableCell, TagEnd::TableCell)
    )
}

fn start_matches_end(tag: &Tag<'_>, end: &TagEnd) -> bool {
    matches!(
        (tag, end),
        (Tag::Paragraph, TagEnd::Paragraph)
            | (Tag::Heading { .. }, TagEnd::Heading(_))
            | (Tag::BlockQuote(_), TagEnd::BlockQuote(_))
            | (Tag::CodeBlock(_), TagEnd::CodeBlock)
            | (Tag::HtmlBlock, TagEnd::HtmlBlock)
            | (Tag::List(_), TagEnd::List(_))
            | (Tag::Item, TagEnd::Item)
            | (Tag::FootnoteDefinition(_), TagEnd::FootnoteDefinition)
            | (Tag::Table(_), TagEnd::Table)
            | (Tag::TableHead, TagEnd::TableHead)
            | (Tag::TableRow, TagEnd::TableRow)
            | (Tag::TableCell, TagEnd::TableCell)
    )
}

fn is_toc(text: &str) -> bool {
    let text = text.trim();
    text.eq_ignore_ascii_case("[toc]") || text.eq_ignore_ascii_case("[[toc]]")
}

/// Whether the gap between two block ranges contains a blank line. The
/// previous block's range usually includes its trailing newline, so the gap is
/// widened by one byte to see the blank line's own newline.
fn gap_has_blank_line(source: &str, previous_end: usize, next_start: usize) -> bool {
    let start = if previous_end > 0
        && previous_end <= source.len()
        && source.as_bytes().get(previous_end - 1) == Some(&b'\n')
    {
        previous_end - 1
    } else {
        previous_end
    };
    start <= next_start && next_start <= source.len() && has_blank_line(&source[start..next_start])
}

fn has_blank_line(text: &str) -> bool {
    let bytes = text.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b'\n' {
            continue;
        }
        let mut next = index + 1;
        while next < bytes.len() && matches!(bytes[next], b' ' | b'\t') {
            next += 1;
        }
        if next < bytes.len() && bytes[next] == b'\n' {
            return true;
        }
    }
    false
}

fn html_block_image(node: &HtmlNode) -> Option<ImageBlock> {
    if node.tag != crate::model::HtmlTag::Fragment {
        return html::image_from_html(node);
    }
    if node.children.iter().any(|child| matches!(child, crate::model::HtmlChild::Text(text) if !text.trim().is_empty()))
    {
        return None;
    }
    let mut nodes = node.children.iter().filter_map(|child| match child {
        crate::model::HtmlChild::Node(node) => Some(node),
        crate::model::HtmlChild::Text(_) => None,
    });
    let first = nodes.next()?;
    nodes.next().is_none().then(|| html::image_from_html(first)).flatten()
}

fn undefined_footnote_diagnostics(
    source: &str,
    starts: &[u32],
    protected: &Protected,
    definitions: &HashSet<String>,
) -> Vec<Diagnostic> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if let Some(range) = protected.at(index) {
            index = range.end;
            continue;
        }
        if bytes[index] != b'[' || bytes[index + 1] != b'^' {
            index += 1;
            continue;
        }
        let Some(offset) = source[index + 2..].find(']') else { break };
        let close = index + 2 + offset;
        let label = &source[index + 2..close];
        let is_definition = bytes.get(close + 1) == Some(&b':');
        let valid = !label.is_empty() && !label.chars().any(char::is_whitespace);
        if valid && !is_definition && !definitions.contains(label) {
            out.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                code: DiagnosticCode::UndefinedFootnote,
                message: format!("footnote `{label}` has no definition"),
                line: Some(line_of(starts, index)),
                source: Some(index..close + 1),
            });
        }
        index = close + 1;
    }
    out
}
