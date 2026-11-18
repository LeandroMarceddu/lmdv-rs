//! Path validation for documents and embedded assets.
//!
//! Markdown is untrusted input. Embedded assets are confined to the document
//! folder by default; system paths, network shares, device paths and special
//! files are rejected before anything is opened. See
//! `docs/ARCHITECTURE_DETAIL.md` §9.3.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use lmdv_core::FileMeta;

use crate::read::meta_from;

/// The maximum size of an embedded asset: 32 MiB.
pub const MAX_ASSET_BYTES: u64 = 32 * 1024 * 1024;
/// The maximum decoded image size: 64 megapixels.
pub const MAX_IMAGE_PIXELS: u64 = 64_000_000;
/// The maximum SVG width or height.
pub const MAX_SVG_DIM: u32 = 4096;

/// What may be opened for a document and its embedded assets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathPolicy {
    /// The canonical folder of the document.
    pub doc_root: PathBuf,
    /// A user opt-in that permits assets outside [`Self::doc_root`].
    pub allow_outside: bool,
    /// Maximum asset size; default [`MAX_ASSET_BYTES`].
    pub max_bytes: u64,
    /// Maximum decoded image pixels; default [`MAX_IMAGE_PIXELS`].
    pub max_pixels: u64,
    /// Maximum SVG width or height; default [`MAX_SVG_DIM`].
    pub max_svg_dim: u32,
}

/// A validated regular file and its metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFile {
    /// The canonical path that was validated.
    pub path: PathBuf,
    pub meta: FileMeta,
}

/// Why a path is not allowed.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("the path is empty")]
    Empty,
    #[error("network and UNC paths are not allowed")]
    Network,
    #[error("device paths and reserved device names are not allowed")]
    Device,
    #[error("system path {0} is not readable")]
    System(PathBuf),
    #[error("the path leaves the document folder")]
    OutsideRoot,
    #[error("not a regular file: {0}")]
    NotRegular(PathBuf),
    #[error("file not found: {0}")]
    NotFound(PathBuf),
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("file is {len} bytes; the limit is {max}")]
    TooLarge { len: u64, max: u64 },
}

impl PathError {
    /// Whether the failure was a missing file.
    pub fn is_not_found(&self) -> bool {
        match self {
            Self::NotFound(_) => true,
            Self::Io { source, .. } => source.kind() == io::ErrorKind::NotFound,
            _ => false,
        }
    }
}

impl PathPolicy {
    /// A policy rooted at `doc_root` with the default caps and no opt-in.
    pub fn new(doc_root: impl Into<PathBuf>) -> Self {
        Self {
            doc_root: doc_root.into(),
            allow_outside: false,
            max_bytes: MAX_ASSET_BYTES,
            max_pixels: MAX_IMAGE_PIXELS,
            max_svg_dim: MAX_SVG_DIM,
        }
    }

    /// Returns this policy with the outside-root opt-in changed.
    pub fn with_allow_outside(mut self, allow: bool) -> Self {
        self.allow_outside = allow;
        self
    }

    /// Resolves a relative or absolute path against the document folder and
    /// validates that it is a regular file inside the root.
    ///
    /// Symlinks are followed; the link is accepted only when its canonical
    /// target stays under the canonical root. The size cap is checked from
    /// metadata before any content is read.
    pub fn resolve_asset(&self, path: &Path) -> Result<ResolvedFile, PathError> {
        reject_network_or_device(path)?;
        let candidate = if path.is_absolute() { path.to_path_buf() } else { self.doc_root.join(path) };
        let resolved = self.resolve_existing(&candidate)?;

        if !self.allow_outside {
            let root = fs::canonicalize(&self.doc_root)
                .map_err(|source| PathError::Io { path: self.doc_root.clone(), source })?;
            if !resolved.path.starts_with(&root) {
                return Err(PathError::OutsideRoot);
            }
        }
        if resolved.meta.len > self.max_bytes {
            return Err(PathError::TooLarge { len: resolved.meta.len, max: self.max_bytes });
        }
        Ok(resolved)
    }

    /// Resolves any local regular file, relative to the document folder when
    /// the spelling is relative. Used for document navigation and explicit
    /// opens, which may leave the current document folder.
    pub fn resolve_file(&self, path: &Path) -> Result<ResolvedFile, PathError> {
        reject_network_or_device(path)?;
        let candidate = if path.is_absolute() { path.to_path_buf() } else { self.doc_root.join(path) };
        self.resolve_existing(&candidate)
    }

    /// Resolves an embedded asset and reads its bytes after the size cap has
    /// been checked from metadata. Exporters and image caches use this instead
    /// of opening the path themselves (ADR-017).
    pub fn read_asset(&self, path: &Path) -> Result<(ResolvedFile, Vec<u8>), PathError> {
        let resolved = self.resolve_asset(path)?;
        let bytes = fs::read(&resolved.path).map_err(|source| PathError::Io { path: resolved.path.clone(), source })?;
        if bytes.len() as u64 > self.max_bytes {
            return Err(PathError::TooLarge { len: bytes.len() as u64, max: self.max_bytes });
        }
        Ok((resolved, bytes))
    }

