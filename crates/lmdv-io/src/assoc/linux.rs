//! Linux default-handler registration for Markdown files.
//!
//! XDG has no user-consent hash like Windows' `UserChoice`: a desktop entry that
//! declares the Markdown types plus the user's `mimeapps.list` defaults *are*
//! the association. This module therefore applies the change at once and reports
//! [`Confirmation::Applied`]; there is no settings page to confirm in.
//!
//! It follows `scripts/install-md-handler.sh` field for field, and reuses what a
//! development install or a package already provides:
//!
//! 1. The program is `$APPIMAGE` when that names a file, else the running
//!    executable.
//! 2. A usable `lmdv.desktop` in `$XDG_DATA_HOME/applications` or in any
//!    `$XDG_DATA_DIRS` `applications` dir is left byte for byte alone; it may
//!    point at the installer's wrapper, which picks the build to run. Only a
//!    user-level entry that exists and does *not* register Markdown stops the
//!    press ([`AssociationError::Unmanaged`]).
//! 3. Only when none exists is `$XDG_DATA_HOME/applications/lmdv.desktop`
//!    written, with the packaging entry's fields and `X-LMDV-Managed=true`.
//! 4. The icon in `$XDG_DATA_HOME/icons/hicolor/scalable/apps` is written when
//!    absent or already lmdv's, and left alone when someone else put it there.
//! 5. `$XDG_CONFIG_HOME/mimeapps.list` names `lmdv.desktop` first for all three
//!    Markdown types, under both `[Default Applications]` and
//!    `[Added Associations]`, keeping the previous handlers after it and every
//!    other line.
//! 6. `update-desktop-database` runs detached, best effort, so desktops that
//!    cache application lists notice the entry.
//!
//! [`apply`] takes every directory it touches, so its tests use temp dirs and
//! never read or write the real `$HOME`. Nothing here needs root, and nothing
//! calls `xdg-mime` or `gio`: the files are the interface.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{AssociationError, Confirmation};

/// The desktop entry's file name in any `applications` directory.
pub const DESKTOP_ID: &str = "lmdv.desktop";
/// The Markdown types the entry claims and the defaults name, in the
/// installer's order.
pub const MIME_TYPES: [&str; 3] = ["text/markdown", "text/x-markdown", "application/x-markdown"];
/// The entry's `MimeType=` value: [`MIME_TYPES`] as a desktop-file list.
pub const MIME_LIST: &str = "text/markdown;text/x-markdown;application/x-markdown;";
/// Marks the desktop entry lmdv owns, as the installer does.
pub const MANAGED_MARKER: &str = "X-LMDV-Managed=true";
/// Marks the icon lmdv owns.
const ICON_MARKER: &str = "<!-- lmdv-managed -->";
/// The data roots consulted when `$XDG_DATA_DIRS` is unset or empty.
const DEFAULT_DATA_DIRS: [&str; 2] = ["/usr/local/share", "/usr/share"];
/// The application icon the packaging ships, embedded so a press needs no
/// repository around it.
const ICON: &str = include_str!("../../../../assets/lmdv.svg");

/// Makes lmdv the Markdown handler for this session's environment.
///
/// The row is offered on Linux, so the environment's XDG directories are read
/// here; everything below [`apply`] takes them as arguments.
pub(super) fn make_default_for_markdown() -> Result<Confirmation, AssociationError> {
    let program = program(std::env::var_os("APPIMAGE"))?;
    let get = |name: &str| std::env::var_os(name);
    let locations = locations(&get)?;
    apply(&program, &locations.data_home, &locations.config_home, &locations.data_dirs)
}

