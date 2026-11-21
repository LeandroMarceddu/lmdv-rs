//! Making lmdv the default handler for Markdown files, on both platforms that
//! let an application ask for it.
//!
//! * Windows 10 and 11 protect `UserChoice` (a hash plus the user's consent in
//!   Settings), so no application can silently become the default handler. The
//!   Windows side does what the NSIS installer does — register the per-user
//!   `lmdv.md` ProgID and its Open with / Default apps entries under
//!   `HKEY_CURRENT_USER` — and then opens the surface where the user confirms:
//!   the Default Apps page on Windows 11 (filtered to lmdv when the build
//!   supports it) or, when that page cannot open, the “Open with” dialog for
//!   the document in front. It never reads, deletes or forges `UserChoice`.
//! * Linux has no protected consent hash: an XDG desktop entry plus the user's
//!   `mimeapps.list` are the association, so the press applies at once and
//!   reports [`Confirmation::Applied`]. The user's own desktop entry (the one
//!   `scripts/install-md-handler.sh` writes, or a packaging wrapper) is reused
//!   byte for byte rather than replaced.
//!
//! The pure planners — [`association_plan`], [`settings_uri`] on Windows and
//! [`linux::mimeapps_with_defaults`], [`linux::desktop_entry`] here — compile on
//! every platform, so their tests run on Linux even though only Windows writes
//! the registry.
//!
//! [`linux::apply`] takes every directory it touches, so tests use temp dirs and
//! never read or write the real `$HOME`.

use std::path::Path;

#[cfg(target_os = "linux")]
pub mod linux;
mod windows;

// The Windows planner, re-exported so the `.md` registration can be inspected
// (and tested) on every platform; only Windows writes the registry.
pub use windows::{
    PROG_ID, REGISTERED_NAME, RegistryValue, SETTINGS_PAGE, SETTINGS_PAGE_FILTERED, WINDOWS_11_BUILD, association_plan,
    settings_uri,
};

/// Whether this build offers “make lmdv the default for Markdown files”.
///
/// Windows and Linux both do, by different means; elsewhere the settings row is
/// not drawn and [`make_default_for_markdown`] reports
/// [`AssociationError::Unsupported`].
pub const fn default_supported() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

/// What the press of “Make lmdv the default for Markdown files” did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    /// lmdv is the Markdown handler now. Linux writes the XDG entry and the
    /// `mimeapps.list` defaults directly, so no further confirmation exists.
    Applied,
    /// The Windows 11 Default Apps page, filtered to lmdv.
    FilteredSettingsPage,
    /// The plain Default Apps page.
    SettingsPage,
    /// The settings page would not open, so the Open with dialog was shown for
    /// the active document.
    OpenWithDialog,
    /// Neither surface opened; the registration is still in place, so the user
    /// can choose lmdv in Default apps by hand.
    Unavailable,
}

/// Why making lmdv the default Markdown handler failed.
#[derive(Debug, thiserror::Error)]
pub enum AssociationError {
    /// This platform has no default-handler hand-off.
    #[error("the default Markdown app cannot be set on this platform")]
    Unsupported,
    /// Neither `$XDG_DATA_HOME` nor an absolute `$HOME` is available, so there
    /// is nowhere to install the desktop entry.
    #[error("no home or XDG data directory is available")]
    NoHome,
    /// The running executable's path could not be resolved.
    #[error("the lmdv executable path is unavailable: {source}")]
    Exe {
        #[source]
        source: std::io::Error,
    },
    /// The program path cannot be written into a desktop entry: it has no UTF-8
    /// form, or holds a character desktop files cannot represent reliably
    /// (`"`, `\`, `$`, a backtick, `%` or a newline).
    #[error("{} cannot be written into a desktop entry", path.display())]
    Unencodable { path: std::path::PathBuf },
    /// A registry write was refused.
    #[error("the registry refused {key}: {source}")]
    Registry {
        key: String,
        #[source]
        source: std::io::Error,
    },
    /// A file the association owns could not be read or written.
    #[error("{} could not be read or written: {source}", path.display())]
    File {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A desktop entry lmdv does not own already exists at the user level and
    /// does not name a Markdown handler, so it was left alone.
    #[error("{} exists and does not register a Markdown handler; it was left unchanged", path.display())]
    Unmanaged { path: std::path::PathBuf },
}

/// Registers lmdv for Markdown files and, where the platform asks for it, hands
/// the final choice to the user.
///
/// On Windows this writes the registry and opens Default apps (returning which
/// page opened); on Linux it writes the XDG entry and the user's `mimeapps.list`
/// and returns [`Confirmation::Applied`]. `active` is the document in front,
/// used only for the Windows Open with fallback.
#[cfg(windows)]
pub fn make_default_for_markdown(active: Option<&Path>) -> Result<Confirmation, AssociationError> {
    windows::make_default_for_markdown(active)
}

/// Writes the XDG association and the `mimeapps.list` defaults for the running
/// program. `active` is unused: Linux needs no confirmation surface.
#[cfg(target_os = "linux")]
pub fn make_default_for_markdown(_active: Option<&Path>) -> Result<Confirmation, AssociationError> {
    linux::make_default_for_markdown()
}

/// Neither Windows nor Linux: the settings row is hidden and the action says
/// the hand-off does not exist here.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn make_default_for_markdown(_active: Option<&Path>) -> Result<Confirmation, AssociationError> {
    Err(AssociationError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_follows_the_platform() {
        assert_eq!(default_supported(), cfg!(any(windows, target_os = "linux")));
    }

    #[test]
    fn the_unsupported_error_reads_as_a_platform_limit() {
        let text = AssociationError::Unsupported.to_string();
        assert!(text.contains("platform"), "{text}");
        assert!(!text.contains("Windows"), "the message must not promise Windows where Linux is supported: {text}");
    }
}
