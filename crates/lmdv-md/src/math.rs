//! The math pre-pass: GitHub's `$`/`$$` delimiters, ported from
//! the original app's `src/renderer.ts:268-390`.
//!
//! Formulas are replaced, before the Markdown parse, by a placeholder of the
//! same byte length: U+FFFC (three bytes) followed by spaces, with newlines
//! kept. Pulldown-cmark therefore sees a normal document and every offset and
//! line number stays exact. Two one-byte sentinels cover the cases U+FFFC
//! cannot: a block formula with no three-byte run on a line, and the
//! continuation lines of a multi-line inline formula (so a blanked line never
//! becomes a blank line that splits a paragraph or quote). The inline builder
//! widens both to U+FFFC in the rendered text.

use std::ops::Range;

use crate::model::{MathScan, MathSpan};

/// A one-byte placeholder for a block formula whose lines are too short for
/// U+FFFC (`$$`, then `x`, then `$$`). The inline builder widens it to U+FFFC
/// in the rendered text; keeping one byte here preserves every newline.
pub(crate) const SHORT_OBJECT: char = '\u{0001}';

/// A one-byte filler on the continuation lines of a multi-line inline formula,
/// so the blanked line is not a blank line that would split the paragraph.
pub(crate) const PADDING_OBJECT: char = '\u{0002}';
use crate::text::{Protected, is_blank, line_of, line_range, line_starts};

/// Replace every formula in `source` with a same-length placeholder.
pub(crate) fn scan(source: &str, protected: &Protected) -> MathScan {
    let starts = line_starts(source);
    let mut out = source.as_bytes().to_vec();
    let mut spans = Vec::new();
    let mut placeholders = Vec::new();
    let mut guard = protected.clone();

    // 1. `$$` blocks that own their lines.
    let mut line = 0;
    while let Some(range) = line_range(source, &starts, line) {
        if guard.contains(range.start) {
            line += 1;
            continue;
        }
        let raw = &source[range.clone()];
        let container = strip_containers(raw);
        if container.indent >= 4 || container.content >= raw.len() {
            line += 1;
            continue;
        }
        let content_start = range.start + container.content;
        let content = &source[content_start..range.end];
        if !content.starts_with("$$") || content.as_bytes().get(2) == Some(&b'$') {
            line += 1;
            continue;
        }

        let Some((close, close_line)) =
            find_block_close(source, &starts, &guard, line, content_start, container.quote_depth)
        else {
            line += 1;
            continue;
        };
        let tex = block_tex(source, &starts, line, container.indent, content_start, close, close_line);
        if tex.trim().is_empty() {
            line += 1;
            continue;
        }
        if out.len() >= close + 2
            && let Some(placeholder) = write_block_placeholder(
                &mut out,
                source,
                &starts,
                line,
                content_start,
                close,
                close_line,
                container.indent,
            )
        {
            spans.push(MathSpan { source: content_start..close + 2, tex, display: true, line });
            placeholders.push(placeholder);
            guard.insert(content_start..close + 2);
        }
        line = close_line + 1;
    }

    // 2. Inline `$…$`, `$`…`$` and inline `$$…$$`.
    //
    // `paragraph_end` walks the following lines to the next blank line or
    // block start. The scan only moves forward and formulas found so far are
    // behind it, so the boundary found for one `$` is still valid for every
    // later `$` in the same paragraph: cache it instead of walking the
    // paragraph once per formula (OI-143).
    let mut paragraph: Option<Range<usize>> = None;
    let mut index = 0;
    while index < source.len() {
        if let Some(range) = guard.at(index) {
            index = range.end;
            continue;
        }
        match source.as_bytes()[index] {
            b'$' => {
                let pos_max = match &paragraph {
                    Some(range) if range.contains(&index) => range.end,
                    _ => {
                        let end = paragraph_end(source, &starts, index, &guard);
                        let start =
                            line_range(source, &starts, line_of(&starts, index)).map_or(index, |range| range.start);
                        paragraph = Some(start..end);
                        end
                    }
                };
                let quote_depth = quote_depth_at(source, &starts, index);
                if source.as_bytes()[index..].starts_with(b"$$") {
                    match inline_display(source, index, pos_max, quote_depth) {
                        Some((range, tex)) => {
                            if let Some(placeholder) = write_inline_span(&mut out, source, &starts, range.clone()) {
                                spans.push(MathSpan {
                                    source: range.clone(),
                                    tex,
                                    display: true,
                                    line: line_of(&starts, range.start),
                                });
                                placeholders.push(placeholder);
                                guard.insert(range.clone());
                                index = range.end;
                            } else {
                                index += 2;
                            }
                        }
                        None => index += 2,
                    }
                } else {
                    match inline_math(source, index, pos_max, quote_depth) {
                        Some((range, tex)) => {
                            if let Some(placeholder) = write_inline_span(&mut out, source, &starts, range.clone()) {
                                spans.push(MathSpan {
                                    source: range.clone(),
                                    tex,
                                    display: false,
                                    line: line_of(&starts, range.start),
                                });
                                placeholders.push(placeholder);
                                guard.insert(range.clone());
                                index = range.end;
                            } else {
                                index += 1;
                            }
                        }
                        None => index += 1,
                    }
                }
            }
            b'<' => match angle_end(source, index) {
                Some(end) => index = end,
                None => index += 1,
            },
            b'[' => match reference_definition_end(source, &starts, index) {
                Some(end) => index = end,
                None => index += 1,
            },
            b']' if source.as_bytes().get(index + 1) == Some(&b'(') => {
                index = link_destination_end(source, index + 2);
            }
            _ => index += 1,
        }
    }

    let mut ordered = spans.into_iter().zip(placeholders).collect::<Vec<_>>();
    ordered.sort_by_key(|(span, _)| span.source.start);
    let (spans, placeholders): (Vec<_>, Vec<_>) = ordered.into_iter().unzip();
    match String::from_utf8(out) {
        Ok(text) => MathScan { text, spans, placeholders },
        // The rewrite only writes ASCII and U+FFFC over ASCII delimiters; a
        // failure would mean an internal bug, so fall back to the input.
        Err(_) => MathScan { text: source.to_owned(), spans: Vec::new(), placeholders: Vec::new() },
    }
}

