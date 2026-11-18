//! Find/replace state shared by the viewer and the editor.

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// One hit in the source buffer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindMatch {
    /// Byte range in the LF-normalized source.
    pub source: Range<usize>,
    /// The top-level block containing the hit.
    pub block: usize,
    /// The zero-based source line.
    pub line: usize,
}

/// The find bar's query, options and current result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FindState {
    pub query: String,
    pub replace: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub matches: Vec<FindMatch>,
    pub current: usize,
    /// The replace row is visible (`Ctrl+H`) rather than find-only (`Ctrl+F`).
    pub replace_open: bool,
}

impl FindState {
    /// The selected match, if the index is still in range.
    pub fn current_match(&self) -> Option<&FindMatch> {
        self.matches.get(self.current)
    }

    /// Whether the query has at least one hit.
    pub fn has_matches(&self) -> bool {
        !self.matches.is_empty()
    }

    /// Keeps `current` inside `matches` after a refresh or a restore.
    pub fn sanitize(&mut self) {
        if self.matches.is_empty() {
            self.current = 0;
        } else {
            self.current = self.current.min(self.matches.len() - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_state_round_trips_through_json() {
        let state = FindState {
            query: "needle".into(),
            replace: "thread".into(),
            case_sensitive: true,
            whole_word: true,
            regex: false,
            matches: vec![
                FindMatch { source: 10..16, block: 2, line: 4 },
                FindMatch { source: 40..46, block: 5, line: 9 },
            ],
            current: 1,
            replace_open: true,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: FindState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
        assert_eq!(parsed.current_match(), Some(&state.matches[1]));
    }

    #[test]
    fn empty_and_out_of_range_selection_are_safe() {
        let mut state = FindState::default();
        assert_eq!(state.current_match(), None);
        state.current = 4;
        state.sanitize();
        assert_eq!(state.current, 0);

        state.matches.push(FindMatch { source: 0..1, block: 0, line: 0 });
        state.current = 99;
        state.sanitize();
        assert_eq!(state.current, 0);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let state: FindState = serde_json::from_str(r#"{"query":"x"}"#).unwrap();
        assert_eq!(state.query, "x");
        assert!(!state.case_sensitive);
        assert!(!state.whole_word);
        assert!(!state.regex);
        assert!(state.matches.is_empty());
        assert_eq!(state.current, 0);
        assert!(!state.replace_open);
    }
}
