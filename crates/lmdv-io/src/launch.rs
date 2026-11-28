//! The original command-line grammar: `lmdv [--new-window] [file] [--line N]`,
//! plus the session extension `--restore-session`.
//!
//! The parser is deliberately small and does not validate that a file exists:
//! a missing file still gets a full path so the window can show “Not found”
//! and tab matching keeps working. See the upstream `src/lib.rs:171-204`
//! and `docs/BEHAVIOR_SPEC.md` §3.15.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What a command line asks for.
///
/// `line` is 1-based. `cwd` is the absolute, lexically normalized directory
/// against which a relative `path` was resolved; it is kept so a forwarded
/// launch can still explain or re-resolve the request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Launch {
    /// The first positional argument, absolute and normalized, or `None`.
    pub path: Option<PathBuf>,
    /// 1-based source line to show at the top.
    pub line: Option<usize>,
    /// `--new-window`: a window of its own, whatever other windows hold.
    pub new_window: bool,
    /// `--restore-session`: restore the stored windows even when this launch
    /// also names a file (the file still wins for the first window).
    pub restore_session: bool,
    /// The client's working directory, absolute and lexically normalized.
    pub cwd: PathBuf,
    /// A compositor activation token (`XDG_ACTIVATION_TOKEN` on Wayland,
    /// `DESKTOP_STARTUP_ID` on X11) forwarded with the launch so the window
    /// that opens it can take focus (OI-117). Grammar parsing stays pure; the
    /// entry point fills this from [`activation_token_from_env`].
    pub activation_token: Option<String>,
}

/// The activation token the desktop gave this process, when it gave one.
///
/// Wayland compositors only grant focus to a window whose launch can prove it
/// came from an interaction; the token travels with the launch request and is
/// handed to Rune's `WindowHandle::activate`. X11's startup id is the equivalent
/// older spelling.
pub fn activation_token_from_env() -> Option<String> {
    ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|token| !token.is_empty())
}

/// Parses the original CLI grammar.
///
/// `args` includes the program name, exactly like `std::env::args()`.
/// Relative paths are resolved against `cwd` without touching the disk;
/// `.` and `..` are folded lexically. `--new-window` and `--restore-session`
/// (with its `--no-restore-session` negation) are accepted anywhere before
/// `--`; unknown flags beginning with `-` are ignored; only the first
/// positional path is used; a non-numeric value after `--line`/`-l` or after
/// `--line=` is ignored. A `--line` with no following argument consumes
/// nothing and leaves the line unset, matching the original.
pub fn parse_args<I, S>(args: I, cwd: impl AsRef<Path>) -> Launch
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let cwd = absolute_lexical(cwd.as_ref());
    let mut it = args.into_iter().skip(1);
    let mut launch = Launch { cwd: cwd.clone(), ..Launch::default() };
    let mut positional_only = false;

    while let Some(arg) = it.next() {
        let Some(text) = arg.as_ref().to_str() else {
            // A non-UTF-8 argument cannot be a flag; treat it as a path.
            if launch.path.is_none() {
                launch.path = Some(resolve_path(Path::new(arg.as_ref()), &cwd));
            }
            continue;
        };

        match text {
            "--" if !positional_only => positional_only = true,
            "--new-window" if !positional_only => launch.new_window = true,
            "--restore-session" if !positional_only => launch.restore_session = true,
            "--no-restore-session" if !positional_only => launch.restore_session = false,
            "--line" | "-l" if !positional_only => {
                launch.line = it.next().and_then(|value| value.as_ref().to_str()?.parse().ok());
            }
            value if !positional_only && value.starts_with("--line=") => {
                launch.line = value[7..].parse().ok();
            }
            value if !positional_only && value.starts_with('-') => {}
            value if launch.path.is_none() => {
                launch.path = Some(resolve_path(Path::new(value), &cwd));
            }
            _ => {}
        }
    }

    launch
}

/// Lexically absolutizes `path` against `cwd` and folds `.`/`..`, without
/// requiring the path to exist.
///
/// This is the normalization used by the CLI and by settings paths. It never
/// follows symlinks; callers that need a canonical on-disk path use
/// [`std::fs::canonicalize`] (or [`crate::path_policy::PathPolicy`]).
pub fn absolute_lexical(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(current) => current.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    normalize_lexical(&joined)
}

