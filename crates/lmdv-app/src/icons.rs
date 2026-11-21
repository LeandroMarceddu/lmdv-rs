//! The few glyphs the P1 shell draws itself.
//!
//! Rune-kit supplies controls and its own close/sidebar glyphs; these cover the
//! app-specific marks (document, start page, folder, help, plus).

use rune::Icon;

/// The app's mark (`assets/lmdv.svg`): a page with its ochre bookmark ribbon,
/// drawn in ink for the start page (H-01).
pub static APP: Icon = Icon {
    name: "lmdv-app",
    view: 16.0,
    body: r#"<path d="M4 1.6h4.9l3.5 3.5v9.2a.6.6 0 0 1-.6.6H4a.6.6 0 0 1-.6-.6V2.2a.6.6 0 0 1 .6-.6Z" stroke="currentColor" stroke-width="1.1" fill="none" stroke-linejoin="round"/><path d="M8.9 1.7v5l1.3-.9 1.3.9v-5" fill="currentColor"/><path d="M5.6 9.1h4.8M5.6 11.2h3.1" stroke="currentColor" stroke-width="1" stroke-linecap="round"/>"#,
};

/// A Markdown document.
pub static DOCUMENT: Icon = Icon {
    name: "lmdv-document",
    view: 16.0,
    body: r#"<path d="M4 1.5h5l3 3V14a.5.5 0 0 1-.5.5h-7A.5.5 0 0 1 4 14V2a.5.5 0 0 1 .5-.5Z" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round"/><path d="M9 1.8V5h3.2M6 8h4M6 10.5h4" stroke="currentColor" stroke-width="1.1" stroke-linecap="round"/>"#,
};

/// Pasted text, as the original's clipboard tab shows it (T-02).
pub static CLIPBOARD: Icon = Icon {
    name: "lmdv-clipboard",
    view: 16.0,
    body: r#"<rect x="3.2" y="2.8" width="9.6" height="11.4" rx="1" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M6 2.8V2a.5.5 0 0 1 .5-.5h3A.5.5 0 0 1 10 2v.8" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round"/><path d="M5.8 6.2h4.4M5.8 8.6h4.4M5.8 11h2.8" stroke="currentColor" stroke-width="1.1" stroke-linecap="round"/>"#,
};

/// The start page.
pub static HOME: Icon = Icon {
    name: "lmdv-home",
    view: 16.0,
    body: r#"<path d="M3 7.2 8 3l5 4.2V13a.6.6 0 0 1-.6.6h-2.9V9.6H6.5v4H3.6A.6.6 0 0 1 3 13Z" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round"/>"#,
};

/// Open a file.
pub static FOLDER: Icon = Icon {
    name: "lmdv-folder",
    view: 16.0,
    body: r#"<path d="M2.5 4.2c0-.4.3-.7.7-.7h3l1.3 1.6h5.3c.4 0 .7.3.7.7v6.4c0 .4-.3.7-.7.7H3.2a.7.7 0 0 1-.7-.7Z" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round"/>"#,
};

/// Add a tab.
pub static PLUS: Icon = Icon {
    name: "lmdv-plus",
    view: 16.0,
    body: r#"<path d="M8 3.4v9.2M3.4 8h9.2" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>"#,
};

/// Take away: the reader-size stepper's smaller end.
pub static MINUS: Icon = Icon {
    name: "lmdv-minus",
    view: 16.0,
    body: r#"<path d="M3.4 8h9.2" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>"#,
};

