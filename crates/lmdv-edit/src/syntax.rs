//! Markdown source highlighting for the editor.
//!
//! [`rune_kit::syntax`] runs the same highlights lexers as the reader's code
//! blocks. Below [`SYNTAX_MAX_BYTES`] the whole text is highlighted; above it
//! only the visible window is, expanded to line boundaries and capped at the
//! same 128 KiB. A window cut after a fence opened earlier does not see that
//! fence's language; the app can highlight a whole fenced block when it knows
//! the boundaries. A [`SyntaxCache`] keys the result by the session revision
//! and theme so a frame that changes nothing does not re-highlight.

use std::ops::Range;

use rune::Span;
use rune::TextEdit;
use rune_kit::syntax::SyntaxTheme;

/// Largest source the editor highlights; above this only the visible window
/// is coloured (E-05 performance guardrail).
pub const SYNTAX_MAX_BYTES: usize = 128 * 1024;

/// The language name the Markdown source is lexed as.
const LANGUAGE: &str = "markdown";

/// The spans for `edit`'s text, in byte coordinates of the whole text.
///
/// `visible_range` is the editor viewport, used only when the text is above
/// [`SYNTAX_MAX_BYTES`]; `None` there leaves the text plain.
pub fn syntax_spans(edit: &TextEdit, theme: &SyntaxTheme, visible_range: Option<Range<usize>>) -> Vec<Span> {
    let text = &edit.text;
    if text.is_empty() {
        return Vec::new();
    }
    if text.len() <= SYNTAX_MAX_BYTES {
        let found = rune_kit::syntax::highlight(Some(LANGUAGE), text);
        return theme.spans(&found, 0);
    }
    let Some(visible) = visible_range else {
        return Vec::new();
    };
    let (start, end) = window(text, visible);
    if start >= end {
        return Vec::new();
    }
    let found = rune_kit::syntax::highlight(Some(LANGUAGE), &text[start..end]);
    theme.spans(&found, start)
}

/// Editor-side cache of [`syntax_spans`] keyed by text revision and theme.
#[derive(Clone, Debug, Default)]
pub struct SyntaxCache {
    key: Option<CacheKey>,
    spans: Vec<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CacheKey {
    revision: u64,
    theme: SyntaxTheme,
    start: Option<usize>,
    end: Option<usize>,
}

impl SyntaxCache {
    /// Spans for the current revision; recomputes only when the revision,
    /// theme or (for large documents) the window changed. Documents at or
    /// below the limit always use the whole text, so scrolling does not
    /// invalidate them.
    pub fn spans(
        &mut self,
        revision: u64,
        edit: &TextEdit,
        theme: SyntaxTheme,
        visible: Option<Range<usize>>,
    ) -> &[Span] {
        let effective = if edit.text.len() <= SYNTAX_MAX_BYTES { None } else { visible };
        let (start, end) = match &effective {
            Some(range) => (Some(range.start), Some(range.end)),
            None => (None, None),
        };
        let key = CacheKey { revision, theme, start, end };
        if self.key != Some(key) {
            self.spans = syntax_spans(edit, &theme, effective);
            self.key = Some(key);
        }
        &self.spans
    }

    /// Drops the cached spans, for a theme switch or a reload.
    pub fn clear(&mut self) {
        self.key = None;
        self.spans.clear();
    }
}

fn window(text: &str, visible: Range<usize>) -> (usize, usize) {
    let len = text.len();
    let mut start = text.floor_char_boundary(visible.start.min(len));
    let mut end = text.floor_char_boundary(visible.end.min(len));
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }

    start = text[..start].rfind('\n').map_or(0, |offset| offset + 1);
    if end < len {
        end = text[end..].find('\n').map_or(len, |offset| end + offset + 1);
    }

