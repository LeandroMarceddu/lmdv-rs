//! Saving a document: conflict check, atomic replacement, CRLF conversion and
//! permission preservation.
//!
//! Saving is explicit. A deleted file is never recreated unless the caller
//! passes `create` (the **Save again** action). A UTF-8 BOM is never written.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use lmdv_core::{Eol, FileMeta};

use crate::atomic::write_atomic_with;
use crate::path_policy::{PathError, reject_network_or_device};
use crate::read::{MAX_DOC_BYTES, probe};

/// The result of a successful save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveOutcome {
    /// The canonical path that was written.
    pub path: PathBuf,
    /// Metadata after the rename, for the next conflict check and watcher
    /// echo suppression.
    pub meta: FileMeta,
    /// Bytes written, including CRLF expansion.
    pub bytes: u64,
}

/// Why a save failed.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The file is gone and `create` was not set.
    #[error("the file was deleted; choose Save again to recreate it")]
    Deleted(PathBuf),
    /// The file changed on disk since the expected metadata was captured.
    #[error("the file changed on disk since it was read")]
    Conflict,
    #[error("the parent folder does not exist: {0}")]
    MissingParent(PathBuf),
    #[error("the target is not a regular file: {0}")]
    NotRegular(PathBuf),
    #[error("the path is not allowed: {0}")]
    Path(#[from] PathError),
    #[error("file would be {len} bytes; the limit is {max}")]
    TooLarge { len: u64, max: u64 },
    #[error("cannot save {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl WriteError {
    /// Whether the failure was a conflict or a deleted file, both of which
    /// have explicit UI actions.
    pub fn needs_decision(&self) -> bool {
        matches!(self, Self::Conflict | Self::Deleted(_))
    }
}

/// Saves LF-normalized `text` to `path`.
///
/// * `expected`: metadata from the load or the previous save. `None` means
///   “overwrite whatever is there”. When present, a changed inode, mtime or
///   length is [`WriteError::Conflict`].
/// * `create`: recreate a file that disappeared, only for **Save again**.
/// * `eol`: CRLF is applied to every LF on write; the buffer itself stays LF.
/// * Permissions of an existing file are preserved; no BOM is ever written.
///
/// The write is atomic (temp file, sync, rename, fsync directory).
pub fn save_document(
    path: &Path,
    text: &str,
    eol: Eol,
    expected: Option<FileMeta>,
    create: bool,
) -> Result<SaveOutcome, WriteError> {
    reject_network_or_device(path)?;

    let (target, check_expected) = match fs::canonicalize(path) {
        Ok(canonical) => (canonical, true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if !create {
                return Err(WriteError::Deleted(path.to_path_buf()));
            }
            let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let name = path.file_name().ok_or_else(|| WriteError::MissingParent(path.to_path_buf()))?;
            let parent = fs::canonicalize(parent).map_err(|_| WriteError::MissingParent(parent.to_path_buf()))?;
            (parent.join(name), false)
        }
        Err(source) => return Err(WriteError::Io { path: path.to_path_buf(), source }),
    };
    // Canonicalization may have followed a symlink into a forbidden area.
    reject_network_or_device(&target)?;

    if let Ok(metadata) = fs::metadata(&target)
        && !metadata.is_file()
    {
        return Err(WriteError::NotRegular(target));
    }

    if check_expected && let Some(expected) = expected {
        let current = probe(&target).map_err(|error| probe_error(error, &target))?;
        if !meta_matches(&expected, &current) {
            return Err(WriteError::Conflict);
        }
    }

    let expected_bytes = if eol == Eol::CrLf {
        text.len() as u64 + text.as_bytes().iter().filter(|&&byte| byte == b'\n').count() as u64
    } else {
        text.len() as u64
    };
    if expected_bytes > MAX_DOC_BYTES {
        return Err(WriteError::TooLarge { len: expected_bytes, max: MAX_DOC_BYTES });
    }

    write_atomic_with(&target, None, |file| write_text(file, text, eol))
        .map_err(|source| WriteError::Io { path: target.clone(), source })?;

    let meta = probe(&target).map_err(|error| probe_error(error, &target))?;
    Ok(SaveOutcome { path: target, meta, bytes: expected_bytes })
}

fn probe_error(error: crate::read::ReadError, target: &Path) -> WriteError {
    match error {
        crate::read::ReadError::NotFound(_) => WriteError::Deleted(target.to_path_buf()),
        crate::read::ReadError::NotRegular(path) => WriteError::NotRegular(path),
        crate::read::ReadError::Io { path, source } => WriteError::Io { path, source },
        crate::read::ReadError::Path(error) => WriteError::Path(error),
        crate::read::ReadError::TooLarge { .. } | crate::read::ReadError::NotUtf8(_) => {
            WriteError::Io { path: target.to_path_buf(), source: io::Error::other("cannot stat the file") }
        }
    }
}

