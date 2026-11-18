//! GitHub alerts (`> [!NOTE]` and friends), as the original app's
//! `markdown-it-github-alerts` plugin parses them.
//!
//! The marker and an optional custom title are blanked out (same length,
//! newlines kept) before the Markdown parse, so a title never becomes text and
//! the quote body is parsed normally.

use crate::math::strip_containers;
use crate::model::{Callout, CalloutKind};
use crate::text::{Protected, is_blank, line_range, line_starts};

/// One alert found before the parse.
#[derive(Clone, Debug)]
pub(crate) struct CalloutSpan {
    /// The 0-based line the quote starts on.
    pub line: usize,
    /// The quote nesting depth (1 for a top-level quote).
    pub depth: usize,
    /// The parsed alert.
    pub callout: Callout,
}

/// Find alerts and return the parse input with their marker lines blanked.
pub(crate) fn scan(source: &str, protected: &Protected) -> (Vec<CalloutSpan>, String) {
    let starts = line_starts(source);
    let mut found = Vec::new();
    let mut text = source.as_bytes().to_vec();
    // Whether each open quote level has had content yet. The marker is only an
    // alert when it opens the quote's first paragraph.
    let mut frames: Vec<bool> = Vec::new();
    let mut line = 0;
    while let Some(range) = line_range(source, &starts, line) {
        let raw = &source[range.clone()];
        let container = strip_containers(raw);
        let depth = container.quote_depth;
        if depth == 0 {
            frames.clear();
            line += 1;
            continue;
        }
        if frames.len() > depth {
            frames.truncate(depth);
        }
        while frames.len() < depth {
            frames.push(false);
        }
        if protected.overlaps(range.clone()) {
            frames[..depth].fill(true);
            line += 1;
            continue;
        }
        let content_start = range.start + container.content;
        let content = &source[content_start..range.end];
        let blank = is_blank(content);
        let first_paragraph = !frames[depth - 1];
        if !blank
            && first_paragraph
            && !container.list_inside_quote
            && let Some((kind, title)) = parse_marker(content)
        {
            found.push(CalloutSpan { line, depth, callout: Callout { kind, title } });
            for byte in &mut text[content_start..range.end] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
        }
        if !blank {
            frames[..depth].fill(true);
        }
        line += 1;
    }

    match String::from_utf8(text) {
        Ok(text) => (found, text),
        Err(_) => (found, source.to_owned()),
    }
}

fn parse_marker(content: &str) -> Option<(CalloutKind, Option<String>)> {
    const KINDS: [CalloutKind; 5] =
        [CalloutKind::Note, CalloutKind::Tip, CalloutKind::Important, CalloutKind::Warning, CalloutKind::Caution];
    for kind in KINDS {
        let marker = format!("[!{}]", kind.marker());
        if content.len() < marker.len() || !content.as_bytes()[..marker.len()].eq_ignore_ascii_case(marker.as_bytes()) {
            continue;
        }
        let rest = &content[marker.len()..];
        if rest.is_empty() || rest.starts_with([' ', '\t']) {
            let title = rest.trim();
            return Some((kind, (!title.is_empty()).then(|| title.to_owned())));
        }
    }
    None
}