/// The first unescaped `$$` in `from..to`, ignoring an odd run of backslashes.
fn find_dollars(source: &str, from: usize, to: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = from;
    while index + 1 < to.min(bytes.len()) {
        if bytes[index] == b'$' && bytes[index + 1] == b'$' {
            let mut slashes = 0;
            while index > slashes && bytes[index - 1 - slashes] == b'\\' {
                slashes += 1;
            }
            if slashes % 2 == 0 {
                return Some(index);
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    None
}

/// Find the closing line of a `$$` block (`renderer.ts:268-308`).
fn find_block_close(
    source: &str,
    starts: &[u32],
    guard: &Protected,
    open_line: usize,
    open: usize,
    quote_depth: usize,
) -> Option<(usize, usize)> {
    let mut line = open_line;
    loop {
        let range = line_range(source, starts, line)?;
        if line > open_line {
            if guard.contains(range.start) {
                return None;
            }
            let stripped = strip_quote_markers(&source[range.clone()]);
            if stripped.quote_depth < quote_depth {
                return None;
            }
            if is_blank(&source[range.start + stripped.content..range.end]) {
                return None;
            }
        }
        let stripped = strip_quote_markers(&source[range.clone()]);
        let search_from = if line == open_line { open + 2 } else { range.start + stripped.content };
        if search_from >= range.end {
            line += 1;
            continue;
        }
        if let Some(close) = find_dollars(source, search_from, range.end) {
            // Nothing may follow the closing `$$` on its line.
            return is_blank(&source[close + 2..range.end]).then_some((close, line));
        }
        line += 1;
    }
}

/// The formula text of a `$$` block, with container prefixes stripped.
fn block_tex(
    source: &str,
    starts: &[u32],
    open_line: usize,
    indent: usize,
    open: usize,
    close: usize,
    close_line: usize,
) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if close_line == open_line {
        parts.push(&source[open + 2..close]);
    } else {
        let open_end = line_range(source, starts, open_line).map_or(source.len(), |range| range.end);
        parts.push(&source[(open + 2).min(open_end)..open_end]);
        for line in open_line + 1..close_line {
            if let Some(range) = line_range(source, starts, line) {
                let raw = &source[range.clone()];
                let stripped = strip_quote_markers(raw);
                let start = range.start + stripped.content;
                let start = strip_spaces(source, start, range.end, indent);
                parts.push(&source[start..range.end]);
            }
        }
        if let Some(range) = line_range(source, starts, close_line) {
            let raw = &source[range.clone()];
            let stripped = strip_quote_markers(raw);
            let start = range.start + stripped.content;
            let start = strip_spaces(source, start, close, indent);
            parts.push(&source[start..close]);
        }
    }
    let mut tex = parts.into_iter().filter(|part| !part.trim().is_empty()).collect::<Vec<_>>().join("\n");
    tex = tex.trim().to_owned();
    tex
}

fn strip_spaces(source: &str, mut start: usize, end: usize, count: usize) -> usize {
    let mut skipped = 0;
    while skipped < count && start < end && source.as_bytes()[start] == b' ' {
        start += 1;
        skipped += 1;
    }
    start
}

/// An inline `$$…$$` display span (`renderer.ts:310-337`).
fn inline_display(source: &str, start: usize, pos_max: usize, quote_depth: usize) -> Option<(Range<usize>, String)> {
    let bytes = source.as_bytes();
    if bytes.get(start) != Some(&b'$') || bytes.get(start + 1) != Some(&b'$') {
        return None;
    }
    let prev = start.checked_sub(1).and_then(|index| bytes.get(index)).copied();
    if prev == Some(b'$') || prev == Some(b'\\') || bytes.get(start + 2) == Some(&b'$') {
        return None;
    }
    let close = find_dollars(source, start + 2, pos_max)?;
    let after = bytes.get(close + 2).copied();
    let content = normalize_content(&source[start + 2..close], quote_depth);
    let trimmed = content.trim();
    if after == Some(b'$') || trimmed.is_empty() {
        return None;
    }
    let starts_with_space = content.starts_with([' ', '\t'])
        && content.trim_start_matches([' ', '\t']).chars().next().is_some_and(|ch| !ch.is_whitespace());
    if starts_with_space {
        return None;
    }
    let ends_with_space = content.ends_with([' ', '\t'])
        && content.trim_end_matches([' ', '\t']).chars().next_back().is_some_and(|ch| !ch.is_whitespace());
    if ends_with_space {
        return None;
    }
    Some((start..close + 2, trimmed.to_owned()))
}

/// A `$…$` or `$`…`$` inline span (`renderer.ts:339-390`).
fn inline_math(source: &str, start: usize, pos_max: usize, quote_depth: usize) -> Option<(Range<usize>, String)> {
    let close = closing_dollar(source, start, pos_max)?;
    if close <= start + 1 || close >= source.len() {
        return None;
    }
    let content = normalize_content(&source[start + 1..close], quote_depth);
    let content = if source.as_bytes().get(start + 1) == Some(&b'`') {
        content.strip_prefix('`').and_then(|text| text.strip_suffix('`')).unwrap_or(&content)
    } else {
        &content
    };
    Some((start..close + 1, content.trim().to_owned()))
}

/// Remove block-quote markers from the continuation lines of an inline span
/// so validation and TeX see the content the Markdown inline parser would.
fn normalize_content(content: &str, quote_depth: usize) -> String {
    if quote_depth == 0 || !content.contains('\n') {
        return content.to_owned();
    }
    let mut out = String::with_capacity(content.len());
    for (index, line) in content.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
            let stripped = strip_quote_markers(line);
            out.push_str(&line[stripped.content.min(line.len())..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// The block-quote depth of the line that contains `pos`.
fn quote_depth_at(source: &str, starts: &[u32], pos: usize) -> usize {
    let line = line_of(starts, pos);
    line_range(source, starts, line).map_or(0, |range| strip_quote_markers(&source[range]).quote_depth)
}

fn closing_dollar(source: &str, start: usize, pos_max: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let prev = start.checked_sub(1).and_then(|index| bytes.get(index)).copied();
    let next = (start + 1 < pos_max).then(|| bytes.get(start + 1).copied()).flatten();
    if prev == Some(b'$') || prev == Some(b'\\') || prev.is_some_and(is_word_byte) {
        return None;
    }
    let next = next?;
    if is_space_byte(next) {
        return None;
    }
    if next == b'`' {
        // The first content byte may be multibyte: start the search right after
        // the opening backtick (always a char boundary) instead of at
        // `start + 3`, which can sit inside a multibyte character.
        let content_start = start + 2;
        let limit = pos_max.min(source.len());
        if content_start <= limit
            && let Some(offset) = source[content_start..limit].find("`$")
        {
            let end = content_start + offset;
            if end > content_start && end + 1 < pos_max {
                return Some(end + 1);
            }
        }
    }
    let mut end = start + 1;
    while end < pos_max {
        let offset = source[end..pos_max].find('$')?;
        let position = end + offset;
        let mut slashes = 0;
        while position > slashes && bytes[position - 1 - slashes] == b'\\' {
            slashes += 1;
        }
        if slashes % 2 == 1 {
            end = position + 1;
            continue;
        }
        let after = bytes.get(position + 1).copied();
        let before = position.checked_sub(1).and_then(|index| bytes.get(index)).copied();
        let valid = position > start + 1
            && after != Some(b'$')
            && !after.is_some_and(is_word_byte)
            && !before.is_some_and(is_space_byte);
        return valid.then_some(position);
    }
    None
}

/// The end of an HTML tag, comment or autolink starting at `start`.
fn angle_end(source: &str, start: usize) -> Option<usize> {
    if source[start..].starts_with("<!--") {
        return source[start + 4..].find("-->").map(|offset| start + 4 + offset + 3);
    }
    let next = source.as_bytes().get(start + 1).copied()?;
    if !(next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?')) {
        return None;
    }
    source[start + 1..].find('>').map(|offset| start + 1 + offset + 1)
}

/// The end of a reference definition line (`[label]: …`) at `start`, when the
/// definition begins the line.
fn reference_definition_end(source: &str, starts: &[u32], start: usize) -> Option<usize> {
    let line = line_of(starts, start);
    let range = line_range(source, starts, line)?;
    let prefix = &source[range.start..start];
    if prefix.len() > 3 || !prefix.bytes().all(|byte| byte == b' ') || !source[start..].starts_with('[') {
        return None;
    }
    let close = source[start + 1..range.end].find(']')? + start + 1;
    (source.as_bytes().get(close + 1) == Some(&b':')).then_some(range.end)
}

/// The end of a link destination `(…)`, parentheses balanced.
fn link_destination_end(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut depth = 1usize;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return index;
                }
            }
            b'\n' if index + 1 < bytes.len() && bytes[index + 1] == b'\n' => return index,
            _ => index += 1,
        }
    }
    bytes.len()
}

/// The byte after the paragraph that contains `pos`: the next blank line, or
/// the next line that starts a different block.
fn paragraph_end(source: &str, starts: &[u32], pos: usize, guard: &Protected) -> usize {
    let mut line = line_of(starts, pos);
    loop {
        let Some(range) = line_range(source, starts, line + 1) else {
            return source.len();
        };
        if guard.at(range.start).is_some() {
            return range.start;
        }
        let raw = &source[range.clone()];
        let stripped = strip_quote_markers(raw);
        let content_start = range.start + stripped.content;
        if is_blank(&source[content_start..range.end]) {
            return range.start;
        }
        let content = &source[content_start..range.end];
        if starts_block(content) {
            return range.start;
        }
        line += 1;
    }
}

fn starts_block(content: &str) -> bool {
    let trimmed = content.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return true;
    }
    if trimmed.starts_with('|') {
        return true;
    }
    if trimmed.starts_with('>') {
        return false;
    }
    if is_atx_heading(trimmed) || is_thematic_break(trimmed) || is_setext_underline(trimmed) {
        return true;
    }
    if trimmed.starts_with("[^") && trimmed.contains("]:") {
        return true;
    }
    list_marker_end(trimmed.as_bytes(), 0).is_some()
}

