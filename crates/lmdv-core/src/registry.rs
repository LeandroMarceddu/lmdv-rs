//! The process-wide registry of canonical documents.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::buffer::FileMeta;
use crate::document::Document;
use crate::ids::{DocumentId, DocumentKey};

/// Maps document ids, canonical file paths and `(dev, ino)` identities to the
/// one `Document` per file.
///
/// Paths are compared exactly: Linux filesystems are case-sensitive, so
/// `/tmp/A.md` and `/tmp/a.md` are different documents.
#[derive(Debug, Default)]
pub struct DocumentRegistry {
    docs: HashMap<DocumentId, Rc<Document>>,
    by_path: HashMap<PathBuf, DocumentId>,
    by_identity: HashMap<(u64, u64), DocumentId>,
    next_id: u64,
}

impl DocumentRegistry {
    /// An empty registry. Document ids start at 1, leaving 0 as a null id.
    pub fn new() -> Self {
        Self { docs: HashMap::new(), by_path: HashMap::new(), by_identity: HashMap::new(), next_id: 1 }
    }

    /// Number of registered documents.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Whether the registry holds no documents.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Reserves a fresh document id.
    pub fn allocate_id(&mut self) -> DocumentId {
        let id = DocumentId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Looks a document up by id.
    pub fn get(&self, id: DocumentId) -> Option<Rc<Document>> {
        self.docs.get(&id).cloned()
    }

    /// Looks a document up by its canonical path.
    pub fn find_path(&self, path: &Path) -> Option<Rc<Document>> {
        self.by_path.get(path).and_then(|id| self.docs.get(id)).cloned()
    }

    /// Looks a document up by `(dev, ino)`. A meta with an unknown identity
    /// never matches.
    pub fn find_identity(&self, meta: &FileMeta) -> Option<Rc<Document>> {
        if !meta.identity_known() {
            return None;
        }
        self.by_identity.get(&(meta.dev, meta.ino)).and_then(|id| self.docs.get(id)).cloned()
    }

    /// Creates a loading document and registers it. If a file path is already
    /// registered, the existing document is returned and nothing is created.
    ///
    /// `DocumentKey::Start` is a tab-only key; callers must not create a
    /// document for the start page.
    pub fn insert_loading(&mut self, key: DocumentKey, title: String) -> Rc<Document> {
        debug_assert!(!key.is_start(), "the start page is a tab, never a document");
        if let DocumentKey::File(path) = &key
            && let Some(existing) = self.find_path(path)
        {
            return existing;
        }

        let id = self.allocate_id();
        let doc = Rc::new(Document::loading(id, key.clone(), title));
        self.docs.insert(id, doc.clone());
        if let DocumentKey::File(path) = key {
            self.by_path.insert(path, id);
        }
        doc
    }

    /// Registers an already built document, reusing its id and refreshing its
    /// path and identity maps.
    ///
    /// Call this again after a load completes as well: a document created by
    /// [`Self::insert_loading`] only gains its `(dev, ino)` identity once its
    /// saved metadata is known.
    pub fn insert_ready(&mut self, doc: Rc<Document>) {
        let id = doc.id;
        self.next_id = self.next_id.max(id.0.saturating_add(1));

        let path = doc.key.borrow().path().map(Path::to_path_buf);
        let meta = doc.saved.borrow().meta;
        match path {
            Some(path) => {
                self.by_path.insert(path, id);
                self.refresh_identity(id, meta);
            }
            None => self.drop_identity(id),
        }
        self.docs.insert(id, doc);
    }

    /// Points an existing document at a new key, as for a rename or `Save As`.
    /// The old path mapping is removed; the identity follows the file.
    pub fn rekey(&mut self, id: DocumentId, new_key: DocumentKey) {
        let Some(doc) = self.docs.get(&id).cloned() else {
            return;
        };

        let old_path = doc.key.borrow().path().map(Path::to_path_buf);
        if let Some(old_path) = old_path
            && self.by_path.get(&old_path) == Some(&id)
        {
            self.by_path.remove(&old_path);
        }

        let new_path = new_key.path().map(Path::to_path_buf);
        if let Some(new_path) = &new_path {
            self.by_path.insert(new_path.clone(), id);
        }
        *doc.key.borrow_mut() = new_key;

        let meta = doc.saved.borrow().meta;
        if new_path.is_some() {
            self.refresh_identity(id, meta);
        } else {
            self.drop_identity(id);
        }
    }

    /// Removes one document from the registry because its views joined
    /// another document for the same file (OI-005).
    ///
    /// Only the registry entries are dropped: the returned `Document` stays
    /// alive wherever a tab still holds it, and the caller points those tabs
    /// at the surviving document.
    pub fn remove(&mut self, id: DocumentId) -> Option<Rc<Document>> {
        let doc = self.docs.remove(&id)?;
        self.by_path.retain(|_, mapped| *mapped != id);
        self.by_identity.retain(|_, mapped| *mapped != id);
        Some(doc)
    }

    /// Removes and returns every document that only the registry still holds.
    /// Path and identity entries for removed documents are purged.
    pub fn collect_unused(&mut self) -> Vec<DocumentId> {
        let mut unused: Vec<DocumentId> =
            self.docs.iter().filter(|(_, doc)| Rc::strong_count(doc) == 1).map(|(&id, _)| id).collect();
        unused.sort_unstable();

        for id in &unused {
            self.docs.remove(id);
        }
        self.by_path.retain(|_, mapped| !unused.contains(mapped));
        self.by_identity.retain(|_, mapped| !unused.contains(mapped));
        unused
    }

    /// Keeps exactly the identity of `meta` for this document, dropping stale
    /// identities left by an earlier path or inode.
    fn refresh_identity(&mut self, id: DocumentId, meta: FileMeta) {
        let identity = meta.identity_known().then_some((meta.dev, meta.ino));
        self.by_identity.retain(|key, mapped| *mapped != id || Some(*key) == identity);
        if let Some(identity) = identity {
            self.by_identity.insert(identity, id);
        }
    }

    /// Forgets every identity for a document that no longer has a file.
    fn drop_identity(&mut self, id: DocumentId) {
        self.by_identity.retain(|_, mapped| *mapped != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::SourceBuffer;
    use crate::document::Document;
    use crate::ids::{DocumentKey, PasteId};

    fn path(name: &str) -> PathBuf {
        PathBuf::from("/tmp/lmdv-core").join(name)
    }

    fn ready_doc(id: DocumentId, path: PathBuf, text: &str, meta: FileMeta) -> Rc<Document> {
        let doc = Document::ready(id, DocumentKey::File(path.clone()), "doc.md", SourceBuffer::from_file(text));
        doc.set_saved(meta, doc.hash(), path);
        Rc::new(doc)
    }

    #[test]
    fn loading_documents_are_found_by_exact_path() {
        let mut registry = DocumentRegistry::new();
        let file = path("Note.md");
        let doc = registry.insert_loading(DocumentKey::File(file.clone()), "Note.md".into());

        assert_eq!(registry.len(), 1);
        assert_eq!(doc.id, DocumentId(1));
        assert_eq!(*doc.status.borrow(), crate::document::DocStatus::Loading);
        assert!(Rc::ptr_eq(&doc, &registry.find_path(&file).unwrap()));
        assert!(registry.find_path(Path::new("/tmp/lmdv-core/note.md")).is_none());
        assert!(registry.find_path(Path::new("/tmp/lmdv-core/other.md")).is_none());
    }

    #[test]
    fn inserting_the_same_path_returns_the_existing_document() {
        let mut registry = DocumentRegistry::new();
        let key = DocumentKey::File(path("a.md"));
        let first = registry.insert_loading(key.clone(), "a.md".into());
        let second = registry.insert_loading(key, "again.md".into());

        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.allocate_id(), DocumentId(2), "the duplicate did not burn an id");
    }

    #[test]
    fn ready_documents_are_found_by_identity() {
        let mut registry = DocumentRegistry::new();
        let meta = FileMeta { len: 4, mtime_ns: 9, dev: 10, ino: 20 };
        let doc = ready_doc(DocumentId(5), path("b.md"), "text", meta);
        registry.insert_ready(doc.clone());

        assert!(Rc::ptr_eq(&doc, &registry.find_identity(&meta).unwrap()));
        assert!(Rc::ptr_eq(&doc, &registry.find_path(&path("b.md")).unwrap()));
        assert_eq!(registry.allocate_id(), DocumentId(6), "next id advanced past an inserted id");

        let unknown = FileMeta { len: 4, mtime_ns: 9, dev: 0, ino: 0 };
        assert!(registry.find_identity(&unknown).is_none());
        let other = FileMeta { dev: 10, ino: 21, ..meta };
        assert!(registry.find_identity(&other).is_none());
    }

    #[test]
    fn rekey_moves_path_and_identity() {
        let mut registry = DocumentRegistry::new();
        let old_meta = FileMeta { len: 4, mtime_ns: 1, dev: 7, ino: 8 };
        let doc = ready_doc(DocumentId(1), path("old.md"), "text", old_meta);
        registry.insert_ready(doc.clone());

        let new_path = path("new.md");
        let new_meta = FileMeta { len: 4, mtime_ns: 2, dev: 7, ino: 9 };
        doc.set_saved(new_meta, doc.hash(), new_path.clone());
        registry.rekey(doc.id, DocumentKey::File(new_path.clone()));

        assert!(registry.find_path(&path("old.md")).is_none());
        assert!(Rc::ptr_eq(&doc, &registry.find_path(&new_path).unwrap()));
        assert!(registry.find_identity(&old_meta).is_none());
        assert!(Rc::ptr_eq(&doc, &registry.find_identity(&new_meta).unwrap()));
        assert_eq!(*doc.key.borrow(), DocumentKey::File(new_path));
    }

    #[test]
    fn rekey_to_untitled_drops_the_path_but_keeps_the_document() {
        let mut registry = DocumentRegistry::new();
        let doc = registry.insert_loading(DocumentKey::File(path("a.md")), "a.md".into());
        let paste = PasteId(3);
        registry.rekey(doc.id, DocumentKey::Untitled(paste));

        assert!(registry.find_path(&path("a.md")).is_none());
        assert_eq!(registry.get(doc.id).map(|d| d.id), Some(doc.id));
        assert_eq!(*doc.key.borrow(), DocumentKey::Untitled(paste));
    }

    #[test]
    fn collect_unused_waits_for_every_tab_reference() {
        let mut registry = DocumentRegistry::new();
        let doc = registry.insert_loading(DocumentKey::File(path("c.md")), "c.md".into());

        assert!(registry.collect_unused().is_empty(), "the caller still holds the loading document");
        drop(doc);

        assert_eq!(registry.collect_unused(), vec![DocumentId(1)]);
        assert!(registry.get(DocumentId(1)).is_none());
        assert!(registry.find_path(&path("c.md")).is_none());
        assert!(registry.collect_unused().is_empty(), "collection is idempotent");
    }

    #[test]
    fn identity_becomes_searchable_when_a_loading_document_is_completed() {
        let mut registry = DocumentRegistry::new();
        let file = path("loaded.md");
        let doc = registry.insert_loading(DocumentKey::File(file.clone()), "loaded.md".into());
        let meta = FileMeta { len: 4, mtime_ns: 2, dev: 50, ino: 60 };

        doc.replace_all("text", crate::buffer::Eol::Lf);
        doc.set_saved(meta, doc.hash(), file);
        assert!(registry.find_identity(&meta).is_none(), "a loading document has no identity yet");

        registry.insert_ready(doc.clone());
        assert!(Rc::ptr_eq(&doc, &registry.find_identity(&meta).unwrap()));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn untitled_documents_have_no_path_or_identity() {
        let mut registry = DocumentRegistry::new();
        let meta = FileMeta { len: 4, mtime_ns: 1, dev: 30, ino: 40 };
        let doc =
            Document::ready(DocumentId(1), DocumentKey::Untitled(PasteId(9)), "Pasted", SourceBuffer::new("text"));
        doc.set_saved(meta, doc.hash(), path("was.md"));
        registry.insert_ready(Rc::new(doc));

        assert!(!registry.is_empty());
        assert!(registry.find_path(&path("was.md")).is_none());
        assert!(registry.find_identity(&meta).is_none());
    }

    #[test]
    fn removing_a_merged_document_clears_its_path_and_identity_only() {
        let mut registry = DocumentRegistry::new();
        let merged_meta = FileMeta { len: 4, mtime_ns: 1, dev: 3, ino: 4 };
        let merged = ready_doc(DocumentId(1), path("alias.md"), "text", merged_meta);
        let survivor_meta = FileMeta { len: 9, mtime_ns: 2, dev: 3, ino: 5 };
        let survivor = ready_doc(DocumentId(2), path("real.md"), "text", survivor_meta);
        registry.insert_ready(merged.clone());
        registry.insert_ready(survivor.clone());

        assert_eq!(registry.remove(DocumentId(1)).map(|doc| doc.id), Some(DocumentId(1)));
        assert!(registry.get(DocumentId(1)).is_none());
        assert!(registry.find_path(&path("alias.md")).is_none());
        assert!(registry.find_identity(&merged_meta).is_none());
        assert!(Rc::ptr_eq(&survivor, &registry.find_path(&path("real.md")).unwrap()));
        assert!(Rc::ptr_eq(&survivor, &registry.find_identity(&survivor_meta).unwrap()));
        assert!(registry.remove(DocumentId(1)).is_none(), "removing twice is harmless");
    }

    #[test]
    fn unused_documents_with_identity_are_purged_from_both_maps() {
        let mut registry = DocumentRegistry::new();
        let meta = FileMeta { len: 4, mtime_ns: 1, dev: 3, ino: 4 };
        let doc = ready_doc(DocumentId(1), path("d.md"), "text", meta);
        registry.insert_ready(doc);
        assert!(registry.find_identity(&meta).is_some());

        assert_eq!(registry.collect_unused(), vec![DocumentId(1)]);
        assert!(registry.find_identity(&meta).is_none());
        assert!(registry.is_empty());
    }
}
