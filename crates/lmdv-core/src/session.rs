//! Session-facing position types.

use serde::{Deserialize, Serialize};

/// A reading position anchored to a heading.
///
/// `offset` follows the original app: it is how far the heading sat *above*
/// the viewport top when the position was captured. Zero with an empty id is
/// the top of the document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollAnchor {
    /// The heading's id; empty when the position is a raw scroll offset.
    pub id: String,
    /// Distance in points the anchor sat above the viewport top.
    pub offset: f32,
}

impl ScrollAnchor {
    /// The top of the document.
    pub fn top() -> Self {
        Self::default()
    }

    /// Whether this position is the top of the document and can be forgotten.
    pub fn is_top(&self) -> bool {
        self.id.is_empty() && self.offset <= 0.0
    }

    /// Replaces non-finite offsets from corrupt session JSON with zero.
    pub fn sanitize(&mut self) {
        if !self.offset.is_finite() {
            self.offset = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_round_trips_and_sanitizes() {
        let anchor = ScrollAnchor { id: "heading-1".into(), offset: 12.5 };
        let json = serde_json::to_string(&anchor).unwrap();
        let parsed: ScrollAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, anchor);

        let mut corrupt = ScrollAnchor { id: "h".into(), offset: f32::INFINITY };
        corrupt.sanitize();
        assert_eq!(corrupt.offset, 0.0);
    }

    #[test]
    fn empty_anchor_at_or_above_top_is_top() {
        assert!(ScrollAnchor::top().is_top());
        assert!(ScrollAnchor { id: String::new(), offset: -10.0 }.is_top());
        assert!(!ScrollAnchor { id: String::new(), offset: 10.0 }.is_top());
        assert!(!ScrollAnchor { id: "h".into(), offset: 0.0 }.is_top());
    }
}