/// Applies the association for `program` below the given XDG directories.
///
/// `data_dirs` are XDG data roots (`/usr/local/share`, `/usr/share`, …), searched
/// for an entry a package already installed. Returns
/// [`Confirmation::Applied`] once the defaults name lmdv; every failure leaves
/// the user's files as they were, apart from the writes that already succeeded.
pub fn apply(
    program: &Path,
    data_home: &Path,
    config_home: &Path,
    data_dirs: &[PathBuf],
) -> Result<Confirmation, AssociationError> {
    let applications = data_home.join("applications");
    let user_entry = applications.join(DESKTOP_ID);

    // Reuse an entry that already names lmdv; only create ours when none does.
    // The program path is validated before any write, so an unrepresentable
    // path changes nothing at all.
    if !existing_entry(&user_entry, data_dirs)? {
        let entry = desktop_entry(program)?;
        replace_file(&user_entry, &entry)?;
    }
    ensure_icon(data_home)?;
    set_defaults(config_home)?;
    refresh_desktop_database(&applications);
    Ok(Confirmation::Applied)
}

/// The complete `lmdv.desktop` this module writes for `program`.
///
/// The fields are the packaging entry's (`scripts/install-md-handler.sh`), with
/// `X-LMDV-Managed=true` so a later run can tell it owns the file. `TryExec` is
/// omitted: a quoted path cannot be represented there reliably, and the entry's
/// `Exec` is the authority anyway.
///
/// The `Exec` path is left unquoted unless it contains a space — `xdg-mime`'s
/// desktop-file reader does not strip quotes, so quoting a simple path would
/// make the entry look like a different program. A path holding `"`, `\`, `$`, a
/// backtick, `%` or a newline is refused instead of guessed at.
pub fn desktop_entry(program: &Path) -> Result<String, AssociationError> {
    let exec = exec_path(program)?;
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name=lmdv\n\
         GenericName=Markdown Viewer\n\
         Comment=View and edit Markdown documents\n\
         Exec={exec} %f\n\
         Icon=lmdv\n\
         Terminal=false\n\
         StartupNotify=false\n\
         StartupWMClass=lmdv\n\
         SingleMainWindow=true\n\
         DBusActivatable=false\n\
         NoDisplay=false\n\
         Categories=Utility;TextTools;\n\
         Keywords=markdown;md;view;preview;text;\n\
         MimeType={MIME_LIST}\n\
         {MANAGED_MARKER}\n"
    ))
}

/// Whether `text` is a desktop entry lmdv can leave in place: a
/// `[Desktop Entry]` group with an `Exec=` and a `MimeType=` that names at least
/// one of [`MIME_TYPES`].
///
/// A package's entry may carry more fields, a different program and a different
/// order; only those three things decide whether the desktop will hand lmdv a
/// Markdown file.
pub fn is_usable_entry(text: &str) -> bool {
    let mut in_entry = false;
    let mut seen_entry = false;
    let mut exec = false;
    let mut mime = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = section_header(line) {
            in_entry = name == "Desktop Entry";
            seen_entry = seen_entry || in_entry;
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match key.trim() {
            "Exec" => exec = exec || !value.trim().is_empty(),
            "MimeType" => {
                mime = mime || value.split(';').map(str::trim).any(|named| MIME_TYPES.contains(&named));
            }
            _ => {}
        }
    }
    seen_entry && exec && mime
}

/// `existing` with lmdv first in every Markdown default and added association,
/// keeping every other section, key and comment.
///
/// Both target sections carry a desktop-file list per type (`lmdv.desktop;…;`):
/// lmdv first, the handlers that were there after it, and no duplicate of lmdv.
/// A missing section is created at the end of the file, a missing type at the end
/// of its section; existing types are rewritten where they stand. The result is
/// byte-identical when the function is applied to its own output, and the file's
/// line endings are kept.
pub fn mimeapps_with_defaults(existing: &str) -> String {
    let eol = if existing.contains("\r\n") { "\r\n" } else { "\n" };
    let normalized = existing.replace("\r\n", "\n");

    let mut out: Vec<String> = Vec::with_capacity(normalized.lines().count() + 8);
    let mut section: Option<Target> = None;
    let mut pending = [false; MIME_TYPES.len()];
    let mut seen = [false; 2];
    for line in normalized.split('\n') {
        if let Some(name) = section_header(line) {
            flush_section(&mut out, section, &pending);
            section = Target::named(name);
            pending = [false; MIME_TYPES.len()];
            if let Some(target) = section {
                seen[target.index()] = true;
                pending = [true; MIME_TYPES.len()];
            }
            out.push(line.to_owned());
            continue;
        }
        if section.is_some()
            && let Some((key, value)) = line.split_once('=')
            && let Some(index) = MIME_TYPES.iter().position(|mime| *mime == key.trim())
            && pending[index]
        {
            out.push(format!("{}={}", key.trim(), lmdv_first(value)));
            pending[index] = false;
            continue;
        }
        out.push(line.to_owned());
    }
    flush_section(&mut out, section, &pending);

    let mut text = out.join("\n");
    for (target, header) in [(Target::Defaults, "[Default Applications]"), (Target::Added, "[Added Associations]")] {
        if seen[target.index()] {
            continue;
        }
        if !text.is_empty() {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push('\n');
        }
        text.push_str(header);
        text.push('\n');
        for mime in MIME_TYPES {
            text.push_str(mime);
            text.push('=');
            text.push_str(DESKTOP_ID);
            text.push_str(";\n");
        }
    }
    if eol == "\r\n" { text.replace('\n', "\r\n") } else { text }
}