fn is_atx_heading(text: &str) -> bool {
    let hashes = text.bytes().take_while(|byte| *byte == b'#').count();
    (1..=6).contains(&hashes) && text.as_bytes().get(hashes).is_none_or(|byte| *byte == b' ' || *byte == b'\t')
}

fn is_setext_underline(text: &str) -> bool {
    let text = text.trim_end();
    (text.len() >= 2 && text.bytes().all(|byte| byte == b'='))
        || (text.len() >= 2 && text.bytes().all(|byte| byte == b'-'))
}

fn is_thematic_break(text: &str) -> bool {
    let mut symbol = None;
    let mut count = 0;
    for byte in text.bytes() {
        match byte {
            b' ' | b'\t' => {}
            b'-' | b'*' | b'_' => {
                if symbol.is_some_and(|current| current != byte) {
                    return false;
                }
                symbol = Some(byte);
                count += 1;
            }
            _ => return false,
        }
    }
    count >= 3
}

/// Blank a `$$` block line by line, keeping container prefixes (indentation,
/// `>` markers) and newlines intact, then put U+FFFC on the first content
/// region that has room for it.
#[allow(clippy::too_many_arguments)]
fn write_block_placeholder(
    out: &mut [u8],
    source: &str,
    starts: &[u32],
    open_line: usize,
    open: usize,
    close: usize,
    close_line: usize,
    indent: usize,
) -> Option<usize> {
    let mut regions: Vec<(usize, usize)> = Vec::new();
    if close_line == open_line {
        regions.push((open, close + 2));
    } else {
        let open_end = line_range(source, starts, open_line).map_or(close + 2, |range| range.end);
        regions.push((open, open_end));
        for line in open_line + 1..close_line {
            if let Some(range) = line_range(source, starts, line) {
                let raw = &source[range.clone()];
                let stripped = strip_quote_markers(raw);
                let start = strip_spaces(source, range.start + stripped.content, range.end, indent);
                regions.push((start, range.end));
            }
        }
        if let Some(range) = line_range(source, starts, close_line) {
            let raw = &source[range.clone()];
            let stripped = strip_quote_markers(raw);
            let start = strip_spaces(source, range.start + stripped.content, close, indent);
            regions.push((start, close + 2));
        }
    }
    for (start, end) in &regions {
        for byte in &mut out[*start..*end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    const PLACEHOLDER: [u8; 3] = [0xEF, 0xBF, 0xBC];
    if let Some((start, _)) = regions.iter().find(|(start, end)| end.saturating_sub(*start) >= 3) {
        out[*start..*start + 3].copy_from_slice(&PLACEHOLDER);
        return Some(*start);
    }
    // Shorter than three bytes: a one-byte sentinel keeps the newline and the
    // container prefix exact; the inline builder renders it as U+FFFC.
    if let Some((start, _)) = regions.iter().find(|(start, end)| end > start) {
        out[*start] = SHORT_OBJECT as u8;
        return Some(*start);
    }
    None
}

/// Write the placeholder for an inline span, keeping every newline.
///
/// A single-line formula gets U+FFFC at its start. A multi-line `$$…$$` gets
/// the placeholder on its first line and a one-byte filler on each following
/// line (after the quote/list prefix), so blanked lines do not end the
/// paragraph or the quote.
fn write_inline_span(out: &mut [u8], source: &str, starts: &[u32], range: Range<usize>) -> Option<usize> {
    const PLACEHOLDER: [u8; 3] = [0xEF, 0xBF, 0xBC];
    if range.end > out.len() || range.is_empty() {
        return None;
    }
    if !source[range.clone()].contains('\n') {
        let clear = range.start + 3 <= range.end && !out[range.start..range.start + 3].contains(&b'\n');
        for byte in &mut out[range.clone()] {
            *byte = b' ';
        }
        if clear {
            out[range.start..range.start + 3].copy_from_slice(&PLACEHOLDER);
        } else {
            out[range.start] = SHORT_OBJECT as u8;
        }
        return Some(range.start);
    }

    let start_line = line_of(starts, range.start);
    let end_line = line_of(starts, range.end.saturating_sub(1));
    let mut regions: Vec<(usize, usize)> = Vec::new();
    if let Some(first) = line_range(source, starts, start_line) {
        regions.push((range.start, first.end.min(range.end)));
    }
    for line in start_line + 1..=end_line {
        let Some(full) = line_range(source, starts, line) else { break };
        let stripped = strip_quote_markers(&source[full.clone()]);
        let start = full.start + stripped.content;
        let end = if line == end_line { range.end } else { full.end };
        if start < end {
            regions.push((start, end));
        }
    }
    for (start, end) in &regions {
        for byte in &mut out[*start..*end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    let clear = range.start + 3 <= range.end && !out[range.start..range.start + 3].contains(&b'\n');
    if clear {
        out[range.start..range.start + 3].copy_from_slice(&PLACEHOLDER);
    } else {
        out[range.start] = SHORT_OBJECT as u8;
    }
    // Keep continuation lines non-blank without rendering anything: the
    // inline builder drops these fillers together with the formula padding.
    for (start, _) in regions.iter().skip(1) {
        out[*start] = PADDING_OBJECT as u8;
    }
    Some(range.start)
}

struct QuoteStrip {
    content: usize,
    quote_depth: usize,
}

/// Strip repeated block-quote markers from a line.
fn strip_quote_markers(line: &str) -> QuoteStrip {
    let bytes = line.as_bytes();
    let mut pos = 0;
    let mut quote_depth = 0;
    loop {
        let mut spaces = 0;
        while pos + spaces < bytes.len() && (bytes[pos + spaces] == b' ' || bytes[pos + spaces] == b'\t') {
            spaces += 1;
        }
        if pos + spaces < bytes.len() && bytes[pos + spaces] == b'>' {
            quote_depth += 1;
            pos += spaces + 1;
            if pos < bytes.len() && bytes[pos] == b' ' {
                pos += 1;
            }
            continue;
        }
        return QuoteStrip { content: pos + spaces, quote_depth };
    }
}

pub(crate) struct Container {
    pub(crate) content: usize,
    pub(crate) indent: usize,
    pub(crate) quote_depth: usize,
    pub(crate) list_inside_quote: bool,
}

/// Strip block-quote markers and list markers from the first line of a block.
pub(crate) fn strip_containers(line: &str) -> Container {
    let bytes = line.as_bytes();
    let mut pos = 0;
    let mut quote_depth = 0;
    let mut list_inside_quote = false;
    loop {
        let start = pos;
        while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
            pos += 1;
        }
        if pos < bytes.len() && bytes[pos] == b'>' {
            quote_depth += 1;
            pos += 1;
            if pos < bytes.len() && bytes[pos] == b' ' {
                pos += 1;
            }
            continue;
        }
        if let Some(after) = list_marker_end(bytes, pos) {
            if quote_depth > 0 {
                list_inside_quote = true;
            }
            pos = after;
            continue;
        }
        return Container { content: pos, indent: pos - start, quote_depth, list_inside_quote };
    }
}

/// After a bullet (`-`, `+`, `*`) or ordered (`1.`, `1)`) marker and its space.
fn list_marker_end(bytes: &[u8], pos: usize) -> Option<usize> {
    let byte = bytes.get(pos).copied()?;
    if matches!(byte, b'-' | b'+' | b'*') {
        let mut next = pos + 1;
        if bytes.get(next).is_some_and(|byte| *byte == b' ' || *byte == b'\t') {
            while bytes.get(next).is_some_and(|byte| *byte == b' ' || *byte == b'\t') {
                next += 1;
            }
            return Some(next);
        }
        return None;
    }
    if byte.is_ascii_digit() {
        let mut digits = pos;
        while bytes.get(digits).is_some_and(|byte| byte.is_ascii_digit()) && digits - pos < 9 {
            digits += 1;
        }
        if digits > pos
            && matches!(bytes.get(digits).copied(), Some(b'.' | b')'))
            && bytes.get(digits + 1).is_some_and(|byte| *byte == b' ')
        {
            let mut next = digits + 2;
            while bytes.get(next).is_some_and(|byte| *byte == b' ' || *byte == b'\t') {
                next += 1;
            }
            return Some(next);
        }
    }
    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_space_byte(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OBJECT_CHAR;

    fn scan_plain(source: &str) -> MathScan {
        crate::scan_math(source)
    }

    fn texts(source: &str) -> Vec<String> {
        scan_plain(source).spans.into_iter().map(|span| span.tex).collect()
    }

    #[test]
    fn inline_math_follows_github_rules() {
        assert_eq!(texts("a $x$ b"), ["x"]);
        assert!(texts("$ x $").is_empty());
        assert!(texts("$5 and $10").is_empty());
        assert!(texts("\\$x\\$").is_empty());
        assert_eq!(texts("$a$ and $b$"), ["a", "b"]);
        assert_eq!(texts("US$5 to $x$"), ["x"]);
        assert_eq!(texts("$`\\sqrt{2}`$"), ["\\sqrt{2}"]);
        assert_eq!(texts("$`\\text{for all $\\alpha$}`$"), ["\\text{for all $\\alpha$}"]);
        assert!(texts("$a$$b$").len() <= 1);
    }

    #[test]
    fn formulas_in_different_paragraphs_are_all_found() {
        // The cached paragraph end must be recomputed at every boundary
        // (OI-143): a blank line, a block start, and the end of the source.
        assert_eq!(texts("$a$\n\n$b$\n\n$c$"), ["a", "b", "c"]);
        assert_eq!(texts("one $a$ two\nthree $b$ four\n\nsecond $c$"), ["a", "b", "c"]);
        assert_eq!(texts("before $a$\n# Heading\nafter $b$"), ["a", "b"]);
        assert_eq!(texts("- item $a$\n- item $b$\n\n$c$"), ["a", "b", "c"]);
    }

    #[test]
    fn display_blocks_need_a_clean_close() {
        assert_eq!(texts("$$\nx\n$$"), ["x"]);
        assert_eq!(texts("$$E = mc^2$$"), ["E = mc^2"]);
        assert!(texts("$$ 100 l'uno").is_empty());
        assert!(texts("$$ 100 l'uno, oppure $$ 50 per due.").is_empty());
        assert!(texts("$$ a\n\nb\n$$").is_empty());
        assert_eq!(texts("$$x$$ and text"), ["x"]);
        assert!(texts("$$$$").is_empty());
        assert_eq!(texts("$$\n\\alpha\n$$\n$$\n\\beta\n$$"), ["\\alpha", "\\beta"]);
    }

    #[test]
    fn display_math_in_sentences() {
        let result = scan_plain("The famous $$E = mc^2$$ formula");
        assert_eq!(result.spans.len(), 1);
        assert!(result.spans[0].display);
        let result = scan_plain("Two in a row: $$a$$ $$b$$");
        assert_eq!(result.spans.len(), 2);
    }

    #[test]
    fn code_and_comments_are_left_alone() {
        assert!(texts("```\n$$x$$\n```").is_empty());
        assert!(texts("`$x$`").is_empty());
        assert!(texts("<!-- $x$ -->").is_empty());
        assert!(texts("[x](http://a/$b$)").is_empty());
        assert!(texts("<span>$</span>5 for $n$ apples").len() == 1);
    }

    #[test]
    fn placeholder_is_same_length_and_keeps_lines() {
        let source = "a $x$ b\n\n$$\ny^2\n$$";
        let result = scan_plain(source);
        assert_eq!(result.text.len(), source.len());
        assert_eq!(result.text.matches('\n').count(), source.matches('\n').count());
        assert_eq!(result.text.matches(OBJECT_CHAR).count(), 2);
    }

    #[test]
    fn short_block_math_still_gets_a_placeholder() {
        let source = "$$\ny\n$$";
        let result = scan_plain(source);
        assert_eq!(result.text.len(), source.len());
        assert_eq!(result.spans.len(), 1);
        assert_eq!(result.placeholders[0], result.spans[0].source.start);
    }

    #[test]
    fn lists_and_quotes_keep_their_markers() {
        assert_eq!(texts("- first\n\n  $$\n  x\n  $$\n"), ["x"]);
        assert_eq!(texts("> A quote\n> $$\n> z = 3\n> $$\n> and more."), ["z = 3"]);
    }
}
