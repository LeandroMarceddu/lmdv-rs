//! Flat YAML front matter, ported from the original app's `src/frontmatter.ts`.
//!
//! Only the shapes real front matter uses are parsed: `key: value`,
//! `key: [a, b]`, `key:` plus indented `- item` lines, quoted strings and
//! trailing comments. Anything else marks the block invalid and keeps the
//! raw text, which the renderer shows verbatim.

use crate::model::{FmValue, FrontMatter};
use crate::text::{line_range, line_starts};

/// The front matter found at the start of a document, and the parse input
/// with its lines blanked so every later offset and line number is unchanged.
pub(crate) struct FrontMatterScan {
    /// The parsed block, when the document starts with one.
    pub front_matter: Option<FrontMatter>,
    /// The parse input.
    pub text: String,
}

/// Find and strip the leading front matter.
pub(crate) fn scan(source: &str) -> FrontMatterScan {
    let starts = line_starts(source);
    let Some(first) = line_range(source, &starts, 0) else {
        return FrontMatterScan { front_matter: None, text: source.to_owned() };
    };
    let first_text = &source[first.clone()];
    // A UTF-8 BOM is stripped by the loader; tolerate one anyway.
    let first_text = first_text.strip_prefix('\u{feff}').unwrap_or(first_text);
    if first_text.trim_end() != "---" {
        return FrontMatterScan { front_matter: None, text: source.to_owned() };
    }

    let mut close_line = None;
    let mut line = 1;
    while let Some(range) = line_range(source, &starts, line) {
        let text = source[range.clone()].trim_end();
        let dashes = text.bytes().take_while(|byte| *byte == b'-').count();
        let is_close = (dashes >= 3 && dashes == text.len()) || text == "...";
        if is_close {
            close_line = Some((line, range));
            break;
        }
        line += 1;
    }

    let (content, span) = match close_line {
        Some((_, range)) => {
            let content_start = line_range(source, &starts, 0).map_or(0, |first| first.end + 1);
            let end = source[range.end..].find('\n').map_or(source.len(), |offset| range.end + offset + 1);
            (content_start..range.start, 0..end)
        }
        None => {
            // Unclosed: the block runs to EOF (M-12).
            let content_start = first.end.min(source.len());
            let content_start = if content_start < source.len() { content_start + 1 } else { content_start };
            (content_start..source.len(), 0..source.len())
        }
    };

    let raw = source[content.clone()].trim_end_matches('\n').to_owned();
    let (entries, valid) = match parse_flat(&raw) {
        Some(entries) if !entries.is_empty() => (entries, true),
        _ => (Vec::new(), false),
    };

    let mut text = String::with_capacity(source.len());
    text.push_str(&source[..span.start]);
    for byte in source[span.clone()].bytes() {
        text.push(if byte == b'\n' { '\n' } else { ' ' });
    }
    text.push_str(&source[span.end..]);
    FrontMatterScan { front_matter: Some(FrontMatter { raw, entries, valid }), text }
}

/// Parse the flat subset; `None` means the YAML is not flat (or is odd).
fn parse_flat(yaml: &str) -> Option<Vec<(String, FmValue)>> {
    let lines: Vec<&str> = yaml.split('\n').collect();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            index += 1;
            continue;
        }
        let (key, rest) = split_key(line)?;
        let rest = rest.trim();
        index += 1;
        if rest.is_empty() || rest == "|" || rest == ">" {
            let mut items = Vec::new();
            while index < lines.len() {
                let Some(item) = list_item(lines[index]) else { break };
                items.push(unquote(item));
                index += 1;
            }
            if items.is_empty() {
                return None;
            }
            entries.push((key, FmValue::List(items)));
        } else if rest.starts_with('[') && rest.ends_with(']') {
            let inner = rest[1..rest.len() - 1].trim();
            let items = if inner.is_empty() { Vec::new() } else { inner.split(',').map(unquote).collect() };
            entries.push((key, FmValue::List(items)));
        } else if rest.starts_with('{') {
            return None;
        } else {
            entries.push((key, FmValue::Scalar(unquote(strip_trailing_comment(rest)))));
        }
    }
    Some(entries)
}

/// `^([A-Za-z0-9_.-]+)\s*:(.*)$`, without the regex.
fn split_key(line: &str) -> Option<(String, &str)> {
    if line.starts_with([' ', '\t']) {
        return None;
    }
    let colon = line.find(':')?;
    let key = line[..colon].trim_end();
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')) {
        return None;
    }
    Some((key.to_owned(), &line[colon + 1..]))
}

/// `^\s+-\s*` followed by an item.
fn list_item(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    if trimmed.len() == line.len() {
        return None;
    }
    let rest = trimmed.strip_prefix('-')?;
    Some(rest.trim_start_matches([' ', '\t']))
}

/// JS `rest.replace(/\s+#.*$/, "")`.
fn strip_trailing_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    for index in 1..bytes.len() {
        if bytes[index] == b'#' && matches!(bytes[index - 1], b' ' | b'\t') {
            return value[..index].trim_end();
        }
    }
    value
}

/// JS `unquote`: trim, then drop one matching quote on each side.
fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
        {
            return trimmed[1..trimmed.len() - 1].to_owned();
        }
    }
    trimmed.to_owned()
}
