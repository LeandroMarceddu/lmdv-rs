//! The canonical document: one shared buffer with one editor lease.

use std::cell::{Cell, Ref, RefCell};
use std::collections::BTreeSet;
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::buffer::{EditDelta, Eol, FileMeta, SourceBuffer};
use crate::ids::{DocumentId, DocumentKey, TabId, WindowId};

/// Where a document is in its load lifecycle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DocStatus {
    #[default]
    Loading,
    Ready,
    Error(String),
    /// The backing file disappeared. The buffer is kept so it can be saved again.
    Gone,
}

/// The last content known to be on disk, and the file identity it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedBase {
    /// Buffer revision captured when this base was recorded.
    pub revision: u64,
    /// BLAKE3 of the LF-normalized text.
    pub hash: [u8; 32],
    /// The file path, when the document is backed by a file.
    pub path: Option<PathBuf>,
    pub meta: FileMeta,
    pub eol: Eol,
    pub had_bom: bool,
}

impl Default for SavedBase {
    fn default() -> Self {
        Self { revision: 0, hash: hash_text(""), path: None, meta: FileMeta::default(), eol: Eol::Lf, had_bom: false }
    }
}

/// The single editor that may mutate a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditLease {
    pub window: WindowId,
    pub tab: TabId,
}

/// Returned when a document is already being edited by another tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseError {
    pub holder: EditLease,
}

impl LeaseError {
    /// The tab that currently holds the lease.
    pub fn holder(&self) -> EditLease {
        self.holder
    }
}

impl fmt::Display for LeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let EditLease { window, tab } = self.holder;
        write!(f, "document is being edited by window {} tab {tab}", window.0)
    }
}

impl std::error::Error for LeaseError {}

/// One canonical document per file, shared by every tab and window.
///
/// All mutation goes through `&self` and one interior cell at a time; the
/// document is never handed out as `&mut`.
#[derive(Debug)]
pub struct Document {
    pub id: DocumentId,
    pub key: RefCell<DocumentKey>,
    pub title: RefCell<String>,
    pub buf: RefCell<SourceBuffer>,
    pub status: RefCell<DocStatus>,
    pub saved: RefCell<SavedBase>,
    pub lease: RefCell<Option<EditLease>>,
    pub subscribers: RefCell<BTreeSet<WindowId>>,
    /// Watcher events for this document are ignored until this instant; set
    /// after an own write to suppress the echo.
    pub ignore_writes_until: Cell<Option<Instant>>,
    /// The file identity produced by our last write, for echo suppression.
    pub last_local_write: Cell<Option<FileMeta>>,
}

impl Document {
    /// A document whose content is still loading.
    pub fn loading(id: DocumentId, key: DocumentKey, title: impl Into<String>) -> Self {
        Self {
            id,
            key: RefCell::new(key),
            title: RefCell::new(title.into()),
            buf: RefCell::new(SourceBuffer::empty()),
            status: RefCell::new(DocStatus::Loading),
            saved: RefCell::new(SavedBase::default()),
            lease: RefCell::new(None),
            subscribers: RefCell::new(BTreeSet::new()),
            ignore_writes_until: Cell::new(None),
            last_local_write: Cell::new(None),
        }
    }

    /// A ready document whose current buffer is treated as the saved base.
    /// Call [`Self::set_saved`] afterwards when the real file metadata is known.
    pub fn ready(id: DocumentId, key: DocumentKey, title: impl Into<String>, buf: SourceBuffer) -> Self {
        let saved = SavedBase {
            revision: buf.revision(),
            hash: hash_text(buf.text()),
            path: key.path().map(PathBuf::from),
            meta: FileMeta::default(),
            eol: buf.eol(),
            had_bom: buf.had_bom(),
        };
        Self {
            id,
            key: RefCell::new(key),
            title: RefCell::new(title.into()),
            buf: RefCell::new(buf),
            status: RefCell::new(DocStatus::Ready),
            saved: RefCell::new(saved),
            lease: RefCell::new(None),
            subscribers: RefCell::new(BTreeSet::new()),
            ignore_writes_until: Cell::new(None),
            last_local_write: Cell::new(None),
        }
    }

    /// The current text revision.
    pub fn revision(&self) -> u64 {
        self.buf.borrow().revision()
    }