    fn resolve_existing(&self, candidate: &Path) -> Result<ResolvedFile, PathError> {
        let canonical = fs::canonicalize(candidate).map_err(map_io(candidate))?;
        // A symlink may point into a forbidden area even when the spelling
        // did not; check the resolved path too.
        reject_network_or_device(&canonical)?;
        reject_system_path(&canonical)?;

        let metadata = fs::metadata(&canonical).map_err(map_io(&canonical))?;
        if !metadata.is_file() {
            return Err(PathError::NotRegular(canonical));
        }
        let meta = meta_from(&metadata);
        Ok(ResolvedFile { path: canonical, meta })
    }
}

/// Rejects network/UNC paths, Windows device paths and reserved device names
/// before any filesystem access.
///
/// This checks the spelling, not the target: a symlink into a network mount
/// is caught later by canonicalization.
pub fn reject_network_or_device(path: &Path) -> Result<(), PathError> {
    if path.as_os_str().is_empty() {
        return Err(PathError::Empty);
    }

    reject_network_prefix(path)?;

    for component in path.components() {
        if let Component::Normal(name) = component
            && is_reserved_device_name(name)
        {
            return Err(PathError::Device);
        }
    }

    reject_system_path(path)
}

/// The path-spelling check for share and device forms.
///
/// On Unix a share (`//host/share`, `\\server\share`) and a drive- or
/// device-like spelling (`C:`, `\\?\…`, `\\.\…`) are only unsafe as *text*, so
/// the bytes are inspected directly.
#[cfg(unix)]
fn reject_network_prefix(path: &Path) -> Result<(), PathError> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_os_str().as_bytes();
    if bytes.starts_with(b"//") {
        return Err(PathError::Network);
    }
    if bytes.starts_with(b"\\\\") {
        if bytes.starts_with(b"\\\\?\\") || bytes.starts_with(b"\\\\.\\") {
            return Err(PathError::Device);
        }
        return Err(PathError::Network);
    }
    if is_drive_prefix(bytes) {
        return Err(PathError::Device);
    }
    Ok(())
}

/// The path-spelling check for share and device forms.
///
/// Windows encodes all of them in the first component, so the parsed prefix is
/// inspected instead of the bytes. A drive (`C:\…`, and the verbatim form
/// `\\?\C:\…` that [`std::fs::canonicalize`] returns) is an ordinary local
/// root here; only shares and device namespaces are refused.
#[cfg(windows)]
fn reject_network_prefix(path: &Path) -> Result<(), PathError> {
    use std::path::Prefix;

    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return Ok(());
    };
    match prefix.kind() {
        Prefix::Disk(_) | Prefix::VerbatimDisk(_) => Ok(()),
        Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _) => Err(PathError::Network),
        Prefix::DeviceNS(_) | Prefix::Verbatim(_) => Err(PathError::Device),
    }
}

