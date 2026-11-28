//! A bounded Markdown syntax tree for live editor gating.
//!
//! [`crate::syntax::SyntaxCache`] caches *painted spans* by revision, but the
//! format bar and the `/` menu need a different question: is the caret inside a
//! fence, an indented code block, an HTML block or an inline code span? Reading
//! that from a debounced `MdDoc` lags the caret, and re-lexing the whole buffer
//! on every frame would break the 128 KiB guardrail (E-05, OI-061).
//!
//! This module keeps a small tree of the ranges that matter to those queries.
//! A [`MarkdownContext`] covers only a bounded window around the caret; it is
//! rebuilt only when the field's revision or its window changes, so moving the
//! caret inside one window reuses the same scan and queries it at the new
//! offset. A scan never touches more than the 128 KiB cap (a small field is
//! covered whole, exactly as the highlighter does).

use std::cell::RefCell;
use std::ops::Range;

use crate::syntax::SYNTAX_MAX_BYTES;

/// Largest source a single context scan touches (the editor's syntax cap).
pub const CONTEXT_WINDOW_BYTES: usize = SYNTAX_MAX_BYTES;

/// The bounded syntax tree around a caret, in whole-field byte coordinates.
///
/// Every range is half-open and sits inside [`Self::window`]. Queries are
/// `O(ranges)`, so a frame costs a handful of comparisons, not a parse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarkdownContext {
    window: Range<usize>,
    fences: Vec<Range<usize>>,
    indented: Vec<Range<usize>>,
    html: Vec<Range<usize>>,
    inline_code: Vec<Range<usize>>,
}

impl MarkdownContext {
    /// The source range the scan covered.
    pub fn window(&self) -> Range<usize> {
        self.window.clone()
    }

    /// The caret is inside a fenced code block (including its fences).
    pub fn in_fence(&self, offset: usize) -> bool {
        contains(&self.fences, offset)
    }

    /// The caret is inside an indented code block.
    pub fn in_indented_code(&self, offset: usize) -> bool {
        contains(&self.indented, offset)
    }

    /// The caret is inside an HTML block.
    pub fn in_html(&self, offset: usize) -> bool {
        contains(&self.html, offset)
    }

    /// The caret is inside an inline code span.
    pub fn in_inline_code(&self, offset: usize) -> bool {
        contains(&self.inline_code, offset)
    }

    /// Any kind of code: fence, indented block or inline span.
    pub fn in_code(&self, offset: usize) -> bool {
        self.in_fence(offset) || self.in_indented_code(offset) || self.in_inline_code(offset)
    }

    /// Nothing in the window needed a range.
    pub fn is_empty(&self) -> bool {
        self.fences.is_empty() && self.indented.is_empty() && self.html.is_empty() && self.inline_code.is_empty()
    }
}

fn contains(ranges: &[Range<usize>], offset: usize) -> bool {
    ranges.iter().any(|range| range.start <= offset && offset < range.end)
}

/// The bounded, line-aligned window a caret is scanned in.
///
/// The caret's own line is always covered; the remaining budget grows upward
/// first (to see a fence opener) and then downward. A pathological single line
/// larger than the cap is still scanned from its start, so the caret is never
/// left outside its own window.
fn context_window(text: &str, caret: usize, window_bytes: usize) -> Range<usize> {
    let len = text.len();
    let caret = caret.min(len);
    let line_start = text[..caret].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[caret..].find('\n').map_or(len, |i| caret + i + 1);
    let cap = window_bytes.max(1);
    // At or below the cap the whole field is cheap and gives a complete tree,
    // just like the highlighter. Only a large field needs a window.
    if len <= cap {
        return 0..len;
    }
    let mut start = line_start;
    let half = cap / 2;
    if start > 0 {
        let target = start.saturating_sub(half);
        // Align forward to a line start so the window never overshoots its
        // budget, even when one huge line separates us from the target.
        start = text[target..].find('\n').map_or(line_start, |i| target + i + 1).min(line_start);
    }
    let mut end = line_end;
    let remaining = cap.saturating_sub(end.saturating_sub(start));
    if remaining > 0 && end < len {
        let bound = text.floor_char_boundary((end + remaining).min(len));
        end = text[..bound].rfind('\n').map_or(bound, |i| i + 1).max(line_end).min(len);
    }
    if end.saturating_sub(start) > cap {
        let bound = text.floor_char_boundary((start + cap).min(len));
        end = text[..bound].rfind('\n').map_or(bound, |i| i + 1).max(line_end).min(len);
    }
    start..end
}

