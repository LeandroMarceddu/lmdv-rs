//! Reading a document from disk into a [`SourceBuffer`].
//!
//! The function is blocking and belongs on a worker thread. It strips a UTF-8
//! BOM, detects CRLF and normalizes the buffer to LF; the original byte policy
//! remains on the [`SourceBuffer`] for the next save.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use lmdv_core::{FileMeta, SourceBuffer};

use crate::path_policy::PathError;

/// The largest document that may be opened: 32 MiB.
pub const MAX_DOC_BYTES: u64 = 32 * 1024 * 1024;

/// The size above which a document is a “large document”: 8 MiB.
///
/// This is a warning threshold, not a limit: the app tells the user and opens
/// such a document in viewer mode. [`MAX_DOC_BYTES`] is the hard open cap.
pub const LARGE_DOC_BYTES: u64 = 8 * 1024 * 1024;

/// Whether a document of `len` bytes is large enough for the warning.
///
/// The threshold itself is not large; only a document above it is.
pub fn is_large_document(len: u64) -> bool {
    len > LARGE_DOC_BYTES
}

/// How many times an open/read is retried while another process holds a
/// transient lock.
const READ_ATTEMPTS: usize = 4;
/// Delay between transient-lock retries (total 120 ms, like the original).
const RETRY_DELAY: Duration = Duration::from_millis(40);

/// A document read from disk, ready to become a [`lmdv_core::Document`].
#[derive(Clone, Debug)]
pub struct LoadedFile {
    /// The canonical path that was read.
    pub path: PathBuf,
    /// The LF-normalized buffer with its EOL/BOM policy.
    pub buffer: SourceBuffer,
    /// Metadata of the file that was read.
    pub meta: FileMeta,
}

/// Why a document could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("file not found: {0}")]
    NotFound(PathBuf),
    #[error("not a regular file: {0}")]
    NotRegular(PathBuf),
    #[error("file is {len} bytes; the limit is {max}")]
    TooLarge { len: u64, max: u64 },
    #[error("not valid UTF-8: {0}")]
    NotUtf8(PathBuf),
    #[error("the path is not allowed: {0}")]
    Path(#[from] PathError),
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn io_error(path: &Path, source: io::Error) -> ReadError {
    if source.kind() == io::ErrorKind::NotFound {
        ReadError::NotFound(path.to_path_buf())
    } else {
        ReadError::Io { path: path.to_path_buf(), source }
    }
}

/// Reads a UTF-8 Markdown document, enforcing [`MAX_DOC_BYTES`].
///
/// A missing file is [`ReadError::NotFound`]; the caller keeps the intended
/// path so the window can show “Not found”. Opening and the first read are
/// retried briefly for transient `EBUSY`/`EAGAIN`-style locks left by editors
/// that replace files, and never for permission or not-found errors.
pub fn read_document(path: &Path) -> Result<LoadedFile, ReadError> {
    // L-10/M-30: an explicit open may name a file on a network share; the
    // canonical form keeps that permission so Windows' `\\?\UNC\…` is read
    // too. Device namespaces and drives are still refused.
    let share = crate::path_policy::network_share(path);
    crate::path_policy::reject_network_or_device_within(path, share.as_deref())?;
    let canonical = fs::canonicalize(path).map_err(|source| io_error(path, source))?;
    crate::path_policy::reject_network_or_device_within(&canonical, share.as_deref())?;

    // Check before opening: opening a FIFO blocks until a writer appears.
    let before = fs::metadata(&canonical).map_err(|source| io_error(&canonical, source))?;
    if !before.is_file() {
        return Err(ReadError::NotRegular(canonical));
    }
    if before.len() > MAX_DOC_BYTES {
        return Err(ReadError::TooLarge { len: before.len(), max: MAX_DOC_BYTES });
    }

    let mut last_error = None;
    let mut file = None;
    for attempt in 0..READ_ATTEMPTS {
        match open_document(&canonical) {
            Ok(opened) => {
                file = Some(opened);
                break;
            }
            Err(source) if is_transient(&source) && attempt + 1 < READ_ATTEMPTS => {
                last_error = Some(source);
                thread::sleep(RETRY_DELAY);
            }
            Err(source) => return Err(io_error(&canonical, source)),
        }
    }
    let mut file = match file {
        Some(file) => file,
        None => return Err(io_error(&canonical, last_error.unwrap_or_else(|| io::Error::other("open failed")))),
    };

    let metadata = file.metadata().map_err(|source| io_error(&canonical, source))?;
    if !metadata.is_file() {
        return Err(ReadError::NotRegular(canonical));
    }
    if metadata.len() > MAX_DOC_BYTES {
        return Err(ReadError::TooLarge { len: metadata.len(), max: MAX_DOC_BYTES });
    }
    // O_NONBLOCK is not meaningful for regular files, but leaving it set
    // would be surprising on unusual filesystems; clear it after validation.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        unsafe {
            let flags = libc::fcntl(file.as_raw_fd(), libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
        }
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut last_error = None;
    for attempt in 0..READ_ATTEMPTS {
        bytes.clear();
        if let Err(source) = file.seek(SeekFrom::Start(0)) {
            return Err(io_error(&canonical, source));
        }
        match read_capped(&mut file, &mut bytes) {
            Ok(()) => {
                last_error = None;
                break;
            }
            Err(source) if is_transient(&source) && attempt + 1 < READ_ATTEMPTS => {
                last_error = Some(source);
                thread::sleep(RETRY_DELAY);
            }
            Err(source) => return Err(io_error(&canonical, source)),
        }
    }
    if let Some(source) = last_error {
        return Err(io_error(&canonical, source));
    }
    if bytes.len() as u64 > MAX_DOC_BYTES {
        return Err(ReadError::TooLarge { len: bytes.len() as u64, max: MAX_DOC_BYTES });
    }

    let text = String::from_utf8(bytes).map_err(|_| ReadError::NotUtf8(canonical.clone()))?;
    let buffer = SourceBuffer::from_file(&text);
    let mut meta = meta_from(&metadata);
    meta.len = text.len() as u64;
    Ok(LoadedFile { path: canonical, buffer, meta })
}

/// Opens the canonical target for reading.
///
/// `O_NONBLOCK` keeps a FIFO swapped in after the metadata check from blocking
/// the worker forever; the metadata check below rejects it. `O_NOFOLLOW`
/// refuses a symlink that replaced the canonical target.
#[cfg(unix)]
fn open_document(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path)
}

