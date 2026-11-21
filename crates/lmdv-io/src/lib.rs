//! Filesystem, persistence, watcher and single-instance services for lmdv.
//!
//! Everything in this crate is blocking I/O and belongs on a worker thread or
//! inside the store writer; no function here touches Rune or a view. The pure
//! domain values ([`lmdv_core::SourceBuffer`], [`lmdv_core::FileMeta`],
//! [`lmdv_core::Draft`]) stay in `lmdv-core`.
//!
//! * [`launch`] — the original `lmdv [--new-window] [file] [--line N]` parser.
//! * [`path_policy`] — canonicalization and embedded-asset confinement.
//! * [`atomic`] — temp + sync + rename + directory fsync.
//! * [`read`] / [`write`] — UTF-8/EOL/BOM loading and conflict-checked saves.
//! * [`dirs`] — XDG, `LMDV_HOME` and portable directory resolution.
//! * [`store`] — versioned JSON with one coalescing writer thread.
//! * [`watch`] — debounced document and folder-tree watchers.
//! * [`tree`] — Markdown folder listings and folder-wide search.
//! * [`instance`] — the `flock` + Unix-socket single-instance protocol.
//! * [`assoc`] — making lmdv the default Markdown handler: the Windows
//!   registry registration with the Default apps hand-off, and the Linux XDG
//!   desktop entry plus `mimeapps.list` defaults.

pub mod assoc;
pub mod atomic;
pub mod dirs;
pub mod instance;
pub mod launch;
pub mod path_policy;
pub mod read;
pub mod store;
pub mod tree;
pub mod watch;
pub mod write;

pub use dirs::{AppDirs, DirsIssue, ResolvedDirs, documents_dir};
pub use launch::{Launch, absolute_lexical, parse_args};
pub use path_policy::{
    LinkResolution, PathError, PathPolicy, ResolvedFile, network_share, on_document_share, reject_network_or_device,
    reject_network_or_device_within, resolve_document_link,
};
pub use read::{LARGE_DOC_BYTES, LoadedFile, MAX_DOC_BYTES, ReadError, is_large_document, probe, read_document};
pub use tree::{SearchHit, TreeEntry, TreeError, TreeListing, is_markdown_name, list_tree, search_tree};
pub use write::{SaveOutcome, WriteError, save_document};

/// Opens `path` with the desktop's handler (`xdg-open`). Only user-initiated
/// actions should call this, and only from a worker: it spawns a process.
pub fn open_external(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(path).spawn().map(|_| ())
}
