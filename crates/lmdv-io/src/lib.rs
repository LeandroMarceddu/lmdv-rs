//! Filesystem, persistence, watcher and single-instance services for lmdv.
//!
//! Everything in this crate is blocking I/O and belongs on a worker thread or
//! inside the store writer; no function here touches Rune or a view. The pure
//! domain values ([`lmdv_core::SourceBuffer`], [`lmdv_core::FileMeta`],
//! [`lmdv_core::Draft`]) stay in `lmdv-core`.
//!
//! * [`launch`] — the original `lmdv [--new-window] [file] [--line N]` parser.
//! * [`path_policy`] — canonicalization and embedded-asset confinement.
//! * [`assets`] — an image pasted or dropped beside the document (E-36).
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

pub mod assets;
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

/// Removes one file. An already absent file is success.
///
/// The policy — which files may be removed — belongs to the caller; this is the
/// worker-side counterpart of [`open_external`] for the app's own temporaries.
pub fn remove_file(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

// ---- revealing a file in the desktop's file manager (X-05) ---------------

/// How long a selecting reveal command may take before the next one runs.
///
/// A missing `FileManager1` service answers at once; the timeout only covers a
/// file manager that is being cold-started (or has hung), so it is generous
/// enough to avoid starting a second one.
pub const REVEAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// One process that reveals a path in the desktop's file manager.
///
/// The plan is data so the selection behavior is testable without a desktop:
/// [`reveal_commands`] builds it, [`reveal_in_file_manager`] runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealCommand {
    /// The program to run.
    pub program: String,
    /// Its arguments, already split: no shell ever parses them.
    pub args: Vec<std::ffi::OsString>,
    /// Whether the command selects `path` itself rather than only opening its
    /// folder. Only a selecting command is waited for; its refusal (no file
    /// manager on the bus, a missing program) falls through to the next one.
    pub selects: bool,
}

/// The path a reveal acts on: `path` itself when it exists, else its folder.
///
/// A saved export always exists; a file deleted between the notice and the
/// click still shows the folder it was in (X-05).
pub fn reveal_path(path: &std::path::Path) -> std::path::PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| path.to_path_buf(), std::path::Path::to_path_buf)
}

/// The commands that reveal `path`, best first.
///
/// Linux asks the desktop's `org.freedesktop.FileManager1` service over D-Bus,
/// which opens the folder and selects the file; when no file manager answers,
/// the plain `xdg-open` on the folder is the fallback (it shows the folder but
/// cannot select inside it). Windows selects with `explorer /select,`; macOS
/// with `open -R`.
pub fn reveal_commands(path: &std::path::Path) -> Vec<RevealCommand> {
    let target = reveal_path(path);
    let folder = if target.is_dir() {
        target.clone()
    } else {
        target.parent().map_or_else(|| target.clone(), std::path::Path::to_path_buf)
    };
    let folder_arg = std::ffi::OsString::from(folder);

    #[cfg(target_os = "linux")]
    {
        let mut commands = Vec::new();
        if let Some(url) = file_url(&target) {
            commands.push(RevealCommand {
                program: "dbus-send".to_owned(),
                args: vec![
                    "--session".into(),
                    "--print-reply".into(),
                    "--dest=org.freedesktop.FileManager1".into(),
                    "/org/freedesktop/FileManager1".into(),
                    "org.freedesktop.FileManager1.ShowItems".into(),
                    format!("array:string:{url}").into(),
                    // The startup-id argument the interface requires. An empty
                    // argument is not valid for `dbus-send`; the explicit
                    // `string:` type is an empty string.
                    "string:".into(),
                ],
                selects: true,
            });
        }
        commands.push(RevealCommand { program: "xdg-open".to_owned(), args: vec![folder_arg], selects: false });
        commands
    }
    #[cfg(target_os = "macos")]
    {
        let _ = folder_arg;
        vec![RevealCommand { program: "open".to_owned(), args: vec!["-R".into(), target.into()], selects: true }]
    }
    #[cfg(windows)]
    {
        let _ = folder_arg;
        vec![RevealCommand {
            program: "explorer".to_owned(),
            args: vec![format!("/select,{}", target.display()).into()],
            selects: true,
        }]
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    {
        let _ = target;
        vec![RevealCommand { program: "xdg-open".to_owned(), args: vec![folder_arg], selects: false }]
    }
}

/// Opens the file manager showing `path`'s folder and, where the platform
/// supports it, selecting `path`.
///
/// Only user-initiated actions should call this, and only from a worker: it
/// spawns a process. A missing `path` reveals its folder.
pub fn reveal_in_file_manager(path: &std::path::Path) -> std::io::Result<()> {
    let commands = reveal_commands(path);
    let mut failure = None;
    for command in &commands {
        match run_reveal(command) {
            Ok(()) => return Ok(()),
            Err(error) => failure = Some(error),
        }
    }
    Err(failure
        .unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no file-manager command is available")))
}

fn run_reveal(command: &RevealCommand) -> std::io::Result<()> {
    let mut child = std::process::Command::new(&command.program)
        .args(&command.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    if !command.selects {
        return Ok(());
    }
    let deadline = std::time::Instant::now() + REVEAL_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!("{} exited with {status}", command.program)))
            };
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{} did not answer", command.program),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// A `file://` URL for an absolute path, percent-encoded per RFC 8089.
#[cfg(target_os = "linux")]
fn file_url(path: &std::path::Path) -> Option<String> {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt as _;

    let absolute = if path.is_absolute() { path.to_path_buf() } else { std::path::absolute(path).ok()? };
    let mut url = String::from("file://");
    for &byte in absolute.as_os_str().as_bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => url.push(byte as char),
            _ => {
                let _ = write!(url, "%{byte:02X}");
            }
        }
    }
    Some(url)
}

