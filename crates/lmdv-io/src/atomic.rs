//! Atomic file replacement: temp file in the target directory, sync, rename,
//! then fsync the directory.
//!
//! A failure never leaves a half-written target. The temporary file is
//! removed on error. The existing mode is preserved where the platform has
//! POSIX modes; a new file uses the requested mode (or the process umask
//! default). Windows has no modes, so the target keeps the ACL it inherits.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// The canonical path an export may write to: the parent folder must already
/// exist, and the leaf must not be an existing directory.
///
/// The parent is canonicalized so a symlinked destination folder resolves the
/// same way the save dialog showed it. The leaf itself may be a new file
/// (X-15); an existing symlink is refused later by [`write_atomic`].
pub fn canonical_target(path: &Path) -> io::Result<PathBuf> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let Some(name) = path.file_name() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the target has no file name"));
    };
    let canonical_parent = fs::canonicalize(&parent)?;
    if !canonical_parent.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} is not a folder", canonical_parent.display()),
        ));
    }
    let target = canonical_parent.join(name);
    if target.is_dir() {
        return Err(io::Error::new(io::ErrorKind::IsADirectory, format!("{} is a folder", target.display())));
    }
    Ok(target)
}

/// Replaces `path` with `bytes` atomically.
///
/// Parent directories are created. A symlink at `path` is refused: callers
/// pass the canonical target for a document, so replacing the link itself is
/// never intended.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, None, |file| file.write_all(bytes))
}

/// Atomic replacement with a callback that writes the content.
///
/// `new_mode` is applied to a newly created file; an existing file keeps its
/// mode. The mode is set explicitly rather than relying on the umask, which
/// keeps private state files private. Platforms without POSIX modes ignore the
/// argument and leave the target's ACLs alone.
pub fn write_atomic_with(
    path: &Path,
    new_mode: Option<u32>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let Some(name) = path.file_name() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the target has no file name"));
    };

    let existing_mode = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "refusing to replace a symlink"));
        }
        Ok(metadata) => preserved_mode(&metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };

    fs::create_dir_all(&parent)?;

    let mut attempt = 0u32;
    let (temp_path, mut file) = loop {
        let mut temp_name = OsString::from(".");
        temp_name.push(name);
        temp_name.push(format!(".{}.{}.tmp", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
        let candidate = parent.join(temp_name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(new_mode.unwrap_or(0o666));
        match options.open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 16 => attempt += 1,
            Err(error) => return Err(error),
        }
    };

    let mut guard = TempGuard { path: temp_path.clone(), armed: true };
    #[cfg(unix)]
    {
        if let Some(mode) = existing_mode.or(new_mode) {
            fs::set_permissions(&temp_path, fs::Permissions::from_mode(mode))?;
        }
    }
    #[cfg(not(unix))]
    let _ = (existing_mode, new_mode);

    write(&mut file)?;
    file.sync_all()?;
    drop(file);

    fs::rename(&guard.path, path)?;
    guard.armed = false;

    // The rename is durable only after the directory entry is synced. Windows
    // cannot open a directory with `std::fs::File`, so the rename itself is
    // the strongest guarantee the standard library offers there.
    #[cfg(unix)]
    File::open(&parent)?.sync_all()?;
    Ok(())
}

/// The mode an existing target keeps across the replacement.
#[cfg(unix)]
fn preserved_mode(metadata: &fs::Metadata) -> Option<u32> {
    Some(metadata.permissions().mode() & 0o7777)
}

/// Windows has no POSIX mode: the replacement inherits the parent's ACLs.
#[cfg(not(unix))]
fn preserved_mode(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn replaces_content_without_leaving_temporaries() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        fs::write(&path, b"old").unwrap();

        write_atomic(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");

        let names: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["state.json"], "no temp file remains");
    }

    #[test]
    fn creates_the_parent_and_a_new_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("nested/dir/file.json");
        write_atomic(&path, b"{}").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{}");
    }

    #[cfg(unix)]
    #[test]
    fn preserves_the_existing_mode() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("script.md");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        write_atomic(&path, b"new").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn applies_the_requested_mode_to_a_new_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("private.json");
        write_atomic_with(&path, Some(0o600), |file| file.write_all(b"{}")).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_failing_writer_leaves_the_target_untouched() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        fs::write(&path, b"old").unwrap();

        let result = write_atomic_with(&path, Some(0o600), |_| Err(io::Error::other("simulated failure")));
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1, "the temp file was removed");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_replace_a_symlink() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target.json");
        let link = temp.path().join("link.json");
        fs::write(&target, b"old").unwrap();
        symlink(&target, &link).unwrap();

        assert!(write_atomic(&link, b"new").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(link.is_symlink());
    }

    #[cfg(windows)]
    #[test]
    fn refuses_to_replace_a_symlink() {
        use std::os::windows::fs::symlink_file;

        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target.json");
        let link = temp.path().join("link.json");
        fs::write(&target, b"old").unwrap();
        // Developer Mode (or SeCreateSymbolicLinkPrivilege) is required; without
        // it the test would measure the privilege, not the writer.
        if symlink_file(&target, &link).is_err() {
            return;
        }

        assert!(write_atomic(&link, b"new").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(link.is_symlink());
    }

    #[test]
    fn canonical_target_requires_an_existing_parent_and_a_file_leaf() {
        let temp = TempDir::new().unwrap();
        let parent = temp.path().join("out");
        fs::create_dir(&parent).unwrap();
        let target = canonical_target(&parent.join("note.html")).unwrap();
        assert_eq!(target, fs::canonicalize(&parent).unwrap().join("note.html"));

        assert!(canonical_target(&temp.path().join("missing/note.html")).is_err());
        fs::create_dir(temp.path().join("folder.html")).unwrap();
        assert!(canonical_target(&temp.path().join("folder.html")).is_err());
    }
}