/// The two `mimeapps.list` sections the defaults live in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    /// `[Default Applications]`: the handler a double-click opens.
    Defaults,
    /// `[Added Associations]`: the handlers offered in “Open with”.
    Added,
}

impl Target {
    /// The section a header names, or `None` for any other section.
    fn named(name: &str) -> Option<Self> {
        match name {
            "Default Applications" => Some(Self::Defaults),
            "Added Associations" => Some(Self::Added),
            _ => None,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Defaults => 0,
            Self::Added => 1,
        }
    }
}

/// Adds the types a target section did not name, at its end.
fn flush_section(out: &mut Vec<String>, section: Option<Target>, pending: &[bool]) {
    if section.is_none() {
        return;
    }
    for (index, mime) in MIME_TYPES.iter().enumerate() {
        if pending[index] {
            out.push(format!("{mime}={}", lmdv_first("")));
        }
    }
}

/// A desktop-file list with `lmdv.desktop` first and the remaining handlers,
/// in order, after it.
fn lmdv_first(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + DESKTOP_ID.len() + 1);
    out.push_str(DESKTOP_ID);
    out.push(';');
    for handler in value.split(';') {
        let handler = handler.trim();
        if handler.is_empty() || handler == DESKTOP_ID {
            continue;
        }
        out.push_str(handler);
        out.push(';');
    }
    out
}

/// The header a line names (`[Desktop Entry]` → `Desktop Entry`), if it is one.
fn section_header(line: &str) -> Option<&str> {
    line.trim().strip_prefix('[')?.strip_suffix(']')
}

/// The `Exec` program for `program`: quoted only when it holds a space, and
/// refused when desktop files cannot carry it faithfully.
fn exec_path(program: &Path) -> Result<String, AssociationError> {
    let unencodable = || AssociationError::Unencodable { path: program.to_owned() };
    let text = program.to_str().ok_or_else(unencodable)?;
    let refused =
        text.is_empty() || text.chars().any(|letter| matches!(letter, '"' | '\\' | '$' | '`' | '%' | '\n' | '\r'));
    if refused {
        return Err(unencodable());
    }
    if text.contains(' ') { Ok(format!("\"{text}\"")) } else { Ok(text.to_owned()) }
}

/// The running program: `$APPIMAGE` when that names a file (an AppImage mounts
/// itself read-only elsewhere, so its `current_exe` is not what to launch),
/// else the executable.
fn program(appimage: Option<OsString>) -> Result<PathBuf, AssociationError> {
    if let Some(path) = appimage.map(PathBuf::from)
        && path.is_absolute()
        && path.is_file()
    {
        return Ok(path);
    }
    std::env::current_exe().map_err(|source| AssociationError::Exe { source })
}

/// The XDG directories one apply reads and writes.
struct Locations {
    data_home: PathBuf,
    config_home: PathBuf,
    data_dirs: Vec<PathBuf>,
}

