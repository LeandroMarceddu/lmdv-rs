//! Find and replace over the LF editor text.
//!
//! Matches are byte ranges in the source, never in rendered text. Literal
//! search is the default; the optional regex path uses `regex` with Unicode
//! case folding. Whole-word matching checks the characters around each hit,
//! so it works for both paths. Empty matches (from patterns such as `a*` or
//! `^`) are dropped: they cannot be replaced or stepped through. This is the
//! editor half of E-20; the app owns the panel's face and focus routing.

use std::ops::Range;

use lmdv_core::FindState;
use regex::{Regex, RegexBuilder};

/// The find bar's query options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindOptions {
    /// Match case exactly; the find bar defaults to case-insensitive.
    pub case_sensitive: bool,
    /// Only matches whose surrounding characters are not word characters.
    pub whole_word: bool,
    /// Treat the query as a regular expression instead of a literal.
    pub regex: bool,
}

impl FindOptions {
    /// Reads the options stored in the shared [`FindState`].
    pub fn from_state(state: &FindState) -> Self {
        Self { case_sensitive: state.case_sensitive, whole_word: state.whole_word, regex: state.regex }
    }
}

/// One hit with its capture groups (group 0 is the whole match); captures are
/// only filled in regex mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Match {
    pub(crate) range: Range<usize>,
    pub(crate) groups: Vec<Option<String>>,
}

/// All matches of `query` in `text`, in source order and without overlap.
pub fn find(text: &str, query: &str, opts: &FindOptions) -> Vec<Range<usize>> {
    matches_in(text, query, opts).into_iter().map(|found| found.range).collect()
}

/// The regex compile error for the find bar, or `None` when the pattern is
/// valid (or the query is literal/empty). The app can show this beside the
/// query instead of an unexplained zero count.
pub fn find_error(query: &str, opts: &FindOptions) -> Option<String> {
    if query.is_empty() || !opts.regex {
        return None;
    }
    build(query, opts).err().map(|error| error.to_string())
}

pub(crate) fn matches_in(text: &str, query: &str, opts: &FindOptions) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let Some(regex) = compile(query, opts) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for captures in regex.captures_iter(text) {
        let Some(found) = captures.get(0) else {
            continue;
        };
        if found.start() == found.end() {
            continue;
        }
        if opts.whole_word && !whole_word_boundary(text, found.start(), found.end()) {
            continue;
        }
        let groups = if opts.regex {
            captures.iter().map(|group| group.map(|m| m.as_str().to_owned())).collect()
        } else {
            Vec::new()
        };
        out.push(Match { range: found.start()..found.end(), groups });
    }
    out
}

/// Expands `$1`–`$9`, `$0`, `$&`, `${n}` and `$$` in a regex replacement.
pub(crate) fn expand_replacement(template: &str, groups: &[Option<String>]) -> String {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            out.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('&') => {
                chars.next();
                push_group(&mut out, groups, 0);
            }
            Some(digit) if digit.is_ascii_digit() => {
                let index = take_number(&mut chars);
                push_group(&mut out, groups, index);
            }
            Some('{') => {
                chars.next();
                let mut digits = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_digit() {
                        digits.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if !digits.is_empty() && chars.peek() == Some(&'}') {
                    chars.next();
                    let index = digits.parse::<usize>().unwrap_or(0);
                    push_group(&mut out, groups, index);
                } else {
                    out.push_str("${");
                    out.push_str(&digits);
                }
            }
            _ => out.push('$'),
        }
    }
    out
}

fn take_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> usize {
    let mut value = 0usize;
    while let Some(&ch) = chars.peek() {
        let Some(digit) = ch.to_digit(10) else {
            break;
        };
        value = value.saturating_mul(10).saturating_add(digit as usize);
        chars.next();
    }
    value
}

fn push_group(out: &mut String, groups: &[Option<String>], index: usize) {
    if let Some(Some(group)) = groups.get(index) {
        out.push_str(group);
    }
}

fn build(query: &str, opts: &FindOptions) -> Result<Regex, regex::Error> {
    let pattern = if opts.regex { query.to_owned() } else { regex::escape(query) };
    RegexBuilder::new(&pattern).case_insensitive(!opts.case_sensitive).unicode(true).multi_line(true).build()
}

fn compile(query: &str, opts: &FindOptions) -> Option<Regex> {
    build(query, opts).ok()
}

