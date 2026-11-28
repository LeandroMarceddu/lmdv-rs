//! Small source-text helpers shared by the parser passes.
//!
//! This module also owns [`MdDoc::find`]: the source-range search every
//! consumer (the reading find bar, headless checks, exports) agrees on (OI-024).

use std::ops::Range;

use crate::model::{Block, Inline, MdDoc, SourceSegment};

/// The byte offset of every line start, including 0 and the empty line after
/// a trailing newline.
pub(crate) fn line_starts(source: &str) -> Vec<u32> {
    let mut starts = Vec::with_capacity(source.len() / 24 + 1);
    starts.push(0);
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push((index + 1) as u32);
        }
    }
    starts
}

/// The line containing `byte` (0-based), given [`line_starts`].
pub(crate) fn line_of(starts: &[u32], byte: usize) -> usize {
    let byte = byte.min(u32::MAX as usize) as u32;
    match starts.binary_search(&byte) {
        Ok(line) => line,
        Err(next) => next.saturating_sub(1),
    }
}

/// The content range of a line, without its trailing `\n`.
pub(crate) fn line_range(source: &str, starts: &[u32], line: usize) -> Option<Range<usize>> {
    let start = *starts.get(line)? as usize;
    if start > source.len() {
        return None;
    }
    let end = match starts.get(line + 1) {
        Some(next) => (*next as usize).saturating_sub(1),
        None => source.len(),
    };
    Some(start..end.max(start))
}

/// Whether a line holds only spaces and tabs.
pub(crate) fn is_blank(text: &str) -> bool {
    text.bytes().all(|byte| byte == b' ' || byte == b'\t')
}

/// Sorted, merged source ranges that other passes must leave alone (code,
/// inline code, raw HTML, already-scanned formulas).
#[derive(Clone, Debug, Default)]
pub(crate) struct Protected {
    ranges: Vec<Range<usize>>,
}

impl Protected {
    /// Build from arbitrary ranges, sorting and merging them.
    pub(crate) fn new(mut ranges: Vec<Range<usize>>) -> Self {
        ranges.retain(|range| !range.is_empty());
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        Self { ranges: merged }
    }

    /// Add one range, merging overlaps. Ranges usually arrive in increasing
    /// order, so the common append path stays cheap.
    pub(crate) fn insert(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        if self.ranges.last().is_none_or(|last| range.start > last.end) {
            self.ranges.push(range);
            return;
        }
        let start = self.ranges.partition_point(|candidate| candidate.end < range.start);
        let end = self.ranges.partition_point(|candidate| candidate.start <= range.end);
        if start == end {
            self.ranges.insert(start, range);
            return;
        }
        let merged = self.ranges[start].start.min(range.start)..self.ranges[end - 1].end.max(range.end);
        self.ranges.splice(start..end, [merged]);
    }

    /// Whether `offset` is protected.
    pub(crate) fn contains(&self, offset: usize) -> bool {
        let index = match self.ranges.binary_search_by(|range| range.start.cmp(&offset)) {
            Ok(_) => return true,
            Err(0) => return false,
            Err(next) => next - 1,
        };
        self.ranges[index].end > offset
    }

    /// The protected range containing `offset`, if any.
    pub(crate) fn at(&self, offset: usize) -> Option<Range<usize>> {
        let index = match self.ranges.binary_search_by(|range| range.start.cmp(&offset)) {
            Ok(index) => index,
            Err(0) => return None,
            Err(next) => next - 1,
        };
        let range = &self.ranges[index];
        (range.end > offset).then(|| range.clone())
    }

    /// Whether any protected range intersects `range`.
    pub(crate) fn overlaps(&self, range: Range<usize>) -> bool {
        let index = self.ranges.partition_point(|candidate| candidate.end <= range.start);
        self.ranges.get(index).is_some_and(|candidate| candidate.start < range.end)
    }
}

/// Case-insensitive substring search, for bounded source scans of HTML tags.
pub(crate) fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    h.len()
        .checked_sub(n.len())
        .and_then(|last| (0..=last).find(|&index| h[index..index + n.len()].eq_ignore_ascii_case(n)))
}

