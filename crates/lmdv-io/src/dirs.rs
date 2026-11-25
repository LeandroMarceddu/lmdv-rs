//! XDG and portable application directories.
//!
//! Installed builds use `$XDG_CONFIG_HOME/md.lmdv`, `$XDG_DATA_HOME/md.lmdv`,
//! `$XDG_CACHE_HOME/md.lmdv` and `$XDG_STATE_HOME/md.lmdv/logs`. A portable
//! copy (marker `lmdv-portable.txt` beside the executable) keeps everything
//! under `<exe dir>/data` and gets a distinct instance id. `LMDV_HOME`
//! overrides the root, for tests and advanced users.
//!
//! [`AppDirs::resolve_reported`] also says when portable mode could not use
//! `<exe dir>/data` — because it cannot be created or because the existing
//! folder is not writable — and the normal XDG roots were substituted.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The application directory name inside the XDG roots.
pub const APP_DIR: &str = "md.lmdv";
/// The marker file that selects portable mode.
pub const PORTABLE_MARKER: &str = "lmdv-portable.txt";

/// Why the resolved roots differ from what the environment and the marker
/// asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirsIssue {
    /// Portable mode was selected, but `<exe dir>/data` could not be created
    /// or an existing one is not writable. The normal XDG roots are used
    /// instead, so the copy still starts.
    PortableUnavailable {
        /// The portable data root that could not be used.
        root: PathBuf,
        /// The creation or permission error, for the notice.
        reason: String,
    },
}

/// The roots to use plus anything that went wrong while resolving them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedDirs {
    /// The resolved locations.
    pub dirs: AppDirs,
    /// Empty on the normal path; one entry per fallback.
    pub issues: Vec<DirsIssue>,
}

/// Every location the application writes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDirs {
    /// Preferences, `prefs.json`.
    pub config: PathBuf,
    /// Recents, positions, UI, session and drafts.
    pub data: PathBuf,
    /// Optional caches, such as math SVG.
    pub cache: PathBuf,
    /// Logs and crash state.
    pub state: PathBuf,
    /// Crash logs only; no routine writes.
    pub logs: PathBuf,
    /// Whether portable mode selected the root.
    pub portable: bool,
    /// The single-instance id (`main` or a portable/test-specific value).
    pub instance_id: String,
}

impl AppDirs {
    /// Resolves the directories from the process environment and executable.
    ///
    /// This never fails: a missing environment falls back to the home
    /// directory, and then to a private temporary root, so the application can
    /// still start degraded. A portable copy whose data root cannot be created
    /// falls back to the normal roots; this function drops that issue, so
    /// callers that show a notice should use [`Self::resolve_reported`].
    pub fn resolve() -> Self {
        Self::resolve_reported().dirs
    }

    /// Resolves the directories and reports any fallback.
    ///
    /// Portable mode creates `<exe dir>/data` while resolving, and checks an
    /// existing one for write access, so it can tell whether the copy is
    /// usable; if not, the normal XDG roots are used and
    /// [`DirsIssue::PortableUnavailable`] is returned.
    pub fn resolve_reported() -> ResolvedDirs {
        let executable = std::env::current_exe().ok();
        Self::resolve_from_reported(&|name| std::env::var_os(name), executable.as_deref())
    }

    /// Creates config, data, cache, state, logs and drafts with mode `0700`.
    ///
    /// Platforms without POSIX modes create the folders. Windows gives each of
    /// them a protected DACL that grants the current user full control and lets
    /// that entry inherit into the state, draft and log files written inside,
    /// so another account cannot read them. The ACL step is best effort: a
    /// filesystem without ACL support (a FAT portable copy) keeps the profile
    /// folder's own per-user ACL instead of failing startup.
    pub fn ensure(&self) -> io::Result<()> {
        for path in [&self.config, &self.data, &self.cache, &self.state, &self.logs, &self.drafts_dir()] {
            if path.as_os_str().is_empty() || path.parent().is_none() {
                continue;
            }
            fs::create_dir_all(path)?;
            #[cfg(unix)]
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            #[cfg(windows)]
            windows_privacy::protect(path);
        }
        Ok(())
    }

    /// `prefs.json`.
    pub fn prefs_path(&self) -> PathBuf {
        self.config.join("prefs.json")
    }

    /// `recents.json`.
    pub fn recents_path(&self) -> PathBuf {
        self.data.join("recents.json")
    }

    /// `positions.json`.
    pub fn positions_path(&self) -> PathBuf {
        self.data.join("positions.json")
    }

    /// `ui.json`.
    pub fn ui_path(&self) -> PathBuf {
        self.data.join("ui.json")
    }