/// Builds the bounded syntax tree for `caret` in `text`.
pub fn markdown_context(text: &str, caret: usize, window_bytes: usize) -> MarkdownContext {
    let window = context_window(text, caret, window_bytes);
    let (start, end) = (window.start, window.end);
    let mut ctx = MarkdownContext { window, ..MarkdownContext::default() };

    let mut fence: Option<(u8, usize)> = None;
    let mut fence_start = 0usize;
    let mut indent_start: Option<usize> = None;
    let mut html_start: Option<usize> = None;

    let mut pos = start;
    while pos < end {
        let line_end = text[pos..end].find('\n').map_or(end, |i| pos + i + 1);
        let line = &text[pos..line_end];
        let body = line.strip_suffix('\n').unwrap_or(line);
        let spaces = body.bytes().take_while(|&b| b == b' ').count();
        let indent = body.bytes().take_while(|&b| b == b' ' || b == b'\t').count();
        let trimmed = body.trim_start_matches(' ');
        let blank = body.trim().is_empty();

        if let Some((marker, run)) = fence {
            if spaces <= 3 && is_closing_fence(trimmed, marker, run) {
                ctx.fences.push(fence_start..line_end);
                fence = None;
            }
        } else if indent >= 4 && !blank {
            // A fence opener cannot be indented this far; the line is code.
            if let Some(from) = html_start.take() {
                ctx.html.push(from..pos);
            }
            indent_start.get_or_insert(pos);
        } else {
            if let Some(from) = indent_start.take() {
                ctx.indented.push(from..pos);
            }
            if let Some(from) = html_start {
                if blank || html_closes(trimmed) {
                    ctx.html.push(from..line_end);
                    html_start = None;
                }
            } else if spaces <= 3 && html_opens(trimmed) {
                html_start = Some(pos);
            } else if spaces <= 3 {
                if let Some((marker, run)) = fence_open(trimmed) {
                    fence = Some((marker, run));
                    fence_start = pos;
                } else if pos > start && closing_fence(trimmed).is_some() {
                    // The window opened inside a fence whose closer is here.
                    ctx.fences.push(start..line_end);
                }
            }
        }
        pos = line_end;
    }
    if fence.is_some() {
        ctx.fences.push(fence_start..end);
    }
    if let Some(from) = indent_start {
        ctx.indented.push(from..end);
    }
    if let Some(from) = html_start {
        ctx.html.push(from..end);
    }

    ctx.inline_code = inline_code_spans(text, start, end);
    ctx
}

fn fence_open(trimmed: &str) -> Option<(u8, usize)> {
    let (marker, run) = closing_fence(trimmed)?;
    // A backtick fence's info string cannot contain a backtick (CommonMark).
    if marker == b'`' && trimmed[run..].contains('`') {
        return None;
    }
    Some((marker, run))
}

fn closing_fence(trimmed: &str) -> Option<(u8, usize)> {
    let marker = *trimmed.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let run = trimmed.bytes().take_while(|&b| b == marker).count();
    (run >= 3).then_some((marker, run))
}

fn is_closing_fence(trimmed: &str, marker: u8, run: usize) -> bool {
    let count = trimmed.bytes().take_while(|&b| b == marker).count();
    count >= run && trimmed[count..].trim().is_empty()
}

fn html_opens(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    let Some(after) = lower.strip_prefix('<') else { return false };
    if after.starts_with('!') || after.starts_with('?') || after.starts_with('/') {
        return true;
    }
    const BLOCK_TAGS: &[&str] = &[
        "address",
        "article",
        "aside",
        "base",
        "basefont",
        "blockquote",
        "body",
        "caption",
        "center",
        "col",
        "colgroup",
        "dd",
        "details",
        "dialog",
        "dir",
        "div",
        "dl",
        "dt",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "frame",
        "frameset",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "head",
        "header",
        "hr",
        "html",
        "iframe",
        "legend",
        "li",
        "link",
        "main",
        "menu",
        "menuitem",
        "nav",
        "noframes",
        "ol",
        "optgroup",
        "option",
        "p",
        "param",
        "pre",
        "script",
        "search",
        "section",
        "style",
        "summary",
        "table",
        "tbody",
        "td",
        "textarea",
        "tfoot",
        "th",
        "thead",
        "title",
        "tr",
        "track",
        "ul",
    ];
    let tag = after.split(|ch: char| !ch.is_ascii_alphanumeric()).next().unwrap_or("");
    BLOCK_TAGS.contains(&tag)
}

fn html_closes(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("</") && lower.ends_with('>')
}

fn inline_code_spans(text: &str, start: usize, end: usize) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut pos = start;
    while pos < end {
        let line_end = text[pos..end].find('\n').map_or(end, |i| pos + i + 1);
        let bytes = &text.as_bytes()[pos..line_end];
        let mut i = 0;
        let mut open: Option<(usize, usize)> = None;
        while i < bytes.len() {
            if bytes[i] == b'`' {
                let from = i;
                while i < bytes.len() && bytes[i] == b'`' {
                    i += 1;
                }
                let run = i - from;
                match open {
                    None => open = Some((run, pos + from)),
                    Some((open_run, start_byte)) if open_run == run => {
                        spans.push(start_byte..pos + i);
                        open = None;
                    }
                    _ => {}
                }
            } else {
                i += 1;
            }
        }
        pos = line_end;
    }
    spans
}

/// Field identity, length, revision and window. An edit bumps the revision and
/// a caret that crosses the window boundary gets a fresh scan; caret moves
/// inside one window reuse it.
type CacheKey = (usize, usize, u64, usize, usize);