/// The last case-insensitive occurrence of `needle`, for bounded back-scans.
pub(crate) fn rfind_ci(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(haystack.len());
    }
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    (0..=h.len().checked_sub(n.len())?).rev().find(|&index| h[index..index + n.len()].eq_ignore_ascii_case(n))
}

/// Repair source-map segments the inline builder could only map linearly
/// (OI-025). Pulldown-cmark decodes entities and backslash escapes in the
/// rendered text but reports the range of the *source spelling*: `&amp;` is
/// five source bytes for one rendered `&`, `\*` two for one. Walk the source
/// beside the rendered bytes and split the segment wherever they diverge.
/// Segments that cannot be realigned (typographic quotes, math placeholders,
/// already-exact text) are left untouched, so existing maps stay exact.
pub(crate) fn repair_source_map(source: &str, inline: &mut Inline) {
    let mut out = Vec::with_capacity(inline.source_map.segments.len());
    let mut scratch = Vec::new();
    for segment in &inline.source_map.segments {
        scratch.clear();
        if repair_segment(source, &inline.text, segment, &mut scratch) {
            out.append(&mut scratch);
        } else {
            out.push(segment.clone());
        }
    }
    inline.source_map.segments = out;
}

fn repair_segment(source: &str, rendered: &str, segment: &SourceSegment, out: &mut Vec<SourceSegment>) -> bool {
    let Some(text) = rendered.get(segment.rendered.clone()) else { return false };
    let bytes = text.as_bytes();
    let source_bytes = source.as_bytes();
    let mut i = 0usize;
    let mut j = segment.source.min(source.len());
    while i < bytes.len() && j < source_bytes.len() {
        // An entity decodes to a shorter run of rendered bytes. Only accept a
        // token that really produces what the inline shows at this position.
        if source_bytes[j] == b'&' {
            let rest = &source[j..];
            if let Some(semicolon) = rest[1..].find(';')
                && semicolon <= 32
                && let Some(decoded) = crate::html::decode_entity(&rest[1..1 + semicolon])
                && bytes[i..].starts_with(decoded.as_bytes())
            {
                // Every rendered byte of a decoded character names the first
                // source byte of its entity. `SourceMap::source_range` derives
                // the end from the last rendered byte, so a multi-byte glyph
                // must not walk into the following source byte.
                for byte in 0..decoded.len() {
                    let at = segment.rendered.start + i + byte;
                    push_segment(out, at..at + 1, j);
                }
                i += decoded.len();
                j += semicolon + 2;
                continue;
            }
        }
        // A backslash escape keeps the escaped byte but drops the backslash.
        if source_bytes[j] == b'\\' && j + 1 < source_bytes.len() && source_bytes[j + 1] == bytes[i] {
            push_segment(out, segment.rendered.start + i..segment.rendered.start + i + 1, j);
            i += 1;
            j += 2;
            continue;
        }
        if source_bytes[j] != bytes[i] {
            break;
        }
        push_segment(out, segment.rendered.start + i..segment.rendered.start + i + 1, j);
        i += 1;
        j += 1;
    }
    i == bytes.len()
}

fn push_segment(out: &mut Vec<SourceSegment>, rendered: Range<usize>, source: usize) {
    if rendered.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.rendered.end == rendered.start
        && last.source + last.rendered.len() == source
    {
        last.rendered.end = rendered.end;
    } else {
        out.push(SourceSegment { rendered, source });
    }
}

impl MdDoc {
    /// Every case-insensitive occurrence of `query` in the document's drawn
    /// text, as source byte ranges in drawing order (R-18, OI-024).
    ///
    /// The query is lowercased once; inline text is searched through its source
    /// map so Markdown markers never match. Code spans are ordinary inline
    /// text; code-block bodies use the same line map the renderer draws.
    /// Generated or unmapped surfaces (front-matter values, the TOC labels,
    /// formula TeX and Mermaid source) are not searched (OI-127). There is no
    /// regex, whole-word matching or accent folding (R-18).
    pub fn find(&self, query: &str) -> Vec<Range<usize>> {
        let needle = query.to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for block in &self.blocks {
            self.find_in_block(block, &needle, &mut out);
        }
        out
    }