// ---- the process's local calendar date (X-14) ----------------------------

/// The local calendar date `(year, month, day)` for `unix_secs` in the
/// process's time zone.
///
/// The export header names the user's day, not UTC's: a print at 23:30 must
/// not be dated tomorrow (X-14, OI-104). `None` when the platform has no
/// time-zone data (the caller keeps the UTC date).
#[cfg(unix)]
pub fn local_civil_date(unix_secs: u64) -> Option<(i64, u32, u32)> {
    let time = unix_secs.min(i64::MAX as u64) as libc::time_t;
    // SAFETY: `time` and `tm` are valid for the call; `localtime_r` only writes
    // through `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let filled = unsafe { libc::localtime_r(&time, &mut tm) };
    if filled.is_null() {
        return None;
    }
    let month = u32::try_from(tm.tm_mon + 1).ok()?;
    let day = u32::try_from(tm.tm_mday).ok()?;
    Some((i64::from(tm.tm_year) + 1900, month, day))
}

/// The local calendar date `(year, month, day)` for `unix_secs` in the
/// process's time zone (see the Unix implementation).
///
/// Windows converts the UTC instant through the current zone with
/// `SystemTimeToTzSpecificLocalTime`, so daylight saving follows the zone's
/// own rules rather than a fixed offset.
#[cfg(windows)]
pub fn local_civil_date(unix_secs: u64) -> Option<(i64, u32, u32)> {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SystemTimeToTzSpecificLocalTime(
            time_zone: *const std::ffi::c_void,
            universal: *const SystemTime,
            local: *mut SystemTime,
        ) -> i32;
    }

    let (year, month, day) = civil_from_days((unix_secs / 86_400) as i64);
    if !(1601..=30_827).contains(&year) {
        return None;
    }
    let rest = unix_secs % 86_400;
    let utc = SystemTime {
        year: year as u16,
        month: month as u16,
        day: day as u16,
        hour: (rest / 3_600) as u16,
        minute: ((rest / 60) % 60) as u16,
        second: (rest % 60) as u16,
        ..SystemTime::default()
    };
    let mut local = SystemTime::default();
    // SAFETY: both SYSTEMTIME values are initialized and live for the call;
    // a null time-zone pointer means the current zone, as documented.
    let converted = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) };
    (converted != 0).then(|| (i64::from(local.year), u32::from(local.month), u32::from(local.day)))
}

