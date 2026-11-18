//! XDG and portable application directories.
//!
//! Installed builds use `$XDG_CONFIG_HOME/md.lmdv`, `$XDG_DATA_HOME/md.lmdv`,
//! `$XDG_CACHE_HOME/md.lmdv` and `$XDG_STATE_HOME/md.lmdv/logs`. A portable
//! copy (marker `lmdv-portable.txt` beside the executable) keeps everything
//! under `<exe dir>/data` and gets a distinct instance id. `LMDV_HOME`
//! overrides the root, for tests and advanced users.
//!
//! [`AppDirs::resolve_reported`] also says when portable mode could not use
//! `<exe dir>/data` and the normal XDG roots were substituted.

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
    /// Portable mode was selected, but `<exe dir>/data` could not be created.
    /// The normal XDG roots are used instead, so the copy still starts.
    PortableUnavailable {
        /// The portable data root that could not be created.
        root: PathBuf,
        /// The creation error, for the notice.
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
    /// Portable mode creates `<exe dir>/data` while resolving so it can tell
    /// whether the copy is usable; if that fails, the normal XDG roots are
    /// used and [`DirsIssue::PortableUnavailable`] is returned.
    pub fn resolve_reported() -> ResolvedDirs {
        let executable = std::env::current_exe().ok();
        Self::resolve_from_reported(&|name| std::env::var_os(name), executable.as_deref())
    }

    /// Creates config, data, cache, state, logs and drafts with mode `0700`.
    ///
    /// Platforms without POSIX modes create the folders and leave the
    /// inherited ACLs in place.
    pub fn ensure(&self) -> io::Result<()> {
        for path in [&self.config, &self.data, &self.cache, &self.state, &self.logs, &self.drafts_dir()] {
            if path.as_os_str().is_empty() || path.parent().is_none() {
                continue;
            }
            fs::create_dir_all(path)?;
            #[cfg(unix)]
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
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
                match fs::create_dir_all(&root) {
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