    fn find_in_block(&self, block: &Block, needle: &str, out: &mut Vec<Range<usize>>) {
        match block {
            Block::Para(inline) | Block::Heading { inline, .. } => find_in_inline(inline, needle, out),
            Block::Code { text, source, .. } => {
                for (drawn, from) in code_segments(self.source.as_ref(), text, source) {
                    for hit in match_ranges(&text[drawn], needle) {
                        out.push(from.start + hit.start..from.start + hit.end);
                    }
                }
            }
            Block::List { items, .. } => {
                for item in items {
                    for block in item {
                        self.find_in_block(block, needle, out);
                    }
                }
            }
            Block::Quote { blocks, .. } => {
                for block in blocks {
                    self.find_in_block(block, needle, out);
                }
            }
            Block::Table(table) => {
                for cell in &table.head {
                    find_in_inline(cell, needle, out);
                }
                for row in &table.rows {
                    for cell in row {
                        find_in_inline(cell, needle, out);
                    }
                }
            }
            Block::FootnoteDefs => {
                for footnote in &self.footnotes {
                    for block in &footnote.blocks {
                        self.find_in_block(block, needle, out);
                    }
                }
            }
            Block::Rule
            | Block::Mermaid { .. }
            | Block::MathBlock { .. }
            | Block::HtmlBlock(_)
            | Block::Image(_)
            | Block::Toc => {}
        }
    }
}

fn find_in_inline(inline: &Inline, needle: &str, out: &mut Vec<Range<usize>>) {
    for hit in match_ranges(&inline.text, needle) {
        if let Some(source) = inline.source_map.source_range(hit)
            && !source.is_empty()
        {
            out.push(source);
        }
    }
}

/// The source and drawn byte ranges of each line of a code block's text.
///
/// Mirrors the renderer's `code_segments` (`lmdv-render`): the drawn text is
/// the block body after the opening fence (or with indentation dropped for an
/// indented block), found line by line inside the block's own source span.
fn code_segments(source: &str, code: &str, span: &Range<usize>) -> Vec<(Range<usize>, Range<usize>)> {
    let Some(block) = source.get(span.clone()) else { return Vec::new() };
    let indent = block.len() - block.trim_start_matches(' ').len();
    let fenced = indent <= 3 && {
        let rest = &block[indent..];
        rest.starts_with("```") || rest.starts_with("~~~")
    };
    let mut cursor = if fenced { block.find('\n').map_or(block.len(), |at| at + 1) } else { 0 };
    let mut drawn = 0usize;
    let mut segments = Vec::new();
    for line in code.split('\n') {
        if !line.is_empty()
            && let Some(at) = block[cursor..].find(line)
        {
            let start = span.start + cursor + at;
            segments.push((drawn..drawn + line.len(), start..start + line.len()));
            cursor += at + line.len();
        }
        drawn += line.len() + 1;
    }
    segments
}

/// Case-insensitive, non-overlapping byte ranges of `needle_lower` in `text`.
///
/// Unicode lowercasing can change a character's byte length, so the lowered
/// text keeps the original byte bounds of every lowered byte; a hit is mapped
/// back to whole original characters. Empty queries match nothing.
pub fn match_ranges(text: &str, needle_lower: &str) -> Vec<Range<usize>> {
    if needle_lower.is_empty() || text.is_empty() {
        return Vec::new();
    }
    let mut lower = String::with_capacity(text.len());
    let mut starts = Vec::with_capacity(text.len());
    let mut ends = Vec::with_capacity(text.len());
    for (index, ch) in text.char_indices() {
        let before = lower.len();
        for lowered in ch.to_lowercase() {
            lower.push(lowered);
        }
        let end = index + ch.len_utf8();
        for _ in before..lower.len() {
            starts.push(index);
            ends.push(end);
        }
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find(needle_lower) {
        let start = from + at;
        let end = start + needle_lower.len();
        if let (Some(&source_start), Some(&source_end)) = (starts.get(start), ends.get(end.saturating_sub(1))) {
            out.push(source_start..source_end);
        }
        from = end;
        if from >= lower.len() {
            break;
        }
    }
    out
}