fn resolve_path(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() { normalize_lexical(path) } else { normalize_lexical(&cwd.join(path)) }
}

fn normalize_lexical(path: &Path) -> PathBuf {
    // POSIX leaves exactly two leading slashes implementation-defined. Keep
    // them so a `//host/share` spelling reaches the path policy and is
    // rejected there instead of silently becoming `/host/share`.
    //
    // Windows has no such spelling: `//host/share` parses as a UNC prefix and
    // is preserved (and rejected) as one, so nothing extra is kept here.
    #[cfg(unix)]
    let double_root = {
        use std::os::unix::ffi::OsStrExt;

        let raw = path.as_os_str().as_bytes();
        raw.starts_with(b"//") && !raw.starts_with(b"///")
    };
    #[cfg(not(unix))]
    let double_root = false;

    let mut out = PathBuf::new();
    let mut skip_root = double_root;
    if double_root {
        out.push("//");
    }
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => {
                if skip_root {
                    skip_root = false;
                } else if !out.has_root() {
                    // `PathBuf::push` would add a second separator after a
                    // drive or share prefix, which already roots the path.
                    out.push(Component::RootDir.as_os_str());
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                let pops = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if pops {
                    out.pop();
                } else if !out.has_root() {
                    out.push(Component::ParentDir.as_os_str());
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(Component::CurDir.as_os_str());
    }
    out
}

// The grammar tests spell absolute POSIX paths (and assert the POSIX
// double-slash rule), so they run on Unix; the parser itself is shared.
#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("lmdv").chain(list.iter().copied()).map(str::to_owned).collect()
    }

    const CWD: &str = "/home/user/notes";

    fn parse(list: &[&str]) -> Launch {
        parse_args(args(list), CWD)
    }

    #[test]
    fn full_paths_need_not_exist() {
        let launch = parse(&["../todo.md", "--line", "40"]);
        assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/todo.md")));
        assert_eq!(launch.line, Some(40));
        assert_eq!(launch.cwd, PathBuf::from(CWD));

        let launch = parse(&["./gone/x.md"]);
        assert_eq!(
            launch.path.as_deref(),
            Some(Path::new("/home/user/notes/gone/x.md")),
            "a missing file keeps a path"
        );

        let launch = parse(&["a/b.md", "-l", "3"]);
        assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/notes/a/b.md")));
        assert_eq!(launch.line, Some(3));

        let launch = parse(&["/x/../y.md", "--line=7"]);
        assert_eq!(launch.path.as_deref(), Some(Path::new("/y.md")), ".. is folded without the disk");
        assert_eq!(launch.line, Some(7));

        let launch = parse(&["--unknown", "--", "-dash.md", "second.md"]);
        assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/notes/-dash.md")), "-- ends the options");
        assert_eq!(launch.line, None);
        assert!(!launch.new_window);
        assert_eq!(parse(&[]).path, None);
    }

    #[test]
    fn new_window_is_taken_anywhere_before_the_end_of_the_options() {
        let launch = parse(&["--new-window", "x.md"]);
        assert!(launch.new_window);
        assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/notes/x.md")));

        let launch = parse(&["x.md", "--line", "5", "--new-window"]);
        assert!(launch.new_window, "after the file");
        assert_eq!((launch.path.as_deref(), launch.line), (Some(Path::new("/home/user/notes/x.md")), Some(5)));

        let launch = parse(&["--new-window"]);
        assert!(launch.new_window && launch.path.is_none(), "no file: an empty window");
        assert!(!parse(&["x.md"]).new_window);

        let launch = parse(&["--", "--new-window"]);
        assert!(!launch.new_window, "after --, a file of that name");
        assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/notes/--new-window")));
    }

    #[test]
    fn restore_session_is_an_optional_flag_that_can_be_negated() {
        let launch = parse(&["--restore-session", "a.md", "--line=2"]);
        assert!(launch.restore_session);
        assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/notes/a.md")));
        assert_eq!(launch.line, Some(2));
        assert!(!launch.new_window);

        let launch = parse(&["a.md", "--restore-session", "--no-restore-session"]);
        assert!(!launch.restore_session, "the last spelling wins");

        let launch = parse(&["--", "--restore-session"]);
        assert!(!launch.restore_session, "after -- it is a file name");
        assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/notes/--restore-session")));

        assert!(!parse(&[]).restore_session);
        assert!(!parse(&["--restore-session=1"]).restore_session, "unknown spellings stay flags");
    }

    #[test]
    fn only_the_first_positional_is_used() {
        let launch = parse(&["one.md", "two.md", "three.md"]);
        assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/notes/one.md")));
    }

    #[test]
    fn unknown_flags_are_ignored_and_never_become_a_path() {
        for flag in ["--help", "-h", "--version", "-l40", "--new-window=1", "--line=abc", "-"] {
            let launch = parse(&[flag, "file.md"]);
            assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/notes/file.md")), "flag {flag}");
            assert_eq!(launch.line, None, "flag {flag}");
        }
    }

    #[test]
    fn line_values_are_ignored_when_they_do_not_parse() {
        assert_eq!(parse(&["file.md", "--line", "abc"]).line, None);
        assert_eq!(parse(&["file.md", "--line"]).line, None, "a missing value consumes nothing");
        assert_eq!(parse(&["file.md", "--line="]).line, None);
        assert_eq!(parse(&["file.md", "--line", "-5"]).line, None);
        assert_eq!(parse(&["file.md", "--line", "0"]).line, Some(0), "the original parses 0");
        assert_eq!(parse(&["file.md", "--line=12"]).line, Some(12));
    }

    #[test]
    fn a_line_value_that_looks_like_a_flag_is_consumed() {
        let launch = parse(&["--line", "--new-window", "file.md"]);
        assert_eq!(launch.line, None);
        assert!(!launch.new_window, "the original consumes the next argument as the line value");
        assert_eq!(launch.path.as_deref(), Some(Path::new("/home/user/notes/file.md")));
    }

    #[test]
    fn dot_and_parent_are_folded_and_roots_are_clamped() {
        assert_eq!(parse(&["./a/./b/../c.md"]).path.as_deref(), Some(Path::new("/home/user/notes/a/c.md")));
        assert_eq!(parse(&["../../../../etc/hosts"]).path.as_deref(), Some(Path::new("/etc/hosts")));
        assert_eq!(parse(&["/"]).path.as_deref(), Some(Path::new("/")));
        assert_eq!(parse(&["."]).path.as_deref(), Some(Path::new(CWD)));
        assert_eq!(
            parse(&["//host/share/a.md"]).path.as_deref(),
            Some(Path::new("//host/share/a.md")),
            "a double leading slash is kept for the path policy to reject"
        );
    }

    #[test]
    fn a_relative_cwd_is_made_absolute() {
        let launch = parse_args(args(&["x.md"]), "relative/notes");
        assert!(launch.cwd.is_absolute());
        assert!(launch.path.as_deref().is_some_and(|p| p.ends_with("relative/notes/x.md")));
    }

    #[test]
    fn launch_round_trips_through_the_socket_json() {
        let launch = parse(&["--new-window", "a.md", "--line=9"]);
        let json = serde_json::to_string(&launch).unwrap();
        let parsed: Launch = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, launch);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["newWindow"], serde_json::json!(true));
        assert_eq!(value["line"], serde_json::json!(9));
    }

    #[test]
    fn session_and_token_fields_join_the_socket_json() {
        let launch = parse(&["--restore-session", "a.md"]);
        let json = serde_json::to_string(&launch).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["restoreSession"], serde_json::json!(true));
        assert!(value["activationToken"].is_null());

        let mut launch = launch;
        launch.activation_token = Some("wayland-token".to_owned());
        let parsed: Launch = serde_json::from_str(&serde_json::to_string(&launch).unwrap()).unwrap();
        assert_eq!(parsed, launch);
        // An older peer's JSON leaves the extension fields at their default.
        let old: Launch = serde_json::from_str(r#"{"path":"/a.md","line":3,"newWindow":false,"cwd":"/"}"#).unwrap();
        assert!(!old.restore_session);
        assert_eq!(old.activation_token, None);
    }
}