/// Opens the canonical target for reading.
///
/// Windows has neither `O_NONBLOCK` nor `O_NOFOLLOW`: a named pipe opened for
/// reading blocks until a writer appears, so the metadata check before the open
/// (which already rejects anything that is not a regular file) is what keeps a
/// swapped-in pipe from blocking forever.
#[cfg(not(unix))]
fn open_document(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

/// Reads at most [`MAX_DOC_BYTES`] plus one byte, so an oversized file is
/// rejected without allocating the whole file.
fn read_capped(file: &mut File, out: &mut Vec<u8>) -> io::Result<()> {
    let mut limited = file.take(MAX_DOC_BYTES + 1);
    limited.read_to_end(out)?;
    Ok(())
}

/// Whether a lock held by another process is worth retrying.
#[cfg(unix)]
fn is_transient(error: &io::Error) -> bool {
    match error.raw_os_error() {
        Some(libc::EINTR | libc::EAGAIN | libc::EBUSY | libc::ETIMEDOUT | libc::ETXTBSY) => true,
        _ => matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock),
    }
}

/// Whether a lock held by another process is worth retrying.
///
/// There is no errno table off Unix; Windows reports a replacing editor's lock
/// as `ERROR_SHARING_VIOLATION` (32) or `ERROR_LOCK_VIOLATION` (33), which
/// `std` does not always classify, so both are checked by number.
#[cfg(not(unix))]
fn is_transient(error: &io::Error) -> bool {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    match error.raw_os_error() {
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => true,
        _ => matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock),
    }
}

/// Reads metadata for a regular file, following symlinks.
pub fn probe(path: &Path) -> Result<FileMeta, ReadError> {
    let share = crate::path_policy::network_share(path);
    crate::path_policy::reject_network_or_device_within(path, share.as_deref())?;
    let canonical = fs::canonicalize(path).map_err(|source| io_error(path, source))?;
    crate::path_policy::reject_network_or_device_within(&canonical, share.as_deref())?;
    let metadata = fs::metadata(&canonical).map_err(|source| io_error(&canonical, source))?;
    if !metadata.is_file() {
        return Err(ReadError::NotRegular(path.to_path_buf()));
    }
    Ok(meta_from(&metadata))
}

/// The core `FileMeta` for a `stat` result.
pub(crate) fn meta_from(metadata: &Metadata) -> FileMeta {
    let modified = metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    let (dev, ino) = file_identity(metadata);
    FileMeta {
        len: metadata.len(),
        mtime_ns: modified.map_or(0, |duration| duration.as_nanos().min(u64::MAX as u128) as u64),
        dev,
        ino,
    }
}

/// The stable `(dev, ino)` pair for conflict checks and duplicate-tab dedupe.
#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