/// Resolves the XDG roots from `get` (the environment in production, a table in
/// tests). A relative or empty variable is ignored, as the XDG spec says.
fn locations(get: &dyn Fn(&str) -> Option<OsString>) -> Result<Locations, AssociationError> {
    let home = absolute(get("HOME"));
    let data_home = absolute(get("XDG_DATA_HOME"))
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")))
        .ok_or(AssociationError::NoHome)?;
    let config_home = absolute(get("XDG_CONFIG_HOME"))
        .or_else(|| home.as_ref().map(|home| home.join(".config")))
        .ok_or(AssociationError::NoHome)?;
    Ok(Locations { data_home, config_home, data_dirs: data_dirs(get) })
}

/// An absolute, non-empty path, or `None`.
fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    let value = value.filter(|value| !value.is_empty())?;
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

/// `$XDG_DATA_DIRS` as roots, or the spec's defaults when it is unset or empty.
fn data_dirs(get: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = get("XDG_DATA_DIRS")
        .map(|value| std::env::split_paths(&value).filter(|path| path.is_absolute()).collect())
        .unwrap_or_default();
    if dirs.is_empty() {
        dirs = DEFAULT_DATA_DIRS.iter().map(PathBuf::from).collect();
    }
    dirs
}

/// Whether an entry is available to reuse: lmdv's own (usable) entry, or a
/// usable one a package installed into one of `data_dirs`.
///
/// A user-level entry that exists but is not usable is never overwritten: it
/// belongs to the user (or another installer), so [`AssociationError::Unmanaged`]
/// reports it. An unusable system entry is skipped, because the user's own entry
/// can still be added and will take precedence.
fn existing_entry(user_entry: &Path, data_dirs: &[PathBuf]) -> Result<bool, AssociationError> {
    match std::fs::symlink_metadata(user_entry) {
        Ok(_) => match std::fs::read_to_string(user_entry) {
            Ok(text) if is_usable_entry(&text) => return Ok(true),
            _ => return Err(AssociationError::Unmanaged { path: user_entry.to_owned() }),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(source) => return Err(AssociationError::File { path: user_entry.to_owned(), source }),
    }
    for root in data_dirs {
        let path = root.join("applications").join(DESKTOP_ID);
        if let Ok(text) = std::fs::read_to_string(&path)
            && is_usable_entry(&text)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Writes the managed icon when it is absent or lmdv's, and leaves a foreign one
/// alone.
fn ensure_icon(data_home: &Path) -> Result<(), AssociationError> {
    let path = data_home.join("icons/hicolor/scalable/apps/lmdv.svg");
    let managed = format!("{ICON_MARKER}\n{ICON}");
    match std::fs::read_to_string(&path) {
        Ok(text) if text.contains(ICON_MARKER) => {
            if text != managed {
                replace_file(&path, &managed)?;
            }
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => replace_file(&path, &managed),
        Err(source) => Err(AssociationError::File { path, source }),
    }
}

/// Names lmdv the default for the three Markdown types in
/// `$XDG_CONFIG_HOME/mimeapps.list`, creating the file when it is missing.
fn set_defaults(config_home: &Path) -> Result<(), AssociationError> {
    let path = config_home.join("mimeapps.list");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(source) => return Err(AssociationError::File { path, source }),
    };
    let merged = mimeapps_with_defaults(&existing);
    if merged == existing {
        return Ok(());
    }
    std::fs::create_dir_all(config_home)
        .map_err(|source| AssociationError::File { path: config_home.to_owned(), source })?;
    std::fs::write(&path, merged).map_err(|source| AssociationError::File { path, source })
}

/// Replaces a file lmdv owns, atomically and with its folder created; a symlink
/// is refused, so the write can never land outside the XDG root.
fn replace_file(path: &Path, contents: &str) -> Result<(), AssociationError> {
    crate::atomic::write_atomic(path, contents.as_bytes())
        .map_err(|source| AssociationError::File { path: path.to_owned(), source })
}

/// Rebuilds the desktop database for `applications`, detaching and ignoring a
/// missing tool: the entry works without the cache, only the Open with list may
/// lag until the desktop refreshes it.
fn refresh_desktop_database(applications: &Path) {
    let _ = Command::new("update-desktop-database")
        .arg(applications)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    /// A package's entry: usable, owned by no one, and not the one lmdv writes.
    const PACKAGED_ENTRY: &str = "[Desktop Entry]\n\
        Type=Application\n\
        Name=lmdv\n\
        Exec=/usr/bin/lmdv %f\n\
        TryExec=/usr/bin/lmdv\n\
        MimeType=text/markdown;text/x-markdown;\n";

    fn temp() -> TempDir {
        TempDir::new().unwrap()
    }

    fn entry_text(data_home: &Path) -> String {
        std::fs::read_to_string(data_home.join("applications/lmdv.desktop")).unwrap()
    }

    fn mimeapps_text(config_home: &Path) -> String {
        std::fs::read_to_string(config_home.join("mimeapps.list")).unwrap()
    }

    /// The default and added lines each type must have in a fresh file.
    fn lmdv_default(mime: &str) -> String {
        format!("{mime}={DESKTOP_ID};\n")
    }

    #[test]
    fn a_missing_entry_is_written_with_the_packagings_fields() {
        let root = temp();
        let data_home = root.path().join("data");
        let config_home = root.path().join("config");

        let confirmation = apply(Path::new("/opt/lmdv/bin/lmdv"), &data_home, &config_home, &[]).unwrap();
        assert_eq!(confirmation, Confirmation::Applied);

        let entry = entry_text(&data_home);
        assert!(entry.starts_with("[Desktop Entry]\n"), "{entry}");
        assert!(entry.contains("\nType=Application\n"), "{entry}");
        assert!(entry.contains("\nName=lmdv\n"), "{entry}");
        assert!(entry.contains("\nExec=/opt/lmdv/bin/lmdv %f\n"), "an unrelated path stays unquoted: {entry}");
        assert!(entry.contains("\nIcon=lmdv\n"), "{entry}");
        assert!(entry.contains("\nTerminal=false\n"), "{entry}");
        assert!(entry.contains("\nStartupWMClass=lmdv\n"), "{entry}");
        assert!(entry.contains("\nCategories=Utility;TextTools;\n"), "{entry}");
        assert!(entry.contains(&format!("\nMimeType={MIME_LIST}\n")), "{entry}");
        assert!(entry.contains(&format!("\n{MANAGED_MARKER}\n")), "{entry}");
        assert!(!entry.contains("TryExec"), "a quoted path cannot be represented in TryExec: {entry}");

        let mimeapps = mimeapps_text(&config_home);
        assert!(mimeapps.contains("[Default Applications]\n"), "{mimeapps}");
        assert!(mimeapps.contains("[Added Associations]\n"), "{mimeapps}");
        for mime in MIME_TYPES {
            // Once in each section.
            assert_eq!(mimeapps.matches(&lmdv_default(mime)).count(), 2, "{mime}:\n{mimeapps}");
        }
        assert!(data_home.join("icons/hicolor/scalable/apps/lmdv.svg").is_file());
    }

    #[test]
    fn the_entrys_mime_list_is_the_types_in_order() {
        let joined: String = MIME_TYPES.iter().map(|mime| format!("{mime};")).collect();
        assert_eq!(joined, MIME_LIST, "the entry and the defaults must name the same types");
        assert_eq!(MIME_TYPES[0], "text/markdown", "text/markdown is the preferred type");
    }

    #[test]
    fn a_program_path_is_quoted_only_when_it_holds_a_space() {
        let simple = desktop_entry(Path::new("/home/u/.local/bin/lmdv")).unwrap();
        assert!(simple.contains("\nExec=/home/u/.local/bin/lmdv %f\n"), "{simple}");

        let spaced = desktop_entry(Path::new("/home/u/My Apps/lmdv")).unwrap();
        assert!(spaced.contains("\nExec=\"/home/u/My Apps/lmdv\" %f\n"), "{spaced}");
    }

    #[test]
    fn an_unrepresentable_program_path_is_refused_before_anything_is_written() {
        for program in
            ["/opt/my%app/lmdv", "/opt/quote\"/lmdv", "/opt/back\\slash/lmdv", "/opt/$var/lmdv", "/opt/`tick/lmdv"]
        {
            assert!(
                matches!(desktop_entry(Path::new(program)), Err(AssociationError::Unencodable { .. })),
                "{program} must be refused"
            );
        }
        assert!(matches!(desktop_entry(Path::new("/opt/line\nbreak/lmdv")), Err(AssociationError::Unencodable { .. })));

        let root = temp();
        let data_home = root.path().join("data");
        let config_home = root.path().join("config");
        let error = apply(Path::new("/opt/my%app/lmdv"), &data_home, &config_home, &[]).expect_err("refused");
        assert!(matches!(error, AssociationError::Unencodable { .. }));
        assert!(!data_home.exists(), "nothing was written for a refused program path");
        assert!(!config_home.exists());
    }

    #[test]
    fn a_usable_user_entry_is_reused_byte_for_byte() {
        let root = temp();
        let data_home = root.path().join("data");
        let config_home = root.path().join("config");
        let entry = data_home.join("applications/lmdv.desktop");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        // The installer's entry points at its wrapper, not at the running exe.
        let wrapper_entry = "[Desktop Entry]\n\
             Type=Application\n\
             Name=lmdv\n\
             Exec=/home/u/.local/bin/lmdv %f\n\
             TryExec=/home/u/.local/bin/lmdv\n\
             Icon=lmdv\n\
             MimeType=text/markdown;text/x-markdown;application/x-markdown;\n\
             X-LMDV-Managed=true\n";
        std::fs::write(&entry, wrapper_entry).unwrap();

        assert_eq!(
            apply(Path::new("/target/debug/lmdv"), &data_home, &config_home, &[]).unwrap(),
            Confirmation::Applied
        );
        assert_eq!(entry_text(&data_home), wrapper_entry, "the wrapper entry decides which build runs");
        assert!(mimeapps_text(&config_home).contains(&lmdv_default("text/markdown")));
    }

    #[test]
    fn a_packaged_entry_in_the_data_dirs_is_reused_instead_of_written() {
        let root = temp();
        let system = root.path().join("system");
        std::fs::create_dir_all(system.join("applications")).unwrap();
        std::fs::write(system.join("applications/lmdv.desktop"), PACKAGED_ENTRY).unwrap();
        let data_home = root.path().join("data");
        let config_home = root.path().join("config");

        apply(Path::new("/target/debug/lmdv"), &data_home, &config_home, std::slice::from_ref(&system)).unwrap();
        assert!(!data_home.join("applications/lmdv.desktop").exists(), "the package's entry is the handler");
        assert_eq!(std::fs::read_to_string(system.join("applications/lmdv.desktop")).unwrap(), PACKAGED_ENTRY);
        assert!(mimeapps_text(&config_home).contains(&lmdv_default("application/x-markdown")));
    }

    #[test]
    fn a_user_entry_that_is_not_usable_is_left_alone() {
        let root = temp();
        let data_home = root.path().join("data");
        let config_home = root.path().join("config");
        let entry = data_home.join("applications/lmdv.desktop");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        let foreign = "[Desktop Entry]\nType=Application\nName=lmdv\nExec=/usr/bin/other %f\n";
        std::fs::write(&entry, foreign).unwrap();

        let error = apply(Path::new("/target/debug/lmdv"), &data_home, &config_home, &[]).expect_err("unmanaged");
        match error {
            AssociationError::Unmanaged { path } => assert_eq!(path, entry),
            other => panic!("expected Unmanaged, got {other}"),
        }
        assert_eq!(entry_text(&data_home), foreign, "the user's file is not clobbered");
        assert!(!config_home.join("mimeapps.list").exists(), "nothing else is written when the press is refused");
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_user_entry_symlink_is_not_written_through() {
        let root = temp();
        let data_home = root.path().join("data");
        let entry = data_home.join("applications/lmdv.desktop");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(root.path().join("nowhere"), &entry).unwrap();

        let error = apply(Path::new("/target/debug/lmdv"), &data_home, &root.path().join("config"), &[])
            .expect_err("unmanaged");
        assert!(matches!(error, AssociationError::Unmanaged { .. }), "{error}");
        assert!(std::fs::symlink_metadata(&entry).unwrap().file_type().is_symlink());
        assert!(!root.path().join("nowhere").exists(), "nothing was written through the link");
    }

    #[test]
    fn the_icon_is_written_when_absent_refreshed_when_managed_and_kept_when_foreign() {
        let root = temp();
        let data_home = root.path().join("data");
        let config_home = root.path().join("config");
        let icon = data_home.join("icons/hicolor/scalable/apps/lmdv.svg");

        apply(Path::new("/opt/lmdv"), &data_home, &config_home, &[]).unwrap();
        let written = std::fs::read_to_string(&icon).unwrap();
        assert!(written.starts_with(ICON_MARKER), "the icon carries its marker first");
        assert!(written.contains(ICON.trim_end()), "the embedded packaging icon is written whole");

        std::fs::write(&icon, format!("{ICON_MARKER}\nstale")).unwrap();
        apply(Path::new("/opt/lmdv"), &data_home, &config_home, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(&icon).unwrap(), written, "a managed icon is refreshed");

        let foreign = "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n";
        std::fs::write(&icon, foreign).unwrap();
        apply(Path::new("/opt/lmdv"), &data_home, &config_home, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(&icon).unwrap(), foreign, "someone else's icon is not replaced");
    }

    #[test]
    fn applying_twice_changes_nothing() {
        let root = temp();
        let data_home = root.path().join("data");
        let config_home = root.path().join("config");
        let program = Path::new("/opt/lmdv/bin/lmdv");

        apply(program, &data_home, &config_home, &[]).unwrap();
        let entry = entry_text(&data_home);
        let mimeapps = mimeapps_text(&config_home);
        apply(program, &data_home, &config_home, &[]).unwrap();
        assert_eq!(entry_text(&data_home), entry);
        assert_eq!(mimeapps_text(&config_home), mimeapps);
    }

    #[test]
    fn mimeapps_keeps_every_other_line_and_puts_lmdv_first() {
        let existing = "# my handlers\n\
             [Default Applications]\n\
             text/markdown=gedit.desktop;\n\
             text/html=firefox.desktop;\n\
             \n\
             [Removed Associations]\n\
             image/png=foo.desktop;\n";
        let merged = mimeapps_with_defaults(existing);

        assert!(merged.starts_with("# my handlers\n[Default Applications]\n"), "{merged}");
        assert!(merged.contains("\ntext/markdown=lmdv.desktop;gedit.desktop;\n"), "{merged}");
        assert!(merged.contains("\ntext/html=firefox.desktop;\n"), "{merged}");
        assert!(merged.contains("\ntext/x-markdown=lmdv.desktop;\n"), "{merged}");
        assert!(merged.contains("\napplication/x-markdown=lmdv.desktop;\n"), "{merged}");
        assert!(merged.contains("\n[Removed Associations]\nimage/png=foo.desktop;\n"), "{merged}");
        assert_eq!(mimeapps_with_defaults(&merged), merged, "the merge is idempotent");
    }

    #[test]
    fn mimeapps_moves_an_existing_lmdv_to_the_front_without_duplicating_it() {
        let existing = "[Default Applications]\n\
             text/markdown=gedit.desktop;lmdv.desktop;code.desktop;\n";
        let merged = mimeapps_with_defaults(existing);
        assert!(merged.contains("\ntext/markdown=lmdv.desktop;gedit.desktop;code.desktop;\n"), "{merged}");
        for line in merged.lines().filter(|line| line.contains("=")) {
            assert!(line.matches("lmdv.desktop").count() <= 1, "no list names lmdv twice: {line}");
        }
    }

    #[test]
    fn mimeapps_creates_both_sections_with_the_three_types() {
        let merged = mimeapps_with_defaults("");
        assert!(merged.starts_with("[Default Applications]\n"), "{merged}");
        assert!(merged.contains("\n\n[Added Associations]\n"), "{merged}");
        for mime in MIME_TYPES {
            assert_eq!(merged.matches(&format!("{mime}={DESKTOP_ID};")).count(), 2, "{mime}:\n{merged}");
        }
        assert_eq!(merged.lines().count(), 9, "two headers, six type lines and the blank between them:\n{merged}");
        assert_eq!(mimeapps_with_defaults(&merged), merged);
    }

    #[test]
    fn mimeapps_fills_an_empty_value_and_keeps_its_line_endings() {
        let existing = "[Default Applications]\r\ntext/markdown=\r\ntext/html=firefox.desktop;\r\n";
        let merged = mimeapps_with_defaults(existing);
        assert!(merged.contains("\r\ntext/markdown=lmdv.desktop;\r\n"), "{merged:?}");
        assert!(!merged.replace("\r\n", "").contains('\n'), "no line ending is converted: {merged:?}");
        assert_eq!(mimeapps_with_defaults(&merged), merged);
    }

    #[test]
    fn usability_needs_the_entry_group_an_exec_and_a_markdown_type() {
        assert!(is_usable_entry(PACKAGED_ENTRY));
        assert!(is_usable_entry(&desktop_entry(Path::new("/opt/lmdv")).unwrap()));
        assert!(!is_usable_entry(""), "an empty file is not an entry");
        assert!(!is_usable_entry("[Desktop Entry]\nExec=/usr/bin/lmdv %f\n"), "no Markdown type");
        assert!(!is_usable_entry("[Desktop Entry]\nMimeType=text/markdown;\n"), "no Exec");
        assert!(
            !is_usable_entry("[Desktop Entry]\nExec=/usr/bin/lmdv %f\nMimeType=text/plain;\n"),
            "a type that is not Markdown"
        );
        assert!(
            !is_usable_entry("[Desktop Action new]\nExec=/usr/bin/lmdv %f\nMimeType=text/markdown;\n"),
            "the fields must be in [Desktop Entry]"
        );
        assert!(
            is_usable_entry("[Desktop Entry]\nExec=/usr/bin/lmdv %f\nMimeType=text/plain;application/x-markdown;\n"),
            "one of the three is enough"
        );
    }

    #[test]
    fn locations_follow_the_xdg_defaults() {
        let env = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> =
                pairs.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect();
            move |name: &str| pairs.iter().find(|(key, _)| key == name).map(|(_, value)| OsString::from(value))
        };

        let defaults = locations(&env(&[("HOME", "/home/u")])).unwrap();
        assert_eq!(defaults.data_home, Path::new("/home/u/.local/share"));
        assert_eq!(defaults.config_home, Path::new("/home/u/.config"));
        assert_eq!(defaults.data_dirs, [PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")]);

        let dirs = std::env::join_paths(["/a", "/b"]).unwrap().into_string().unwrap();
        let explicit = locations(&env(&[
            ("HOME", "/home/u"),
            ("XDG_DATA_HOME", "/d"),
            ("XDG_CONFIG_HOME", "/c"),
            ("XDG_DATA_DIRS", &dirs),
        ]))
        .unwrap();
        assert_eq!(explicit.data_home, Path::new("/d"));
        assert_eq!(explicit.config_home, Path::new("/c"));
        assert_eq!(explicit.data_dirs, [PathBuf::from("/a"), PathBuf::from("/b")]);

        let relative = locations(&env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "relative/data")])).unwrap();
        assert_eq!(relative.data_home, Path::new("/home/u/.local/share"), "a relative root is ignored");

        assert!(matches!(locations(&env(&[])), Err(AssociationError::NoHome)), "no HOME and no XDG roots");
    }

    #[test]
    fn an_appimage_is_the_program_when_it_is_a_file() {
        let root = temp();
        let appimage = root.path().join("lmdv-1.0-x86_64.AppImage");
        std::fs::write(&appimage, "").unwrap();
        assert_eq!(program(Some(appimage.clone().into_os_string())).unwrap(), appimage);

        let missing = root.path().join("gone.AppImage");
        assert_eq!(
            program(Some(missing.into_os_string())).unwrap(),
            std::env::current_exe().unwrap(),
            "a stale $APPIMAGE falls back to the running executable"
        );
    }
}
