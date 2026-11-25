//! Windows default-handler registration for Markdown files, and the honest
//! hand-off to the user's own choice.
//!
//! Windows 10 and 11 protect `UserChoice` (a hash plus the user's consent in
//! Settings), so no application can silently become the default handler. This
//! module therefore does what the NSIS installer does — register the per-user
//! `lmdv.md` ProgID and its Open with / Default apps entries under
//! `HKEY_CURRENT_USER` — and then opens the surface where the user confirms:
//! the Default Apps page on Windows 11 (filtered to lmdv when the build
//! supports it) or, when that page cannot open, the “Open with” dialog for the
//! document in front. It never reads, deletes or forges `UserChoice`.
//!
//! [`association_plan`] and [`settings_uri`] are pure and compile on every
//! platform, so their tests run on Linux. Only the items behind
//! `#[cfg(windows)]` write the registry or start a process; the parent module
//! dispatches to [`make_default_for_markdown`] on Windows only.
//!
//! [`make_default_for_markdown`]: super::make_default_for_markdown

use std::path::Path;

#[cfg(windows)]
use super::{AssociationError, Confirmation};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use std::ptr;
#[cfg(windows)]
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
#[cfg(windows)]
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SZ, RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, ShellExecuteW};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// The per-user ProgID the installer and this module register.
pub const PROG_ID: &str = "lmdv.md";
/// The value name lmdv uses in `Software\RegisteredApplications`.
pub const REGISTERED_NAME: &str = "lmdv";
/// The Windows 10/11 Default Apps page.
pub const SETTINGS_PAGE: &str = "ms-settings:defaultapps";
/// The Windows 11 page, filtered to lmdv's registered application name.
pub const SETTINGS_PAGE_FILTERED: &str = "ms-settings:defaultapps?registeredAppUser=lmdv";
/// The first Windows 11 build number (21H2).
pub const WINDOWS_11_BUILD: u32 = 22_000;

const CLASSES: &str = r"Software\Classes";
const APP_KEY: &str = r"Software\Classes\Applications\lmdv.exe";
const CAPABILITIES_KEY: &str = r"Software\lmdv\Capabilities";
const REGISTERED_APPLICATIONS: &str = r"Software\RegisteredApplications";

/// One string value the association writes below `HKEY_CURRENT_USER`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryValue {
    /// The registry key, backslash-separated and relative to `HKEY_CURRENT_USER`.
    pub key: String,
    /// The value name; empty means the key's default value.
    pub name: String,
    /// The `REG_SZ` data. Empty is a real value, not a deletion.
    pub value: String,
}

impl RegistryValue {
    fn text(key: impl Into<String>, name: &str, value: impl Into<String>) -> Self {
        Self { key: key.into(), name: name.to_owned(), value: value.into() }
    }
}