    /// `session.json`.
    pub fn session_path(&self) -> PathBuf {
        self.data.join("session.json")
    }

    /// The one-file-per-draft folder.
    pub fn drafts_dir(&self) -> PathBuf {
        self.data.join("drafts")
    }

    /// The optional math SVG cache folder.
    pub fn math_cache_dir(&self) -> PathBuf {
        self.cache.join("math")
    }

    #[cfg(test)]
    fn resolve_from(get: &dyn Fn(&str) -> Option<OsString>, executable: Option<&Path>) -> Self {
        Self::resolve_from_reported(get, executable).dirs
    }

    fn resolve_from_reported(get: &dyn Fn(&str) -> Option<OsString>, executable: Option<&Path>) -> ResolvedDirs {
        let lexical = |path: PathBuf| crate::launch::absolute_lexical(&path);
        let mut issues = Vec::new();

        if let Some(home) = get("LMDV_HOME").filter(|value| !value.is_empty()) {
            let root = lexical(PathBuf::from(home));
            let state = root.join("state");
            return ResolvedDirs {
                dirs: Self {
                    config: root.join("config"),
                    data: root.clone(),
                    cache: root.join("cache"),
                    logs: state.join("logs"),
                    state,
                    portable: false,
                    instance_id: format!("local-{}", short_hash(root.as_os_str())),
                },
                issues,
            };
        }

        if let Some(executable) = executable
            && let Some(exe_dir) = executable.parent()
        {
            let marker = exe_dir.join(PORTABLE_MARKER);
            let has_marker = fs::symlink_metadata(&marker).is_ok_and(|metadata| metadata.is_file());
            if has_marker {
                let root = lexical(exe_dir.join("data"));
                match prepare_portable_root(&root) {
                    Ok(()) => {
                        let state = root.join("state");
                        return ResolvedDirs {
                            dirs: Self {
                                config: root.join("config"),
                                data: root.clone(),
                                cache: root.join("cache"),
                                logs: state.join("logs"),
                                state,
                                portable: true,
                                instance_id: format!("portable-{}", short_hash(exe_dir.as_os_str())),
                            },
                            issues,
                        };
                    }
                    Err(error) => {
                        // A read-only USB stick or an unpacked archive: keep the
                        // copy usable on the normal roots and let the app say so.
                        issues.push(DirsIssue::PortableUnavailable { root, reason: error.to_string() });
                    }
                }
            }
        }

        // Root resolution. Unix follows the XDG spec through the injected
        // environment. Windows has no HOME/XDG variables in practice, so the
        // `dirs` profile folders stand in for them; an explicit XDG value still
        // wins there, which keeps test roots working.
        #[cfg(unix)]
        let home = get("HOME").map(PathBuf::from).filter(|path| path.is_absolute());
        #[cfg(unix)]
        let config_root = xdg_path(get, "XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|home| home.join(".config")));
        #[cfg(unix)]
        let data_root = xdg_path(get, "XDG_DATA_HOME").or_else(|| home.as_ref().map(|home| home.join(".local/share")));
        #[cfg(unix)]
        let cache_root = xdg_path(get, "XDG_CACHE_HOME").or_else(|| home.as_ref().map(|home| home.join(".cache")));
        #[cfg(unix)]
        let state_root =
            xdg_path(get, "XDG_STATE_HOME").or_else(|| home.as_ref().map(|home| home.join(".local/state")));

        #[cfg(not(unix))]
        let config_root = xdg_path(get, "XDG_CONFIG_HOME").or_else(dirs::config_dir);
        #[cfg(not(unix))]
        let data_root = xdg_path(get, "XDG_DATA_HOME").or_else(dirs::data_local_dir);
        #[cfg(not(unix))]
        let cache_root = xdg_path(get, "XDG_CACHE_HOME").or_else(dirs::cache_dir);
        #[cfg(not(unix))]
        let state_root = xdg_path(get, "XDG_STATE_HOME").or_else(dirs::data_local_dir);

        // A missing environment is rare but must not panic or write next to the
        // CWD.
        let fallback = || lexical(fallback_root());
        let config_root = config_root.unwrap_or_else(&fallback);
        let data_root = data_root.unwrap_or_else(&fallback);
        let cache_root = cache_root.unwrap_or_else(&fallback);
        let state_root = state_root.unwrap_or_else(&fallback);

        let config = lexical(config_root.join(APP_DIR));
        let data = lexical(data_root.join(APP_DIR));
        let cache = lexical(cache_root.join(APP_DIR));
        let state = lexical(state_root.join(APP_DIR));
        ResolvedDirs {
            dirs: Self {
                logs: state.join("logs"),
                config,
                data,
                cache,
                state,
                portable: false,
                instance_id: "main".to_owned(),
            },
            issues,
        }
    }
}

fn xdg_path(get: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    let value = get(name)?;
    let path = PathBuf::from(value);
    if path.is_absolute() { Some(path) } else { None }
}

/// Makes sure the portable data root can be used for this session.
///
/// A missing root is created. An existing root is only checked, never probed
/// with a file, so a read-only USB stick, a folder owned by another user or a
/// folder without write permission falls back to XDG before anything is
/// written inside it (OI-170).
fn prepare_portable_root(root: &Path) -> io::Result<()> {
    match fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => {
            if is_writable_dir(root) {
                Ok(())
            } else {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "the folder is not writable"))
            }
        }
        Ok(_) => Err(io::Error::new(io::ErrorKind::NotADirectory, "the path is not a folder")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(root),
        Err(error) => Err(error),
    }
}

