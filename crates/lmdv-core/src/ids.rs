//! Stable identifiers and document keys.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A document identity, allocated once per process and never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DocumentId(pub u64);

/// A window identity, process-monotonic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId(pub u64);

/// A tab identity, unique inside one window and never reused there.
pub type TabId = u32;

/// A background job identity, globally unique.
pub type JobId = u64;

/// Identifies pasted text that has no file yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PasteId(pub u64);

/// What a tab shows. A document is keyed by its canonical file path, its paste
/// identity, or nothing at all (the start page, which is never a `Document`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocumentKey {
    /// Canonical, absolute, not necessarily existing (a missing file still has a tab).
    File(PathBuf),
    /// Clipboard text with no file yet.
    Untitled(PasteId),
    /// The start page. A tab only, never a `Document`.
    Start,
}

impl DocumentKey {
    /// The file path for a file-backed key.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Untitled(_) | Self::Start => None,
        }
    }

    /// Whether this is the start page.
    pub fn is_start(&self) -> bool {
        matches!(self, Self::Start)
    }
}