fn whole_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(is_word) && !after.is_some_and(is_word)
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal(whole_word: bool, case_sensitive: bool) -> FindOptions {
        FindOptions { case_sensitive, whole_word, regex: false }
    }

    fn regex_options(whole_word: bool) -> FindOptions {
        FindOptions { case_sensitive: true, whole_word, regex: true }
    }

    #[test]
    fn literal_search_is_case_insensitive_by_default() {
        assert_eq!(find("Alpha alpha ALPHA", "alpha", &FindOptions::default()), vec![0..5, 6..11, 12..17]);
        assert_eq!(find("Alpha alpha ALPHA", "alpha", &literal(false, true)), vec![6..11]);
    }

    #[test]
    fn whole_word_ignores_embedded_hits() {
        assert_eq!(find("cat scatter cat.", "cat", &literal(true, false)), vec![0..3, 12..15]);
        assert_eq!(find("cat_cat cat", "cat", &literal(true, false)), vec![8..11], "underscore is a word character");
    }

    #[test]
    fn unicode_case_folding_keeps_byte_ranges_valid() {
        let text = "É é E";
        assert_eq!(find(text, "é", &FindOptions::default()), vec![0..2, 3..5]);
        assert_eq!(find(text, "é", &literal(false, true)), vec![3..5]);
        for range in find(text, "é", &FindOptions::default()) {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }
    }

    #[test]
    fn regex_matches_are_leftmost_and_non_overlapping() {
        let text = "a1 bb22 c333";
        assert_eq!(find(text, r"\d+", &regex_options(false)), vec![1..2, 5..7, 9..12]);
        assert_eq!(find("a1\nbb22\nc333", r"^[a-z]+", &regex_options(false)), vec![0..1, 3..5, 8..9]);
    }

    #[test]
    fn regex_whole_word_filters_by_the_actual_match() {
        assert_eq!(find("cat scatter cat", "cat", &regex_options(true)), vec![0..3, 12..15]);
    }

    #[test]
    fn invalid_and_empty_queries_are_empty() {
        assert!(find("a", "", &FindOptions::default()).is_empty());
        assert!(find("a", "(", &regex_options(false)).is_empty());
        assert!(find_error("(", &regex_options(false)).is_some());
        assert!(find_error("(", &literal(false, false)).is_none());
        assert!(find_error("a", &regex_options(false)).is_none());
    }

    #[test]
    fn empty_matches_are_skipped() {
        assert!(find("bbb", "a*", &regex_options(false)).is_empty());
        assert_eq!(find("aa b", "a*", &regex_options(false)), vec![0..2]);
        assert!(find("one\ntwo", "^", &regex_options(false)).is_empty());
    }

    #[test]
    fn replacement_expands_capture_groups() {
        let groups = vec![Some("first second".into()), Some("first".into()), Some("second".into())];
        assert_eq!(expand_replacement("$1-$2", &groups), "first-second");
        assert_eq!(expand_replacement("${2}/${1}", &groups), "second/first");
        assert_eq!(expand_replacement("[$&]", &groups), "[first second]");
        assert_eq!(expand_replacement("$0", &groups), "first second");
        assert_eq!(expand_replacement("cost $$5", &groups), "cost $5");
        assert_eq!(expand_replacement("$9", &groups), "");
        assert_eq!(expand_replacement("${12x}", &groups), "${12x}");
        assert_eq!(expand_replacement("${}", &groups), "${}");
        assert_eq!(expand_replacement("plain", &groups), "plain");
    }

    #[test]
    fn replacement_without_captures_is_verbatim() {
        assert_eq!(expand_replacement("$1 literal", &[]), " literal");
    }

    #[test]
    fn matches_returns_captures_only_in_regex_mode() {
        let regex_matches = matches_in("ab", "(a)(b)", &regex_options(false));
        assert_eq!(regex_matches.len(), 1);
        assert_eq!(regex_matches[0].groups, vec![Some("ab".into()), Some("a".into()), Some("b".into())]);
        let literal_matches = matches_in("ab", "ab", &literal(false, false));
        assert_eq!(literal_matches[0].groups, Vec::new());
    }

    #[test]
    fn state_options_round_trip() {
        let state = FindState {
            query: "q".into(),
            case_sensitive: true,
            whole_word: true,
            regex: true,
            ..FindState::default()
        };
        assert_eq!(
            FindOptions::from_state(&state),
            FindOptions { case_sensitive: true, whole_word: true, regex: true }
        );
    }
}
