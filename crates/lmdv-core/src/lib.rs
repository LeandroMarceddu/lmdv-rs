//! Pure domain for lmdv: IDs, source buffer, document, registry, lease,
//! preferences and session value types.
//!
//! No Rune, no filesystem I/O, no threading. Values that cross a worker
//! boundary (snapshots) are `Send`; the shared document graph is UI-thread-only.

mod buffer;
mod document;
mod draft;
mod find;
mod ids;
mod nav;
mod prefs;
mod registry;
mod session;

pub use buffer::{EditDelta, Eol, FileMeta, SourceBuffer, detect_eol, normalize_lf, strip_bom};
pub use document::{DocStatus, Document, EditLease, LeaseError, SavedBase, hash_text};
pub use draft::{DRAFT_MAX_CHARS, DRAFT_MAX_ENTRIES, Draft};
pub use find::{FindMatch, FindState};
pub use ids::{DocumentId, DocumentKey, JobId, PasteId, TabId, WindowId};
pub use nav::{NAV_HISTORY_MAX, NavHistory, Place};
pub use prefs::{
    ColumnWidth, DEFAULT_READER_SIZE, EditorPrefs, LEFT_WIDTH_DEFAULT, LEFT_WIDTH_MAX, LargeFileMode, LineHeight,
    PREFS_SCHEMA, PanelPrefs, Prefs, RIGHT_WIDTH_DEFAULT, RIGHT_WIDTH_MAX, ReaderFont, ReaderPrefs, SIZE_STEPS,
    SideSize, ThemePref, snap_size,
};
pub use registry::DocumentRegistry;
pub use session::ScrollAnchor;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn worker_boundary_value_types_are_send() {
        assert_send::<SourceBuffer>();
        assert_send::<Prefs>();
        assert_send::<Draft>();
        assert_send::<FindState>();
        assert_send::<Arc<str>>();
    }
}