/// The complete, ordered list of values the NSIS installer's association
/// section writes, for the executable at `exe`.
///
/// `previous_prog_id` is recorded only when the `.md` key named another
/// application before this run, so the installer's uninstaller can put it back;
/// it is never derived from `UserChoice`. Writing the plan repeatedly is
/// idempotent: every entry is a `(key, name)` the shell overwrites in place.
pub fn association_plan(exe: &Path, previous_prog_id: Option<&str>) -> Vec<RegistryValue> {
    let exe = exe.to_string_lossy();
    let command = format!("\"{exe}\" \"%1\"");
    let icon = format!("{exe},0");
    let prog_id = prog_id_key();
    let extension = extension_key();
    let mut values = Vec::with_capacity(17);
    values.push(RegistryValue::text(prog_id.clone(), "", "Markdown document"));
    values.push(RegistryValue::text(prog_id.clone(), "FriendlyTypeName", "Markdown document"));
    values.push(RegistryValue::text(format!(r"{prog_id}\DefaultIcon"), "", icon.clone()));
    values.push(RegistryValue::text(format!(r"{prog_id}\shell\open\command"), "", command.clone()));
    values.push(RegistryValue::text(format!(r"{prog_id}\shell\edit\command"), "", command.clone()));
    // The user's UserChoice still wins over this default; the settings page is
    // where the user confirms. Writing the key matches the installer.
    values.push(RegistryValue::text(extension.clone(), "", PROG_ID));
    values.push(RegistryValue::text(extension.clone(), "Content Type", "text/markdown"));
    values.push(RegistryValue::text(format!(r"{extension}\OpenWithProgids"), PROG_ID, ""));
    values.push(RegistryValue::text(APP_KEY, "FriendlyAppName", "lmdv"));
    values.push(RegistryValue::text(format!(r"{APP_KEY}\shell\open\command"), "", command));
    values.push(RegistryValue::text(format!(r"{APP_KEY}\SupportedTypes"), ".md", ""));
    values.push(RegistryValue::text(REGISTERED_APPLICATIONS, REGISTERED_NAME, CAPABILITIES_KEY));
    values.push(RegistryValue::text(CAPABILITIES_KEY, "ApplicationName", "lmdv"));
    values.push(RegistryValue::text(CAPABILITIES_KEY, "ApplicationDescription", "Native Markdown viewer and editor"));
    values.push(RegistryValue::text(CAPABILITIES_KEY, "ApplicationIcon", icon));
    values.push(RegistryValue::text(format!(r"{CAPABILITIES_KEY}\FileAssociations"), ".md", PROG_ID));
    if let Some(previous) = previous_prog_id {
        values.push(RegistryValue::text(r"Software\lmdv", "PreviousMdProgId", previous));
    }
    values
}

/// The Default Apps URI for a Windows build number (or for an unknown version).
///
/// Windows 11 understands `registeredAppUser` and opens the page filtered to
/// lmdv; Windows 10 gets the plain page. An unknown build takes the plain page
/// too, which every supported Windows opens.
pub fn settings_uri(build: Option<u32>) -> &'static str {
    match build {
        Some(build) if build >= WINDOWS_11_BUILD => SETTINGS_PAGE_FILTERED,
        _ => SETTINGS_PAGE,
    }
}

/// Registers lmdv for `.md` below `HKEY_CURRENT_USER` and asks the user to
/// confirm, returning which surface opened.
///
/// The registry work matches `packaging/windows/lmdv.nsi`'s association section
/// key for key. `UserChoice` is never touched: a default handler is the user's
/// decision, so the confirmation surface is the only way this changes.
///
/// `active` is the document in front, used only for the Open with fallback.
#[cfg(windows)]
pub(super) fn make_default_for_markdown(active: Option<&Path>) -> Result<Confirmation, AssociationError> {
    let exe = std::env::current_exe().map_err(|source| AssociationError::Exe { source })?;
    let current = read_string(HKEY_CURRENT_USER, &extension_key(), "");
    let previous = current.filter(|id| !id.is_empty() && !id.eq_ignore_ascii_case(PROG_ID));
    for value in association_plan(&exe, previous.as_deref()) {
        write_value(HKEY_CURRENT_USER, &value)?;
    }
    refresh_shell();
    Ok(open_confirmation(active))
}

fn prog_id_key() -> String {
    format!(r"{CLASSES}\{PROG_ID}")
}

fn extension_key() -> String {
    format!(r"{CLASSES}\.md")
}

/// `HKEY_LOCAL_MACHINE`'s version key, read only for the build number.
#[cfg(windows)]
const CURRENT_VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
/// Keeps `rundll32.exe` from flashing a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The UTF-16, NUL-terminated form the registry and shell APIs take.
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Writes one plan value, creating the key when it does not exist yet.
#[cfg(windows)]
fn write_value(root: HKEY, value: &RegistryValue) -> Result<(), AssociationError> {
    let key = wide(&value.key);
    let name = wide(&value.name);
    let data = wide(&value.value);
    // REG_SZ byte count includes the terminating NUL.
    let bytes = u32::try_from(data.len() * 2).unwrap_or(u32::MAX);
    let status = unsafe { RegSetKeyValueW(root, key.as_ptr(), name.as_ptr(), REG_SZ, data.as_ptr().cast(), bytes) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(AssociationError::Registry {
            key: value.key.clone(),
            source: std::io::Error::from_raw_os_error(status as i32),
        })
    }
}