/// Whether `path` admits new entries for the current user.
///
/// On Unix `access(2)` answers from the inode's mode, owner and the mount's
/// read-only flag without creating anything in the folder.
#[cfg(unix)]
fn is_writable_dir(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let mut bytes = path.as_os_str().as_bytes().to_vec();
    if bytes.contains(&0) {
        return false;
    }
    bytes.push(0);
    unsafe { libc::access(bytes.as_ptr().cast(), libc::W_OK) == 0 }
}

/// Windows exposes no portable write-permission check for a folder, so a
/// uniquely named folder is created and removed. This is the one platform
/// where answering costs a probe; a denied folder is then reported before the
/// app writes into it.
#[cfg(windows)]
fn is_writable_dir(path: &Path) -> bool {
    let probe = path.join(format!(".lmdv-write-probe-{}", std::process::id()));
    match fs::create_dir(&probe) {
        Ok(()) => {
            let _ = fs::remove_dir(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(not(any(unix, windows)))]
fn is_writable_dir(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| !metadata.permissions().readonly())
}

/// The degraded root used when neither the environment nor the profile
/// resolves: a private temp folder per user.
#[cfg(unix)]
fn fallback_root() -> PathBuf {
    std::env::temp_dir().join(format!("lmdv-{}", unsafe { libc::geteuid() }))
}

/// Windows has no uid to keep two users' temp roots apart, so the per-user
/// local application folder is the degraded root and the temp folder is only
/// the last resort for a process with no profile at all.
#[cfg(not(unix))]
fn fallback_root() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("lmdv")
}

/// The user's Documents folder, as the desktop defines it (`XDG_DOCUMENTS_DIR`
/// on Linux, the known folder on Windows).
///
/// Save as uses it for pasted text when the file browser has no folder yet
/// (E-11). A platform without a Documents folder returns `None`.
pub fn documents_dir() -> Option<PathBuf> {
    dirs::document_dir()
}

/// A short stable hash of a path, used to isolate portable and `LMDV_HOME`
/// instances. `as_encoded_bytes` is the same UTF-8 byte view on Unix.
fn short_hash(value: &OsStr) -> String {
    let hash = blake3::hash(value.as_encoded_bytes());
    hash.to_hex()[..12].to_owned()
}

/// Windows state-folder privacy.
///
/// `%LOCALAPPDATA%` normally grants only the signed-in user (plus SYSTEM and
/// administrators), but a redirected or roaming profile can inherit a wider
/// ACL. Each folder lmdv owns therefore gets a protected DACL with one entry:
/// the current user's SID, full control, inherited by subfolders and files.
/// State, drafts and logs written inside are then readable only by that user.
///
/// This is best effort. A filesystem without ACL support (a FAT portable copy)
/// or a denied security call keeps the inherited profile ACL rather than
/// failing startup; the folder is still per-user on a normal account.
#[cfg(windows)]
mod windows_privacy {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW,
        TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Gives `path` a protected DACL for the current user; failures are left to
    /// the inherited profile ACL.
    pub(super) fn protect(path: &Path) {
        let _ = set_private_dacl(path);
    }

    fn set_private_dacl(path: &Path) -> std::io::Result<()> {
        let token = open_process_token()?;

        // A null buffer only asks for the required length.
        let mut needed = 0u32;
        // SAFETY: a null buffer with length 0 is the documented size query.
        unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        let length = u32::try_from(buffer.len() * 8).unwrap_or(u32::MAX);
        // SAFETY: the buffer is at least `needed` bytes, 8-aligned for
        // `TOKEN_USER`, and lives until the SID has been copied into the ACL
        // below.
        let status = unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, buffer.as_mut_ptr().cast(), length, &mut needed)
        };
        if status == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: on success the buffer holds one TOKEN_USER whose SID lives in
        // the same allocation.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };

        let access = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: user.User.Sid.cast(),
            },
        };
        let mut acl: *mut ACL = ptr::null_mut();
        let status = unsafe { SetEntriesInAclW(1, &access, ptr::null(), &mut acl) };
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status as i32));
        }

        let name: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let status = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null(),
            )
        };
        // SAFETY: `SetEntriesInAclW` returned this allocation and the call above
        // has finished with it.
        unsafe {
            LocalFree(acl.cast());
        }
        if status == ERROR_SUCCESS { Ok(()) } else { Err(std::io::Error::from_raw_os_error(status as i32)) }
    }

    /// Opens this process's token for a `TokenUser` query.
    fn open_process_token() -> std::io::Result<OwnedHandle> {
        let mut token = ptr::null_mut();
        // SAFETY: `GetCurrentProcess` is a pseudo-handle; a successful
        // `OpenProcessToken` returns a handle this function then owns.
        let status = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if status == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `token` is a valid handle returned by `OpenProcessToken`.
        Ok(unsafe { OwnedHandle::from_raw_handle(token) })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::TempDir;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> =
            pairs.iter().map(|(key, value)| ((*key).to_owned(), OsString::from(value))).collect();
        move |key| map.get(key).cloned()
    }

    // The XDG/HOME assertions spell absolute POSIX paths, so they only run
    // where those spellings are absolute; the portable-marker and `ensure`
    // tests below cover the platform-neutral behavior.
    #[cfg(unix)]
    #[test]
    fn xdg_variables_select_every_root() {
        let get = env(&[
            ("XDG_CONFIG_HOME", "/x/config"),
            ("XDG_DATA_HOME", "/x/data"),
            ("XDG_CACHE_HOME", "/x/cache"),
            ("XDG_STATE_HOME", "/x/state"),
        ]);
        let dirs = AppDirs::resolve_from(&get, None);
        assert_eq!(dirs.config, PathBuf::from("/x/config/md.lmdv"));
        assert_eq!(dirs.data, PathBuf::from("/x/data/md.lmdv"));
        assert_eq!(dirs.cache, PathBuf::from("/x/cache/md.lmdv"));
        assert_eq!(dirs.state, PathBuf::from("/x/state/md.lmdv"));
        assert_eq!(dirs.logs, PathBuf::from("/x/state/md.lmdv/logs"));
        assert!(!dirs.portable);
        assert_eq!(dirs.instance_id, "main");
        assert_eq!(dirs.prefs_path(), PathBuf::from("/x/config/md.lmdv/prefs.json"));
    }

    #[cfg(unix)]
    #[test]
    fn home_fallbacks_follow_the_xdg_defaults() {
        let get = env(&[("HOME", "/home/user")]);
        let dirs = AppDirs::resolve_from(&get, None);
        assert_eq!(dirs.config, PathBuf::from("/home/user/.config/md.lmdv"));
        assert_eq!(dirs.data, PathBuf::from("/home/user/.local/share/md.lmdv"));
        assert_eq!(dirs.cache, PathBuf::from("/home/user/.cache/md.lmdv"));
        assert_eq!(dirs.state, PathBuf::from("/home/user/.local/state/md.lmdv"));
    }

    #[cfg(unix)]
    #[test]
    fn relative_xdg_values_are_ignored() {
        let get = env(&[("HOME", "/home/user"), ("XDG_DATA_HOME", "relative/data")]);
        let dirs = AppDirs::resolve_from(&get, None);
        assert_eq!(dirs.data, PathBuf::from("/home/user/.local/share/md.lmdv"));
    }

    #[cfg(unix)]
    #[test]
    fn lmdv_home_overrides_the_root_and_isolates_the_instance() {
        let get = env(&[("HOME", "/home/user"), ("LMDV_HOME", "/tmp/lmdv-test")]);
        let dirs = AppDirs::resolve_from(&get, None);
        assert_eq!(dirs.config, PathBuf::from("/tmp/lmdv-test/config"));
        assert_eq!(dirs.data, PathBuf::from("/tmp/lmdv-test"));
        assert_eq!(dirs.cache, PathBuf::from("/tmp/lmdv-test/cache"));
        assert_eq!(dirs.state, PathBuf::from("/tmp/lmdv-test/state"));
        assert!(!dirs.portable);
        assert!(dirs.instance_id.starts_with("local-"));
    }

    #[test]
    fn the_portable_marker_beside_the_executable_wins_over_xdg() {
        let temp = TempDir::new().unwrap();
        let exe_dir = temp.path().join("portable");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::write(exe_dir.join(PORTABLE_MARKER), b"").unwrap();
        let executable = exe_dir.join("lmdv");
        let get = env(&[("HOME", "/home/user"), ("XDG_DATA_HOME", "/x/data")]);

        let resolved = AppDirs::resolve_from_reported(&get, Some(&executable));
        assert!(resolved.issues.is_empty());
        let dirs = resolved.dirs;
        assert!(dirs.portable);
        assert_eq!(dirs.data, exe_dir.join("data"));
        assert!(dirs.data.is_dir(), "resolving a usable portable root creates it");
        assert_eq!(dirs.config, exe_dir.join("data").join("config"));
        assert!(dirs.instance_id.starts_with("portable-"));

        let other = AppDirs::resolve_from(&get, None);
        assert_ne!(dirs.instance_id, other.instance_id, "installed and portable copies coexist");
    }

    #[test]
    fn a_portable_root_that_cannot_be_created_falls_back_to_xdg() {
        let temp = TempDir::new().unwrap();
        let exe_dir = temp.path().join("portable-readonly");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::write(exe_dir.join(PORTABLE_MARKER), b"").unwrap();
        // A regular file where the data folder should be makes `create_dir_all`
        // fail on every platform, without relying on POSIX modes or root.
        fs::write(exe_dir.join("data"), b"not a folder").unwrap();
        let executable = exe_dir.join("lmdv");
        let get = env(&[("HOME", "/home/user"), ("XDG_DATA_HOME", "/x/data")]);

        let resolved = AppDirs::resolve_from_reported(&get, Some(&executable));
        assert!(!resolved.dirs.portable, "the unusable portable root is not selected");
        assert_eq!(resolved.dirs.instance_id, "main", "the fallback joins the installed instance");
        assert_ne!(resolved.dirs.data, exe_dir.join("data"));
        assert_eq!(resolved.issues.len(), 1);
        match &resolved.issues[0] {
            DirsIssue::PortableUnavailable { root, reason } => {
                assert_eq!(root, &exe_dir.join("data"));
                assert!(!reason.is_empty(), "the notice has a reason");
            }
        }
        #[cfg(unix)]
        assert_eq!(resolved.dirs.data, PathBuf::from("/x/data/md.lmdv"), "the XDG root is used");
    }

    #[cfg(unix)]
    #[test]
    fn a_pre_existing_portable_root_that_is_not_writable_falls_back_to_xdg() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            // root ignores the permission bits, so no folder is unwritable.
            return;
        }
        let temp = TempDir::new().unwrap();
        let exe_dir = temp.path().join("portable-full");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::write(exe_dir.join(PORTABLE_MARKER), b"").unwrap();
        let data = exe_dir.join("data");
        fs::create_dir_all(&data).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o555)).unwrap();
        let executable = exe_dir.join("lmdv");
        let get = env(&[("HOME", "/home/user"), ("XDG_DATA_HOME", "/x/data")]);

        let resolved = AppDirs::resolve_from_reported(&get, Some(&executable));
        assert!(!resolved.dirs.portable, "an unwritable portable root is not selected");
        assert_eq!(resolved.dirs.instance_id, "main", "the fallback joins the installed instance");
        assert_eq!(resolved.dirs.data, PathBuf::from("/x/data/md.lmdv"), "the XDG root is used");
        match &resolved.issues[..] {
            [DirsIssue::PortableUnavailable { root, reason }] => {
                assert_eq!(root, &data);
                assert!(!reason.is_empty(), "the notice has a reason");
            }
            other => panic!("expected one portable-unavailable issue, got {other:?}"),
        }
        assert_eq!(fs::read_dir(&data).unwrap().count(), 0, "no probe file is written into the folder");

        // Let `TempDir` remove the tree.
        fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn ensure_creates_private_directories() {
        let temp = TempDir::new().unwrap();
        let dirs = AppDirs {
            config: temp.path().join("config"),
            data: temp.path().join("data"),
            cache: temp.path().join("cache"),
            state: temp.path().join("state"),
            logs: temp.path().join("state/logs"),
            portable: false,
            instance_id: "main".to_owned(),
        };
        dirs.ensure().unwrap();
        for path in [&dirs.config, &dirs.data, &dirs.cache, &dirs.state, &dirs.logs, &dirs.drafts_dir()] {
            assert!(path.is_dir(), "{}", path.display());
            #[cfg(unix)]
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }
}