    /// Borrows the buffer; drop the guard before any call that may touch it.
    pub fn text(&self) -> Ref<'_, SourceBuffer> {
        self.buf.borrow()
    }

    /// An immutable snapshot for a parse or export job.
    pub fn snapshot(&self) -> Arc<str> {
        self.buf.borrow().snapshot()
    }

    /// Applies one splice and returns its delta.
    pub fn apply(&self, range: Range<usize>, text: &str) -> EditDelta {
        self.buf.borrow_mut().replace(range, text)
    }

    /// Replaces the whole buffer, for a reload or restore. Returns the new revision.
    pub fn replace_all(&self, text: &str, eol: Eol) -> u64 {
        self.buf.borrow_mut().replace_all(text, eol)
    }

    /// BLAKE3 of the current LF-normalized text.
    pub fn hash(&self) -> [u8; 32] {
        hash_text(self.buf.borrow().text())
    }

    /// Dirty is hash inequality, not revision inequality: undoing back to the
    /// saved text makes the document clean again.
    pub fn is_dirty(&self) -> bool {
        self.hash() != self.saved.borrow().hash
    }

    /// Takes the editor lease, or reports the current holder.
    ///
    /// Acquiring while already holding the same lease succeeds, which makes
    /// repeated command dispatch idempotent.
    pub fn acquire_edit(&self, window: WindowId, tab: TabId) -> Result<EditLease, LeaseError> {
        let requested = EditLease { window, tab };
        let mut lease = self.lease.borrow_mut();
        match *lease {
            None => {
                *lease = Some(requested);
                Ok(requested)
            }
            Some(current) if current == requested => Ok(requested),
            Some(holder) => Err(LeaseError { holder }),
        }
    }

    /// Takes the lease from another tab after the user chose **Take over**.
    pub fn take_over_edit(&self, window: WindowId, tab: TabId) -> EditLease {
        let lease = EditLease { window, tab };
        *self.lease.borrow_mut() = Some(lease);
        lease
    }

    /// Releases the lease if this tab still holds it. A stale release from a
    /// previous holder is ignored.
    pub fn release_edit(&self, lease: EditLease) {
        let mut current = self.lease.borrow_mut();
        if *current == Some(lease) {
            *current = None;
        }
    }

    /// The current lease holder, if any.
    pub fn lease_holder(&self) -> Option<EditLease> {
        *self.lease.borrow()
    }

    /// Records the content and metadata just written to (or read from) disk.
    ///
    /// The caller owns the registry updates: `Save As` needs
    /// [`crate::DocumentRegistry::rekey`], and a completed load needs
    /// [`crate::DocumentRegistry::insert_ready`] so the `(dev, ino)` identity
    /// becomes searchable. This only updates the saved base, revision, EOL and
    /// BOM policy.
    pub fn set_saved(&self, meta: FileMeta, hash: [u8; 32], path: PathBuf) {
        let (revision, eol, had_bom) = {
            let buf = self.buf.borrow();
            (buf.revision(), buf.eol(), buf.had_bom())
        };
        *self.saved.borrow_mut() = SavedBase { revision, hash, path: Some(path), meta, eol, had_bom };
    }

    /// Marks the backing file as gone without discarding the buffer.
    pub fn mark_gone(&self) {
        *self.status.borrow_mut() = DocStatus::Gone;
    }
}

/// BLAKE3 of a UTF-8 string.
pub fn hash_text(text: &str) -> [u8; 32] {
    *blake3::hash(text.as_bytes()).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::DocumentId;

    fn file_doc(text: &str) -> Document {
        let key = DocumentKey::File(PathBuf::from("/tmp/note.md"));
        Document::ready(DocumentId(7), key, "note.md", SourceBuffer::from_file(text))
    }

    #[test]
    fn ready_document_is_clean_and_becomes_dirty_by_hash() {
        let doc = file_doc("hello\n");
        assert!(!doc.is_dirty());
        let clean_revision = doc.revision();

        let delta = doc.apply(0..5, "world");
        assert_eq!(delta.start, 0);
        assert!(doc.is_dirty());
        assert!(doc.revision() > clean_revision);

        doc.apply(0..5, "hello");
        assert!(!doc.is_dirty(), "undoing back to the saved text is clean even though revision moved");
    }

    #[test]
    fn set_saved_updates_base_revision_eol_and_path() {
        let doc = file_doc("a\n");
        doc.apply(1..1, "b");
        assert!(doc.is_dirty());

        let meta = FileMeta { len: 3, mtime_ns: 42, dev: 1, ino: 2 };
        let hash = doc.hash();
        doc.set_saved(meta, hash, PathBuf::from("/tmp/note.md"));

        assert!(!doc.is_dirty());
        let saved = doc.saved.borrow();
        assert_eq!(saved.revision, doc.revision());
        assert_eq!(saved.meta, meta);
        assert_eq!(saved.path.as_deref(), Some(std::path::Path::new("/tmp/note.md")));
        assert_eq!(saved.eol, Eol::Lf);
        assert!(!saved.had_bom);
    }

    #[test]
    fn lease_acquire_takeover_and_release() {
        let doc = file_doc("x");
        let first = EditLease { window: WindowId(1), tab: 2 };
        let second = EditLease { window: WindowId(3), tab: 4 };

        assert_eq!(doc.acquire_edit(first.window, first.tab), Ok(first));
        assert_eq!(doc.acquire_edit(first.window, first.tab), Ok(first), "re-acquire is idempotent");
        assert_eq!(doc.acquire_edit(second.window, second.tab), Err(LeaseError { holder: first }));
        assert_eq!(doc.lease_holder(), Some(first));

        assert_eq!(doc.take_over_edit(second.window, second.tab), second);
        assert_eq!(doc.lease_holder(), Some(second));

        doc.release_edit(first);
        assert_eq!(doc.lease_holder(), Some(second), "a stale release is ignored");

        doc.release_edit(second);
        assert_eq!(doc.lease_holder(), None);
    }

    #[test]
    fn replace_all_and_mark_gone() {
        let doc = file_doc("old");
        let revision = doc.replace_all("new\r\ntext", Eol::CrLf);
        assert_eq!(revision, 1);
        assert_eq!(doc.text().text(), "new\ntext");
        assert_eq!(doc.text().eol(), Eol::CrLf);
        assert!(doc.is_dirty());

        doc.mark_gone();
        assert_eq!(*doc.status.borrow(), DocStatus::Gone);
        assert_eq!(doc.text().text(), "new\ntext", "the buffer survives a deleted file");
    }

    #[test]
    fn empty_document_default_base_is_clean() {
        let doc = Document::ready(
            DocumentId(1),
            DocumentKey::Untitled(crate::ids::PasteId(1)),
            "Pasted",
            SourceBuffer::empty(),
        );
        assert!(!doc.is_dirty());
        assert_eq!(doc.hash(), SavedBase::default().hash);
    }
}