/// A proleptic Gregorian date from days since 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
#[cfg(windows)]
fn civil_from_days(days: i64) -> (i64, usize, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as usize;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// No time-zone data on this target: the caller keeps the UTC date.
#[cfg(not(any(unix, windows)))]
pub fn local_civil_date(_unix_secs: u64) -> Option<(i64, u32, u32)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn a_file_url_percent_encodes_everything_but_the_unreserved_set() {
        assert_eq!(file_url(std::path::Path::new("/tmp/notes.pdf")).as_deref(), Some("file:///tmp/notes.pdf"));
        assert_eq!(
            file_url(std::path::Path::new("/tmp/a file % #1.pdf")).as_deref(),
            Some("file:///tmp/a%20file%20%25%20%231.pdf")
        );
        assert_eq!(file_url(std::path::Path::new("/tmp/caf\u{e9}.pdf")).as_deref(), Some("file:///tmp/caf%C3%A9.pdf"));
    }

    #[test]
    fn the_reveal_plan_selects_the_file_and_falls_back_to_the_folder() {
        let temp = tempfile::TempDir::new().unwrap();
        let file = temp.path().join("Report final.pdf");
        std::fs::write(&file, b"%PDF-").unwrap();

        let commands = reveal_commands(&file);
        #[cfg(target_os = "linux")]
        {
            assert_eq!(commands.len(), 2, "the D-Bus selection, then xdg-open");
            assert_eq!(commands[0].program, "dbus-send");
            assert!(commands[0].selects, "the FileManager1 call selects the file");
            let url = commands[0].args[5].to_string_lossy().into_owned();
            assert!(url.starts_with("array:string:file:///"), "an absolute file URL: {url}");
            assert!(url.ends_with("/Report%20final.pdf"), "the leaf is percent-encoded: {url}");
            assert_eq!(commands[0].args[6], std::ffi::OsString::from("string:"), "the startup id is empty");
            assert_eq!(commands[1].program, "xdg-open");
            assert!(!commands[1].selects, "xdg-open cannot select inside the folder");
            assert_eq!(commands[1].args, vec![std::ffi::OsString::from(temp.path())], "the fallback opens the folder");
        }
        #[cfg(windows)]
        {
            assert_eq!(commands[0].program, "explorer");
            assert_eq!(commands[0].args[0].to_string_lossy(), format!("/select,{}", file.display()));
        }

        // A file deleted since the notice still shows its folder.
        std::fs::remove_file(&file).unwrap();
        assert_eq!(reveal_path(&file), temp.path());
        #[cfg(target_os = "linux")]
        {
            let commands = reveal_commands(&file);
            let url = commands[0].args[5].to_string_lossy();
            assert!(
                url.ends_with(temp.path().file_name().unwrap().to_string_lossy().as_ref()),
                "the folder is selected: {url}"
            );
        }
    }

    #[test]
    fn a_folder_reveals_itself() {
        let temp = tempfile::TempDir::new().unwrap();
        assert_eq!(reveal_path(temp.path()), temp.path());
        #[cfg(target_os = "linux")]
        {
            let commands = reveal_commands(temp.path());
            assert_eq!(
                commands[1].args,
                vec![std::ffi::OsString::from(temp.path())],
                "xdg-open gets the folder itself"
            );
        }
    }

    #[test]
    fn removing_an_absent_file_is_success() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("gone.pdf");
        remove_file(&path).unwrap();
        std::fs::write(&path, b"%PDF-").unwrap();
        remove_file(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn the_local_date_is_a_real_calendar_day_next_to_utc() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let Some((year, month, day)) = local_civil_date(now) else {
            // No time-zone data: the caller keeps the UTC date.
            return;
        };
        assert!((2024..=2100).contains(&year), "{year}");
        assert!((1..=12).contains(&month), "{month}");
        assert!((1..=31).contains(&day), "{day}");
        // A user's day is at most one day away from the UTC day.
        let (utc_year, utc_month, utc_day) = civil_days((now / 86_400) as i64);
        let delta = days_from_civil(year, month, day) - days_from_civil(utc_year, utc_month, utc_day);
        assert!((-1..=1).contains(&delta), "the local day is within one day of UTC, got {delta}");
    }

    /// A test copy of the date arithmetic: the UTC day for `days` since the
    /// epoch.
    fn civil_days(days: i64) -> (i64, u32, u32) {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (if month <= 2 { year + 1 } else { year }, month, day)
    }

    /// A test copy of the inverse of `civil_days`.
    fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
        let era = if year >= 0 { year } else { year - 399 } / 400;
        let yoe = year - era * 400;
        let month = i64::from(month);
        let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }
}
