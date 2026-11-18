//! Per-tab Back/Forward history.

use serde::{Deserialize, Serialize};

use crate::session::ScrollAnchor;

/// Maximum entries on each list.
pub const NAV_HISTORY_MAX: usize = 50;

/// One place in a document's history.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Place {
    /// The document key as a string: a file path or `pasted:<id>`.
    pub path: String,
    pub pos: ScrollAnchor,
}

impl Place {
    /// Builds a place from a key and an anchor.
    pub fn new(path: impl Into<String>, pos: ScrollAnchor) -> Self {
        Self { path: path.into(), pos }
    }

    /// Replaces non-finite offsets from corrupt session JSON.
    pub fn sanitize(&mut self) {
        self.pos.sanitize();
    }
}

/// A tab's Back and Forward lists.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NavHistory {
    pub back: Vec<Place>,
    pub forward: Vec<Place>,
}

impl NavHistory {
    /// Records the place being left before moving somewhere new; the Forward
    /// list is cleared, matching a browser.
    pub fn record(&mut self, from: Place) {
        if self.back.last().is_none_or(|top| !same_place(top, &from)) {
            self.back.push(from);
        }
        trim(&mut self.back);
        self.forward.clear();
    }

    /// Whether Back can be used.
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether Forward can be used.
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// The place Back would go to.
    pub fn peek_back(&self) -> Option<&Place> {
        self.back.last()
    }

    /// The place Forward would go to.
    pub fn peek_forward(&self) -> Option<&Place> {
        self.forward.last()
    }

    /// Commits a Back move once the destination is on screen. `current` is the
    /// place that was left.
    pub fn commit_back(&mut self, current: Option<Place>) {
        self.back.pop();
        if let Some(current) = current {
            self.forward.push(current);
        }
        trim(&mut self.forward);
    }

    /// Commits a Forward move once the destination is on screen.
    pub fn commit_forward(&mut self, current: Option<Place>) {
        self.forward.pop();
        if let Some(current) = current {
            self.back.push(current);
        }
        trim(&mut self.back);
    }

    /// Drops an unreachable place: `dir < 0` drops Back, otherwise Forward.
    pub fn drop_place(&mut self, dir: i8) {
        if dir < 0 {
            self.back.pop();
        } else {
            self.forward.pop();
        }
    }

    /// Makes a renamed file's places follow it. Paths are compared exactly;
    /// Linux paths are case-sensitive.
    pub fn rename_path(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        for place in self.back.iter_mut().chain(self.forward.iter_mut()) {
            if place.path == from {
                place.path = to.to_owned();
            }
        }
    }

    /// Trims lists to their bound and sanitizes anchors loaded from JSON.
    pub fn sanitize(&mut self) {
        trim(&mut self.back);
        trim(&mut self.forward);
        for place in self.back.iter_mut().chain(self.forward.iter_mut()) {
            place.sanitize();
        }
    }
}

fn same_place(a: &Place, b: &Place) -> bool {
    a.path == b.path && a.pos.id == b.pos.id && (a.pos.offset - b.pos.offset).abs() < 4.0
}

fn trim(list: &mut Vec<Place>) {
    if list.len() > NAV_HISTORY_MAX {
        list.drain(..list.len() - NAV_HISTORY_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(path: &str, offset: f32) -> Place {
        Place::new(path, ScrollAnchor { id: "h".into(), offset })
    }

    #[test]
    fn recording_clears_forward_and_dedupes_the_top() {
        let mut history = NavHistory::default();
        history.record(place("/a.md", 0.0));
        history.record(place("/a.md", 1.0));
        assert_eq!(history.back.len(), 1, "the same place within 4 points is not repeated");

        history.commit_back(Some(place("/b.md", 0.0)));
        assert!(history.can_forward());
        history.record(place("/c.md", 0.0));
        assert!(!history.can_forward(), "a new move clears Forward");
        assert_eq!(history.back.len(), 1);
    }

    #[test]
    fn back_and_forward_swap_places() {
        let mut history = NavHistory::default();
        history.record(place("/a.md", 0.0));
        history.record(place("/b.md", 0.0));
        assert_eq!(history.peek_back().map(|p| p.path.as_str()), Some("/b.md"));

        history.commit_back(Some(place("/c.md", 0.0)));
        assert_eq!(history.back.len(), 1);
        assert_eq!(history.peek_forward().map(|p| p.path.as_str()), Some("/c.md"));

        history.commit_forward(Some(place("/d.md", 0.0)));
        assert!(history.forward.is_empty());
        assert_eq!(history.peek_back().map(|p| p.path.as_str()), Some("/d.md"));
    }

    #[test]
    fn lists_are_capped_at_fifty() {
        let mut history = NavHistory::default();
        for index in 0..60 {
            history.record(place(&format!("/{index}.md"), 0.0));
        }
        assert_eq!(history.back.len(), NAV_HISTORY_MAX);
        assert_eq!(history.back.first().map(|p| p.path.as_str()), Some("/10.md"));
    }

    #[test]
    fn drop_and_rename_follow_the_original_rules() {
        let mut history = NavHistory::default();
        history.record(place("/a.md", 0.0));
        history.record(place("/b.md", 0.0));
        history.drop_place(-1);
        assert_eq!(history.peek_back().map(|p| p.path.as_str()), Some("/a.md"));

        history.record(place("/c.md", 0.0));
        history.commit_back(Some(place("/d.md", 0.0)));
        history.rename_path("/a.md", "/renamed.md");
        assert_eq!(history.back.first().map(|p| p.path.as_str()), Some("/renamed.md"));
        history.rename_path("/d.md", "/d2.md");
        assert_eq!(history.forward.first().map(|p| p.path.as_str()), Some("/d2.md"));
    }

    #[test]
    fn paths_are_case_sensitive_on_linux() {
        let mut history = NavHistory::default();
        history.record(place("/A.md", 0.0));
        history.record(place("/a.md", 0.0));
        assert_eq!(history.back.len(), 2);
    }

    #[test]
    fn history_round_trips_through_json() {
        let mut history = NavHistory::default();
        history.record(Place::new("/a.md", ScrollAnchor { id: "intro".into(), offset: 12.0 }));
        history.commit_back(Some(Place::new("pasted:1", ScrollAnchor::top())));

        let json = serde_json::to_string(&history).unwrap();
        let parsed: NavHistory = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, history);
    }
}
