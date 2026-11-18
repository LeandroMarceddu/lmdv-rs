//! Crash-recovery drafts.

use serde::{Deserialize, Serialize};

/// Maximum number of stored drafts.
pub const DRAFT_MAX_ENTRIES: usize = 20;

/// Maximum total characters across all draft text and bases.
pub const DRAFT_MAX_CHARS: usize = 2_000_000;

/// The unsaved editor buffer parked for crash recovery.
///
/// A draft never writes the file by itself. `base` is the LF-normalized file
/// content the edits started from, so a stale draft can be told apart.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    /// The LF-normalized buffer.
    pub text: String,
    /// Last update, milliseconds since the Unix epoch.
    pub at: u64,
    /// The file content the edits started from.
    pub base: String,
}

impl Draft {
    /// Builds a draft from its text, base and timestamp.
    pub fn new(text: impl Into<String>, base: impl Into<String>, at: u64) -> Self {
        Self { text: text.into(), base: base.into(), at }
    }

    /// Characters counted against [`DRAFT_MAX_CHARS`].
    pub fn char_count(&self) -> usize {
        self.text.chars().count() + self.base.chars().count()
    }

    /// Whether the draft equals the text currently on disk.
    pub fn matches_disk(&self, disk: &str) -> bool {
        self.text == disk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_round_trips_and_counts_characters() {
        let draft = Draft::new("# Note\nbody", "# Note\n", 1_700_000_000_000);
        let json = serde_json::to_string(&draft).unwrap();
        let parsed: Draft = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, draft);
        assert_eq!(parsed.char_count(), draft.text.chars().count() + draft.base.chars().count());
        assert!(parsed.matches_disk(&draft.text));
        assert!(!parsed.matches_disk("# Note\n"));
    }

    #[test]
    fn missing_fields_default() {
        let draft: Draft = serde_json::from_str("{}").unwrap();
        assert_eq!(draft, Draft::default());
        assert_eq!(draft.char_count(), 0);
    }
}