/// Windows has no `st_dev`/`st_ino`. Its volume serial and file index are left
/// at zero on purpose: [`FileMeta::identity_known`] then turns identity checks
/// off, and conflicts fall back to the modification time and length, which is
/// how the rest of the pipeline already treats an unknown identity.
#[cfg(not(unix))]
fn file_identity(_metadata: &Metadata) -> (u64, u64) {
    (0, 0)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;
    use lmdv_core::Eol;

    #[test]
    fn reads_utf8_strips_bom_and_detects_crlf() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        fs::write(&path, "\u{feff}one\r\ntwo\r\n".as_bytes()).unwrap();

        let loaded = read_document(&path).unwrap();
        assert_eq!(loaded.buffer.text(), "one\ntwo\n");
        assert_eq!(loaded.buffer.eol(), Eol::CrLf);
        assert!(loaded.buffer.had_bom());
        assert_eq!(loaded.meta.len, 13, "the BOM is counted in the metadata that was read");
        assert!(loaded.meta.mtime_ns > 0);
        #[cfg(unix)]
        assert!(loaded.meta.identity_known(), "Unix reports a stable (dev, ino)");
        #[cfg(not(unix))]
        assert!(!loaded.meta.identity_known(), "Windows leaves the identity unknown; mtime and length guard instead");
    }

    #[test]
    fn missing_and_non_utf8_files_are_distinct_errors() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("missing.md");
        assert!(matches!(read_document(&missing), Err(ReadError::NotFound(_))));

        let invalid = temp.path().join("invalid.md");
        fs::write(&invalid, [0xff, 0xfe, b'a']).unwrap();
        assert!(matches!(read_document(&invalid), Err(ReadError::NotUtf8(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_document_on_a_share_opens_while_a_device_namespace_does_not() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("shared.md");
        fs::write(&path, "# Shared\n").unwrap();
        let spelled = std::path::PathBuf::from(format!("//{}", temp.path().to_string_lossy().trim_start_matches('/')));

        let loaded = read_document(&spelled.join("shared.md")).expect("an explicit open may name a share");
        assert_eq!(loaded.buffer.text(), "# Shared\n");
        assert!(matches!(read_document(std::path::Path::new(r"\\?\C:\x.md")), Err(ReadError::Path(PathError::Device))));

        // Another share is not the document's own and is never read.
        assert!(matches!(
            read_document(std::path::Path::new("//elsewhere/share/x.md")),
            Err(ReadError::NotFound(_) | ReadError::Io { .. })
        ));
    }

    #[test]
    fn oversized_files_are_rejected_from_metadata() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("big.md");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_DOC_BYTES + 1).unwrap();
        assert!(matches!(read_document(&path), Err(ReadError::TooLarge { max: MAX_DOC_BYTES, .. })));
    }

    #[test]
    fn the_large_document_threshold_sits_below_the_open_cap() {
        assert_eq!(LARGE_DOC_BYTES, 8 * 1024 * 1024, "OI-010: 8 MiB");
        const { assert!(LARGE_DOC_BYTES < MAX_DOC_BYTES, "the warning threshold is not the hard cap") };
        assert!(!is_large_document(0));
        assert!(!is_large_document(LARGE_DOC_BYTES), "exactly 8 MiB is not large");
        assert!(is_large_document(LARGE_DOC_BYTES + 1));
        assert!(is_large_document(MAX_DOC_BYTES));
    }

    #[test]
    fn empty_and_exactly_capped_files_are_read() {
        let temp = TempDir::new().unwrap();
        let empty = temp.path().join("empty.md");
        fs::write(&empty, b"").unwrap();
        let loaded = read_document(&empty).unwrap();
        assert_eq!(loaded.buffer.text(), "");
        assert_eq!(loaded.buffer.line_count(), 1);

        let capped = temp.path().join("capped.md");
        let file = File::create(&capped).unwrap();
        file.set_len(MAX_DOC_BYTES).unwrap();
        // The file is all NULs, which is valid UTF-8; reading it is slow but
        // bounded by the cap. The allocation is reused by the buffer.
        let loaded = read_document(&capped).unwrap();
        assert_eq!(loaded.meta.len, MAX_DOC_BYTES);
    }

    #[test]
    fn a_directory_is_not_a_document() {
        let temp = TempDir::new().unwrap();
        assert!(matches!(read_document(temp.path()), Err(ReadError::NotRegular(_))));
        assert!(matches!(probe(temp.path()), Err(ReadError::NotRegular(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_rejected_without_blocking() {
        let temp = TempDir::new().unwrap();
        let fifo = temp.path().join("pipe.md");
        let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(matches!(read_document(&fifo), Err(ReadError::NotRegular(_))));
    }

    // Windows symlinks need a privilege and have no POSIX mode, so the link
    // and permission half of the probe is Unix-only; `identity` is asserted
    // above for both platforms.
    #[cfg(unix)]
    #[test]
    fn probe_follows_symlinks_and_reports_identity() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("real.md");
        let link = temp.path().join("link.md");
        fs::write(&target, b"x").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let meta = probe(&link).unwrap();
        assert_eq!(meta, probe(&target).unwrap());
        assert!(meta.identity_known());

        let mut permissions = fs::metadata(&target).unwrap().permissions();
        permissions.set_mode(0o640);
        fs::set_permissions(&target, permissions).unwrap();
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o640);
    }
}