fn reject_system_path(path: &Path) -> Result<(), PathError> {
    for root in ["/proc", "/sys", "/dev"] {
        if path.starts_with(OsStr::new(root)) {
            return Err(PathError::System(PathBuf::from(root)));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_drive_prefix(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_reserved_device_name(name: &OsStr) -> bool {
    let text = name.to_string_lossy();
    let base = text.split('.').next().unwrap_or("").trim_end_matches(' ');
    let lower = base.to_ascii_lowercase();
    if matches!(lower.as_str(), "con" | "prn" | "aux" | "nul" | "conin$" | "conout$") {
        return true;
    }
    for prefix in ["com", "lpt"] {
        if let Some(number) = lower.strip_prefix(prefix)
            && number.len() == 1
            && matches!(number.as_bytes()[0], b'1'..=b'9')
        {
            return true;
        }
    }
    false
}

fn map_io(path: &Path) -> impl FnOnce(io::Error) -> PathError + '_ {
    move |source| {
        if source.kind() == io::ErrorKind::NotFound {
            PathError::NotFound(path.to_path_buf())
        } else {
            PathError::Io { path: path.to_path_buf(), source }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn assets_resolve_relative_to_the_document_folder() {
        let temp = TempDir::new().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        write(&root.join("img/a.png"), b"png");
        let policy = PathPolicy::new(&root);

        let resolved = policy.resolve_asset(Path::new("img/a.png")).unwrap();
        assert_eq!(resolved.path, root.join("img/a.png"));
        assert_eq!(resolved.meta.len, 3);
        assert_eq!(policy.resolve_asset(&root.join("img/a.png")).unwrap().path, resolved.path);
    }

    #[test]
    fn outside_paths_need_the_opt_in() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("docs");
        let outside = temp.path().join("secret.png");
        write(&root.join("note.md"), b"# note\n");
        write(&outside, b"secret");

        let policy = PathPolicy::new(&root);
        assert!(matches!(policy.resolve_asset(Path::new("../secret.png")), Err(PathError::OutsideRoot)));
        assert!(matches!(policy.resolve_asset(&outside), Err(PathError::OutsideRoot)));
        assert_eq!(
            policy.resolve_file(Path::new("note.md")).unwrap().path,
            fs::canonicalize(root.join("note.md")).unwrap(),
            "relative navigation is against the document folder"
        );
        assert!(policy.resolve_file(&outside).is_ok(), "document navigation may leave the folder");

        let open = policy.with_allow_outside(true);
        assert_eq!(open.resolve_asset(&outside).unwrap().path, fs::canonicalize(&outside).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_accepted_only_when_their_target_is_inside() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("docs");
        write(&root.join("real.png"), b"inside");
        write(&temp.path().join("outside.png"), b"outside");
        symlink(root.join("real.png"), root.join("inside-link.png")).unwrap();
        symlink(temp.path().join("outside.png"), root.join("outside-link.png")).unwrap();

        let policy = PathPolicy::new(&root);
        assert!(policy.resolve_asset(Path::new("inside-link.png")).is_ok());
        assert!(matches!(policy.resolve_asset(Path::new("outside-link.png")), Err(PathError::OutsideRoot)));
    }

    // These spellings are rejected as text on Unix; on Windows a drive and the
    // verbatim drive form `fs::canonicalize` returns are ordinary local paths,
    // so only the share/device forms are refused there (see the Windows branch
    // of `reject_network_prefix`).
    #[cfg(unix)]
    #[test]
    fn system_network_and_device_spellings_are_rejected() {
        let policy = PathPolicy::new("/tmp");
        for path in ["//host/share/a.png", r"\\server\share\a.png", r"\\?\C:\x", "C:/x", "NUL", "con.md", "COM1.txt"] {
            let err = policy.resolve_file(Path::new(path)).unwrap_err();
            assert!(matches!(err, PathError::Network | PathError::Device), "{path} should be rejected, got {err:?}");
        }
        assert!(matches!(policy.resolve_file(Path::new("/proc/self/status")), Err(PathError::System(_))));
        assert!(matches!(policy.resolve_file(Path::new("/sys/kernel")), Err(PathError::System(_))));
        assert!(matches!(policy.resolve_file(Path::new("/dev/null")), Err(PathError::System(_))));
        // A prefix that is not a path component is fine.
        assert!(!matches!(policy.resolve_file(Path::new("/devnull/x.png")), Err(PathError::System(_))));
    }

    #[test]
    fn a_directory_is_not_an_asset() {
        let temp = TempDir::new().unwrap();
        let policy = PathPolicy::new(temp.path());
        assert!(matches!(policy.resolve_asset(Path::new(".")), Err(PathError::NotRegular(_))));
    }

    // Sockets and FIFOs need Unix-only creation calls; the Windows build gets
    // the same `NotRegular` rejection from the directory case above.
    #[cfg(unix)]
    #[test]
    fn sockets_and_fifos_are_not_regular_files() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let policy = PathPolicy::new(root);

        let socket = root.join("s.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(matches!(policy.resolve_asset(Path::new("s.sock")), Err(PathError::NotRegular(_))));

        let fifo = root.join("f.fifo");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(matches!(policy.resolve_asset(Path::new("f.fifo")), Err(PathError::NotRegular(_))));

        assert!(matches!(policy.resolve_asset(Path::new(".")), Err(PathError::NotRegular(_))));
    }

    #[test]
    fn missing_files_and_large_files_report_precise_errors() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let policy = PathPolicy::new(root);
        assert!(matches!(policy.resolve_asset(Path::new("none.png")), Err(PathError::NotFound(_))));

        let big = root.join("big.bin");
        fs::File::create(&big).unwrap().set_len(MAX_ASSET_BYTES + 1).unwrap();
        assert!(matches!(
            policy.resolve_asset(Path::new("big.bin")),
            Err(PathError::TooLarge { max: MAX_ASSET_BYTES, .. })
        ));
    }

    #[test]
    fn device_name_matching_is_exact_and_case_insensitive() {
        assert!(is_reserved_device_name(OsStr::new("COM1")));
        assert!(is_reserved_device_name(OsStr::new("lpt9.txt")));
        assert!(is_reserved_device_name(OsStr::new("con.md")));
        assert!(!is_reserved_device_name(OsStr::new("console.md")));
        assert!(!is_reserved_device_name(OsStr::new("COM10.txt")));
    }

    #[test]
    fn read_asset_returns_validated_bytes_and_keeps_the_size_cap() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&root.join("img/a.png"), b"png-bytes");
        let policy = PathPolicy::new(root);

        let (resolved, bytes) = policy.read_asset(Path::new("img/a.png")).unwrap();
        assert_eq!(bytes, b"png-bytes");
        assert!(resolved.path.ends_with("img/a.png"));
        assert!(matches!(policy.read_asset(Path::new("none.png")), Err(PathError::NotFound(_))));

        let big = root.join("big.bin");
        fs::File::create(&big).unwrap().set_len(MAX_ASSET_BYTES + 1).unwrap();
        assert!(matches!(policy.read_asset(Path::new("big.bin")), Err(PathError::TooLarge { .. })));
    }
}