/// Reads one `REG_SZ` value, or `None` when it is absent or unreadable.
#[cfg(windows)]
fn read_string(root: HKEY, key: &str, name: &str) -> Option<String> {
    let key = wide(key);
    let name = wide(name);
    let mut bytes = 0u32;
    let status = unsafe {
        RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, ptr::null_mut(), ptr::null_mut(), &mut bytes)
    };
    if status != ERROR_SUCCESS || bytes < 2 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    let mut bytes = u32::try_from(buffer.len() * 2).ok()?;
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// The Windows 11 build number, when the version key has a readable
/// `CurrentBuildNumber`.
#[cfg(windows)]
fn windows_build() -> Option<u32> {
    read_string(HKEY_LOCAL_MACHINE, CURRENT_VERSION_KEY, "CurrentBuildNumber")?.trim().parse().ok()
}

/// Opens `target` through the shell. A return value above 32 is success; below
/// that it is an error code.
#[cfg(windows)]
fn shell_open(target: &str) -> bool {
    let operation = wide("open");
    let file = wide(target);
    let result = unsafe {
        ShellExecuteW(ptr::null_mut(), operation.as_ptr(), file.as_ptr(), ptr::null(), ptr::null(), SW_SHOWNORMAL)
    };
    result as isize > 32
}

/// Tells Explorer that file associations changed, as the installer does.
#[cfg(windows)]
fn refresh_shell() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, ptr::null(), ptr::null()) };
}

/// Opens the Default Apps page, falling back to Open with for `active`.
#[cfg(windows)]
fn open_confirmation(active: Option<&Path>) -> Confirmation {
    let build = windows_build();
    let page = settings_uri(build);
    if page != SETTINGS_PAGE && shell_open(page) {
        return Confirmation::FilteredSettingsPage;
    }
    if shell_open(SETTINGS_PAGE) {
        return Confirmation::SettingsPage;
    }
    if let Some(path) = active
        && open_with_dialog(path).is_ok()
    {
        return Confirmation::OpenWithDialog;
    }
    Confirmation::Unavailable
}