/// Edit the document (E-01).
pub static PENCIL: Icon = Icon {
    name: "lmdv-pencil",
    view: 16.0,
    body: r#"<path d="M11.1 2.9a1.4 1.4 0 0 1 2 2L5.5 12.5l-2.6.7.7-2.6Z" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round"/><path d="M10.1 3.9 12.1 5.9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};

/// Back to reading (E-01).
pub static BOOK: Icon = Icon {
    name: "lmdv-book",
    view: 16.0,
    body: r#"<path d="M2.5 3h4a2.5 2.5 0 0 1 1.5.5 2.5 2.5 0 0 1 1.5-.5h4v10h-4a2.5 2.5 0 0 0-1.5.5 2.5 2.5 0 0 0-1.5-.5h-4Z" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round"/><path d="M8 3.5v10" stroke="currentColor" stroke-width="1.1"/>"#,
};

/// Show or hide the live preview beside the source.
pub static PREVIEW: Icon = Icon {
    name: "lmdv-preview",
    view: 16.0,
    body: r#"<rect x="1.8" y="3" width="12.4" height="10" rx="1.2" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M8 3v10" stroke="currentColor" stroke-width="1.1"/>"#,
};

/// Find in the document.
pub static SEARCH: Icon = Icon {
    name: "lmdv-search",
    view: 16.0,
    body: r#"<circle cx="7" cy="7" r="4.4" stroke="currentColor" stroke-width="1.3" fill="none"/><path d="m10.4 10.4 3.1 3.1" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>"#,
};

/// The file browser's up button (F-02).
pub static UP: Icon = Icon {
    name: "lmdv-up",
    view: 16.0,
    body: r#"<path d="M8 13V3.6M4.4 7.2 8 3.5l3.6 3.7" stroke="currentColor" stroke-width="1.3" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// Refresh the file browser's listing (F-08).
pub static REFRESH: Icon = Icon {
    name: "lmdv-refresh",
    view: 16.0,
    body: r#"<path d="M12.9 6.6A5 5 0 1 0 13 9.3" stroke="currentColor" stroke-width="1.3" fill="none" stroke-linecap="round"/><path d="M13.2 3.3v3.5H9.8" stroke="currentColor" stroke-width="1.3" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// Share and export (X-01).
pub static SHARE: Icon = Icon {
    name: "lmdv-share",
    view: 16.0,
    body: r#"<path d="M8 10.5V2.6M5.4 5.1 8 2.5l2.6 2.6" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round" stroke-linejoin="round"/><path d="M4.2 8.4h-1a.7.7 0 0 0-.7.7v4.3c0 .4.3.7.7.7h9.6c.4 0 .7-.3.7-.7V9.1a.7.7 0 0 0-.7-.7h-1" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// A successful export, shown for 1.2 s on Share (X-13), and a success notice.
pub static CHECK: Icon = Icon {
    name: "lmdv-check",
    view: 16.0,
    body: r#"<path d="m3.4 8.4 3 3 6.2-6.6" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// An information notice (H-06).
pub static INFO: Icon = Icon {
    name: "lmdv-info",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.1" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M8 7.1v4.3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><circle cx="8" cy="4.5" r=".85" fill="currentColor"/>"#,
};

/// A warning notice (H-06).
pub static WARNING: Icon = Icon {
    name: "lmdv-warning",
    view: 16.0,
    body: r#"<path d="M8 2.1 14.4 13a.6.6 0 0 1-.5.9H2.1a.6.6 0 0 1-.5-.9Z" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round"/><path d="M8 6.1v3.6" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><circle cx="8" cy="11.8" r=".85" fill="currentColor"/>"#,
};

/// An error notice (H-06).
pub static ERROR: Icon = Icon {
    name: "lmdv-error",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.1" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="m5.9 5.9 4.2 4.2M10.1 5.9l-4.2 4.2" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>"#,
};

/// Help.
pub static HELP: Icon = Icon {
    name: "lmdv-help",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="5.9" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M6.5 6.2a1.6 1.6 0 1 1 2.2 1.5c-.5.2-.7.6-.7 1.1v.4" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round"/><circle cx="8" cy="11.6" r=".75" fill="currentColor"/>"#,
};

/// Settings: a gear (P6-PREF).
pub static SETTINGS: Icon = Icon {
    name: "lmdv-settings",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="2.2" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M8 1.8v1.4M8 12.8v1.4M1.8 8h1.4M12.8 8h1.4M3.6 3.6l1 1M11.4 11.4l1 1M12.4 3.6l-1 1M4.6 11.4l-1 1" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};

/// A table, for the action bar's spreadsheet copy (X-16).
pub static TABLE: Icon = Icon {
    name: "lmdv-table",
    view: 16.0,
    body: r#"<rect x="2" y="2.6" width="12" height="10.8" rx="1.2" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M2 6.2h12M6.3 6.2v7.2" stroke="currentColor" stroke-width="1.1"/>"#,
};

/// A document in a tab: F-12's “Open in a new tab”.
pub static TAB: Icon = Icon {
    name: "lmdv-tab",
    view: 24.0,
    body: r#"<rect x="2" y="6" width="20" height="15" rx="2" stroke="currentColor" stroke-width="1.6" fill="none"/><path d="M2 10h20M7 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linejoin="round"/>"#,
};

/// A document in a window: F-12's “Open in a new window”.
pub static WINDOW: Icon = Icon {
    name: "lmdv-window",
    view: 24.0,
    body: r#"<rect x="2" y="3" width="20" height="18" rx="2" stroke="currentColor" stroke-width="1.6" fill="none"/><path d="M2 8h20M12 11v7M8.5 14.5h7" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
};

/// A Markdown link: F-12's “Copy as Markdown link”.
pub static LINK: Icon = Icon {
    name: "lmdv-link",
    view: 24.0,
    body: r#"<path d="M10 13a5 5 0 0 0 7.2.4l3-3a5 5 0 0 0-7-7l-1.7 1.7" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linecap="round" stroke-linejoin="round"/><path d="M14 11a5 5 0 0 0-7.2-.4l-3 3a5 5 0 0 0 7 7l1.7-1.7" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// Open the full-window preview (V-11).
pub static EXPAND: Icon = Icon {
    name: "lmdv-expand",
    view: 16.0,
    body: r#"<path d="M9.6 2.5h3.9v3.9M6.4 13.5H2.5V9.6M13.4 2.6 9.2 6.8M2.6 13.4l4.2-4.2" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// The strip's ⌄ button: every open tab when they overflow (T-17).
pub static CHEVRON_DOWN: Icon = Icon {
    name: "lmdv-chevron-down",
    view: 16.0,
    body: r#"<path d="m3.6 6.1 4.4 4.4 4.4-4.4" stroke="currentColor" stroke-width="1.4" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