/// Whether `current` still matches the metadata captured when the document
/// was read or saved.
///
/// An all-zero expected meta means “unknown, overwrite”. Otherwise a known
/// `(dev, ino)`, a non-zero mtime or the length must agree. Mtime is compared
/// at nanosecond resolution on Linux.
pub fn meta_matches(expected: &FileMeta, current: &FileMeta) -> bool {
    if *expected == FileMeta::default() {
        return true;
    }
    if expected.identity_known()
        && current.identity_known()
        && (expected.dev, expected.ino) != (current.dev, current.ino)
    {
        return false;
    }
    if expected.mtime_ns != 0 && expected.mtime_ns != current.mtime_ns {
        return false;
    }
    if expected.len != current.len {
        return false;
    }
    true
}

fn write_text(file: &mut fs::File, text: &str, eol: Eol) -> io::Result<()> {
    if eol == Eol::Lf || !text.as_bytes().contains(&b'\n') {
        return file.write_all(text.as_bytes());
    }
    // Stream the conversion so a 32 MiB document does not need a second copy.
    let mut start = 0;
    for (offset, byte) in text.as_bytes().iter().enumerate() {
        if *byte == b'\n' {
            file.write_all(&text.as_bytes()[start..offset])?;
            file.write_all(b"\r\n")?;
            start = offset + 1;
        }
    }
    file.write_all(&text.as_bytes()[start..])
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;
    use crate::read::read_document;

    fn write(path: &Path, text: &str) {
        fs::write(path, text).unwrap();
    }

    #[test]
    fn crlf_is_applied_only_on_write_and_reads_back_normalized() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        write(&path, "old\n");
        let loaded = read_document(&path).unwrap();

        let outcome = save_document(&path, "one\ntwo\n", Eol::CrLf, Some(loaded.meta), false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one\r\ntwo\r\n");
        assert_eq!(outcome.bytes, 10);

        let loaded = read_document(&path).unwrap();
        assert_eq!(loaded.buffer.text(), "one\ntwo\n");
        assert_eq!(loaded.buffer.eol(), Eol::CrLf);
    }

    #[test]
    fn a_bom_is_stripped_on_read_and_never_written() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("bom.md");
        fs::write(&path, "\u{feff}# Title\n").unwrap();
        let loaded = read_document(&path).unwrap();
        assert!(loaded.buffer.had_bom());

        save_document(&path, loaded.buffer.text(), loaded.buffer.eol(), Some(loaded.meta), false).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes, b"# Title\n");
        assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]));
    }

    #[test]
    fn a_changed_file_is_a_conflict() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        write(&path, "one\n");
        let loaded = read_document(&path).unwrap();

        write(&path, "two two\n");
        assert!(matches!(save_document(&path, "mine\n", Eol::Lf, Some(loaded.meta), false), Err(WriteError::Conflict)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "two two\n", "the disk copy was not overwritten");

        save_document(&path, "mine\n", Eol::Lf, None, false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine\n");
    }

    #[test]
    fn a_deleted_file_needs_create_and_is_not_recreated_otherwise() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        write(&path, "one\n");
        let loaded = read_document(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert!(matches!(
            save_document(&path, "again\n", Eol::Lf, Some(loaded.meta), false),
            Err(WriteError::Deleted(_))
        ));
        assert!(!path.exists(), "no ghost file under the old name");

        let outcome = save_document(&path, "again\n", Eol::Lf, Some(loaded.meta), true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "again\n");
        assert_eq!(outcome.path, fs::canonicalize(temp.path()).unwrap().join("note.md"));
    }

    #[cfg(unix)]
    #[test]
    fn permissions_are_preserved() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        write(&path, "one\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        save_document(&path, "two\n", Eol::Lf, None, false).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
    }

    #[test]
    fn final_newline_semantics_are_preserved() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        write(&path, "a\n");
        save_document(&path, "a", Eol::Lf, None, false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"a");
        save_document(&path, "a\n", Eol::Lf, None, false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"a\n");
    }

    #[test]
    fn meta_matching_uses_identity_time_and_length() {
        let base = FileMeta { len: 3, mtime_ns: 10, dev: 1, ino: 2 };
        assert!(meta_matches(&FileMeta::default(), &base), "unknown expected meta overwrites");
        assert!(meta_matches(&base, &base));
        assert!(!meta_matches(&base, &FileMeta { mtime_ns: 11, ..base }));
        assert!(!meta_matches(&base, &FileMeta { len: 4, ..base }));
        assert!(!meta_matches(&base, &FileMeta { ino: 3, ..base }));
        assert!(meta_matches(&FileMeta { dev: 0, ino: 0, ..base }, &FileMeta { dev: 9, ino: 9, ..base }));
    }

    #[test]
    fn writes_reject_network_and_device_paths() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        assert!(matches!(
            save_document(Path::new("//host/share/note.md"), "x", Eol::Lf, None, false),
            Err(WriteError::Path(PathError::Network))
        ));
        assert!(!path.exists());
    }
}