/// A small bounded cache, so drawing several tabs or switching fields does not
/// re-scan every revision on every frame.
const MAX_CACHE_ENTRIES: usize = 8;

#[derive(Clone, Debug, Default)]
struct ContextCache {
    entries: Vec<(CacheKey, MarkdownContext)>,
}

impl ContextCache {
    fn context(&mut self, revision: u64, text: &str, caret: usize) -> MarkdownContext {
        let window = context_window(text, caret, CONTEXT_WINDOW_BYTES);
        let key = (text.as_ptr() as usize, text.len(), revision, window.start, window.end);
        if let Some((_, context)) = self.entries.iter().find(|(cached, _)| *cached == key) {
            return context.clone();
        }
        let context = markdown_context(text, caret, CONTEXT_WINDOW_BYTES);
        if self.entries.len() >= MAX_CACHE_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push((key, context.clone()));
        context
    }
}

thread_local! {
    static CONTEXT_CACHE: RefCell<ContextCache> = RefCell::new(ContextCache::default());
}

/// The bounded tree for a field at `caret`, cached by that field's revision.
///
/// The editor is single-threaded; the thread-local keeps `format_state` and
/// `slash_query_at` free of mutable borrows of the `EditorSession` they read.
pub fn cached_markdown_context(revision: u64, text: &str, caret: usize) -> MarkdownContext {
    CONTEXT_CACHE.with(|cache| cache.borrow_mut().context(revision, text, caret))
}

/// Forgets the cached tree (a reload or an explicit invalidation).
pub fn clear_markdown_context_cache() {
    CONTEXT_CACHE.with(|cache| cache.borrow_mut().entries.clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(text: &str, caret: usize) -> MarkdownContext {
        markdown_context(text, caret, CONTEXT_WINDOW_BYTES)
    }

    #[test]
    fn an_open_fence_marks_its_body_as_code() {
        let ctx = tree("```rust\nlet x = 1;\n/ta\n", 14);
        assert!(ctx.in_code(14));
        assert!(ctx.in_fence(14));
        assert!(!ctx.in_html(14));
    }

    #[test]
    fn a_closed_fence_leaves_the_following_paragraph_plain() {
        let ctx = tree("```\ncode\n```\n/ta\n", 14);
        assert!(!ctx.in_code(14));
    }

    #[test]
    fn a_tilde_fence_with_a_longer_closer_is_supported() {
        let ctx = tree("~~~js\ncode\n~~~~\n/ta\n", 16);
        assert!(!ctx.in_code(16));
    }

    #[test]
    fn an_indented_block_and_an_html_block_are_both_code_or_html() {
        let ctx = tree("    indented\n\n<div>\ntext\n\n/ta\n", 26);
        assert!(ctx.in_indented_code(2));
        assert!(ctx.in_html(20));
        assert!(!ctx.in_code(26));
        assert!(!ctx.in_html(26));
    }

    #[test]
    fn inline_code_spans_only_cover_equal_backtick_runs() {
        let ctx = tree("a `code` and ``x `y` z`` and `unclosed", 3);
        assert!(ctx.in_inline_code(3));
        assert!(!ctx.in_inline_code(11));
        assert!(ctx.in_inline_code(20));
        assert!(!ctx.in_inline_code(38));
    }

    #[test]
    fn the_window_is_bounded_and_always_covers_the_caret_line() {
        let text = format!("{}\nline\n", "x".repeat(CONTEXT_WINDOW_BYTES * 2));
        let caret = CONTEXT_WINDOW_BYTES * 2 + 1;
        let window = context_window(&text, caret, CONTEXT_WINDOW_BYTES);
        assert!(window.end - window.start <= CONTEXT_WINDOW_BYTES);
        assert!(window.start <= caret && caret < window.end);
    }

    #[test]
    fn the_cache_reuses_a_window_and_recomputes_on_revision_and_caret() {
        clear_markdown_context_cache();
        let text = "```\ncode\n```\nplain\n";
        let caret_in = 5;
        let caret_out = 14;
        let first = cached_markdown_context(1, text, caret_in);
        assert!(first.in_fence(caret_in));
        assert_eq!(cached_markdown_context(1, text, caret_in + 1).window(), first.window());
        let moved = cached_markdown_context(1, text, caret_out);
        assert!(moved.in_fence(moved.window().start + 1));
        assert!(!moved.in_code(caret_out));
        let edited = cached_markdown_context(2, text, caret_in);
        assert!(edited.in_fence(caret_in));
    }

    #[test]
    fn the_cache_is_bounded_across_many_field_revisions() {
        clear_markdown_context_cache();
        let text = "```\ncode\n```\nplain\n".to_owned();
        for revision in 0..(MAX_CACHE_ENTRIES * 3) as u64 {
            let context = cached_markdown_context(revision, &text, 5);
            assert!(context.in_fence(5));
        }
        CONTEXT_CACHE.with(|cache| assert!(cache.borrow().entries.len() <= MAX_CACHE_ENTRIES));
    }
}