    if end.saturating_sub(start) > SYNTAX_MAX_BYTES {
        let cap = text.floor_char_boundary((start + SYNTAX_MAX_BYTES).min(len));
        let cut = text[start..cap].rfind('\n').map_or(cap, |offset| start + offset + 1);
        end = cut.max(start);
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> SyntaxTheme {
        SyntaxTheme::neutral(true)
    }

    fn valid(text: &str, spans: &[Span]) {
        for span in spans {
            let start = span.start as usize;
            let end = span.end as usize;
            assert!(start < end, "empty span {start}..{end}");
            assert!(end <= text.len(), "span past the end: {start}..{end} of {}", text.len());
            assert!(text.is_char_boundary(start), "span starts inside a character: {start}");
            assert!(text.is_char_boundary(end), "span ends inside a character: {end}");
        }
        for pair in spans.windows(2) {
            assert!(pair[0].end <= pair[1].start, "spans overlap");
        }
    }

    #[test]
    fn small_markdown_highlights_the_whole_text() {
        let edit = TextEdit::new("# Title\n\nA paragraph with `code`.\n");
        let spans = syntax_spans(&edit, &theme(), None);
        assert!(!spans.is_empty());
        valid(&edit.text, &spans);
    }

    #[test]
    fn empty_text_has_no_spans() {
        assert!(syntax_spans(&TextEdit::new(""), &theme(), None).is_empty());
    }

    #[test]
    fn large_text_without_a_window_is_plain() {
        let text = format!("# Heading\n{}", "line\n".repeat(40_000));
        assert!(text.len() > SYNTAX_MAX_BYTES);
        let edit = TextEdit::new(text);
        assert!(syntax_spans(&edit, &theme(), None).is_empty());
    }

    #[test]
    fn large_text_highlights_only_the_window_and_shifts_ranges() {
        let mut text = String::from("# Heading\n");
        text.push_str(&"filler line\n".repeat(20_000));
        let heading_at = 0;
        let later = text.len();
        text.push_str("## Later\n");
        assert!(text.len() > SYNTAX_MAX_BYTES);

        let edit = TextEdit::new(text.clone());
        let spans = syntax_spans(&edit, &theme(), Some(heading_at..heading_at + 4));
        assert!(!spans.is_empty());
        valid(&text, &spans);
        assert!(spans.iter().all(|span| (span.start as usize) < 4096), "only the first window is highlighted");

        let later_spans = syntax_spans(&edit, &theme(), Some(later..later + 3));
        assert!(!later_spans.is_empty());
        valid(&text, &later_spans);
        assert!(
            later_spans.iter().all(|span| (span.start as usize) >= later.saturating_sub(1)),
            "ranges are shifted to the whole text"
        );
    }

    #[test]
    fn a_window_without_line_breaks_is_capped_at_the_limit() {
        let text = "x".repeat(SYNTAX_MAX_BYTES * 2);
        let edit = TextEdit::new(text.clone());
        let spans = syntax_spans(&edit, &theme(), Some(10..20));
        valid(&text, &spans);
    }

    #[test]
    fn the_cache_invalidates_on_revision_theme_and_window() {
        let edit = TextEdit::new("# Title\n");
        let mut cache = SyntaxCache::default();
        let first = cache.spans(1, &edit, theme(), None).to_vec();
        assert_eq!(cache.spans(1, &edit, theme(), None), first.as_slice());
        let second = cache.spans(2, &edit, theme(), None).to_vec();
        assert_eq!(second, first);
        let light = cache.spans(2, &edit, SyntaxTheme::neutral(false), None).to_vec();
        assert_eq!(light.len(), first.len());
        cache.clear();
        let after_clear = cache.spans(2, &edit, theme(), None).to_vec();
        assert_eq!(after_clear, first);
    }

    #[test]
    fn small_documents_ignore_the_window_for_caching() {
        let edit = TextEdit::new("# Title\n");
        let mut cache = SyntaxCache::default();
        let first = cache.spans(1, &edit, theme(), Some(0..1)).to_vec();
        let second = cache.spans(1, &edit, theme(), Some(5..6)).to_vec();
        assert_eq!(first, second);
    }
}