/// Shows the “Open with” dialog for one file. `rundll32.exe` is a GUI binary, so
/// the child is not waited for and the dialog outlives this call.
#[cfg(windows)]
fn open_with_dialog(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("rundll32.exe")
        .arg("shell32.dll,OpenAs_RunDLL")
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_child| ())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn text<'a>(plan: &'a [RegistryValue], key: &str, name: &str) -> Option<&'a str> {
        plan.iter().find(|value| value.key == key && value.name == name).map(|value| value.value.as_str())
    }

    #[test]
    fn the_plan_writes_the_installers_association_keys() {
        let exe = Path::new(r"C:\Program Files\lmdv\lmdv.exe");
        let plan = association_plan(exe, None);
        let command = r#""C:\Program Files\lmdv\lmdv.exe" "%1""#;
        let icon = r"C:\Program Files\lmdv\lmdv.exe,0";
        let prog = r"Software\Classes\lmdv.md";

        assert_eq!(text(&plan, prog, ""), Some("Markdown document"));
        assert_eq!(text(&plan, prog, "FriendlyTypeName"), Some("Markdown document"));
        assert_eq!(text(&plan, r"Software\Classes\lmdv.md\DefaultIcon", ""), Some(icon));
        assert_eq!(text(&plan, r"Software\Classes\lmdv.md\shell\open\command", ""), Some(command));
        assert_eq!(text(&plan, r"Software\Classes\lmdv.md\shell\edit\command", ""), Some(command));
        assert_eq!(text(&plan, r"Software\Classes\.md", ""), Some("lmdv.md"));
        assert_eq!(text(&plan, r"Software\Classes\.md", "Content Type"), Some("text/markdown"));
        assert_eq!(text(&plan, r"Software\Classes\.md\OpenWithProgids", "lmdv.md"), Some(""));
        assert_eq!(text(&plan, r"Software\Classes\Applications\lmdv.exe", "FriendlyAppName"), Some("lmdv"));
        assert_eq!(text(&plan, r"Software\Classes\Applications\lmdv.exe\shell\open\command", ""), Some(command));
        assert_eq!(text(&plan, r"Software\Classes\Applications\lmdv.exe\SupportedTypes", ".md"), Some(""));
        assert_eq!(text(&plan, r"Software\RegisteredApplications", "lmdv"), Some(r"Software\lmdv\Capabilities"));
        assert_eq!(text(&plan, r"Software\lmdv\Capabilities", "ApplicationName"), Some("lmdv"));
        assert_eq!(
            text(&plan, r"Software\lmdv\Capabilities", "ApplicationDescription"),
            Some("Native Markdown viewer and editor")
        );
        assert_eq!(text(&plan, r"Software\lmdv\Capabilities", "ApplicationIcon"), Some(icon));
        assert_eq!(text(&plan, r"Software\lmdv\Capabilities\FileAssociations", ".md"), Some("lmdv.md"));
        assert_eq!(plan.len(), 16);
    }

    #[test]
    fn the_plan_records_a_previous_progid_only_when_one_is_given() {
        let exe = Path::new(r"C:\lmdv\lmdv.exe");
        let with_previous = association_plan(exe, Some("VSCode.md"));
        assert_eq!(text(&with_previous, r"Software\lmdv", "PreviousMdProgId"), Some("VSCode.md"));
        assert_eq!(with_previous.len(), 17);

        let first_run = association_plan(exe, None);
        assert_eq!(text(&first_run, r"Software\lmdv", "PreviousMdProgId"), None);
    }

    #[test]
    fn the_plan_writes_each_value_once_and_never_touches_user_choice() {
        let plan = association_plan(Path::new(r"C:\lmdv\lmdv.exe"), Some("Other.md"));
        let mut seen = BTreeSet::new();
        for value in &plan {
            assert!(
                seen.insert((value.key.clone(), value.name.clone())),
                "{} {} would be written twice",
                value.key,
                value.name
            );
            assert!(
                !value.key.to_ascii_lowercase().contains("userchoice"),
                "the plan must leave UserChoice to the user: {}",
                value.key
            );
        }
        assert_eq!(
            plan,
            association_plan(Path::new(r"C:\lmdv\lmdv.exe"), Some("Other.md")),
            "the same executable plans the same idempotent writes"
        );
    }

    #[test]
    fn the_default_apps_uri_filters_by_app_only_on_windows_11() {
        assert_eq!(settings_uri(None), SETTINGS_PAGE, "an unknown build takes the plain page");
        assert_eq!(settings_uri(Some(19_045)), SETTINGS_PAGE, "Windows 10 has no app filter");
        assert_eq!(settings_uri(Some(21_999)), SETTINGS_PAGE, "the last build before 21H2");
        assert_eq!(settings_uri(Some(WINDOWS_11_BUILD)), SETTINGS_PAGE_FILTERED);
        assert_eq!(settings_uri(Some(26_100)), SETTINGS_PAGE_FILTERED);
        assert!(settings_uri(Some(26_100)).contains("registeredAppUser=lmdv"));
    }

    #[test]
    fn the_plain_settings_page_is_the_base_the_filter_extends() {
        assert!(SETTINGS_PAGE_FILTERED.starts_with(SETTINGS_PAGE));
        assert_eq!(SETTINGS_PAGE_FILTERED.strip_prefix(SETTINGS_PAGE), Some("?registeredAppUser=lmdv"));
    }
}
