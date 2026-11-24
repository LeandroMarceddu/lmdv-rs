//! The file-browser pane and folder-wide search (F-01–F-14).
//!
//! The pane is rooted at a folder. Opening a document outside the root re-roots
//! it there (F-01); opening one inside keeps it; the up button moves to the
//! parent (F-02). The listing itself is a flattened `lmdv_io::tree::TreeListing`
//! built on a worker: folders before files, only Markdown, four levels deep,
//! 2,000 files, hidden/build/system folders skipped and empty folders hidden
//! (F-03–F-06). Folders are expanded by default; only collapsed paths are kept,
//! lowercased, in `ui.json` (F-07).
//!
//! A recursive `TreeWatcher` on the root asks for a fresh listing after a
//! create/remove/rename, never a content write (F-08). When the new listing is
//! byte-for-byte the same as the last one, the rows, scroll and focus are left
//! alone; when it differs, the focused path (or its nearest listed folder) is
//! restored without scrolling (F-09).
//!
//! Folder search (F-13/F-14) is debounced 250 ms, searches the same tree on a
//! worker and draws grouped results above the tree; the query can be prefilled
//! from the article selection, results move with Up/Down while the field keeps
//! focus, and Return or a click opens the file at its line.
//!
//! The context menu (F-12) opens on a secondary press over a row, or from
//! Shift+F10/the Menu key on the focused row, positioned under that row. Files
//! offer a background tab, a new window, the desktop's file manager, the path
//! and a Markdown link relative to the open document; folders only the last
//! two. A menu is a `rune_kit::Menu`, so A-08's roles and keys (Up/Down wrap,
//! Home/End, Escape/Tab, focus return) are the kit's.
//!
//! # The pane's state across windows (F-18–F-21, OI-129)
//!
//! The pane's visibility and width, the collapsed set and the browser's root seed
//! live in `ui.json`; P-09 stores `tree-hidden`/`toc-hidden` as one choice, so
//! every window shares them. The **root follows each window's own document**
//! (F-01): the stored root is only the folder the next launch starts in, and an
//! automatic follow never rewrites it, so two windows on two projects do not
//! overwrite each other's state. A window that writes `ui.json` asks every other
//! window to redraw, and each window pulls the changed fields on its next poll:
//! a collapse, a resize or a hidden choice reaches the others without a
//! per-field message.
//!
//! Only a **visible** pane runs the recursive watcher: a hidden pane needs no
//! live updates, and a process with several windows then keeps at most one
//! inotify watch per visible root. A watcher that fails because the system's
//! watch limit was reached says so once, instead of only the generic failure.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use lmdv_io::Launch;
use lmdv_io::store::UiPrefs;
use lmdv_io::tree::{SearchHit, TreeListing};
use lmdv_io::watch::TreeWatcher;
use rune::{
    Cursor, Cx, El, FileDialogOptions, GrowthSpec, Key, KeyEvent, Mods, PointerPress, ScrollSpec, Span, Tx, col, el,
    icon, rich, row, text,
};
use rune_kit::shell::Sidebar;
use rune_kit::{Button, ButtonVariant, IconButton, IconButtonVariant, Kit, MenuItem, TextInput};
use rune_kit::{TreeItem, TreeRow, TreeStyle, file_icon};

use crate::icons;
use crate::jobs::{JobMsg, WorkerMailbox};
use crate::shared::UiMsg;
use crate::window::{Mode, WindowState};
use crate::windows::{WindowKind, WindowOpenRequest};

/// The article width below which panes auto-fold, outline first (F-19).
pub const ARTICLE_MIN_WIDTH: f32 = 520.0;
/// What the shell's frame, the panel padding and the pane gaps take from the
/// width the article would otherwise get, in points.
pub const PANE_GUTTER: f32 = 24.0;
/// A resize arrow's step (F-20).
pub const PANE_STEP: f32 = 16.0;
/// Shift with a resize arrow (F-20).
pub const PANE_STEP_LARGE: f32 = 64.0;
/// The tree's element key, stable across rebuilds so focus stays in it.
pub const BROWSER_TREE_KEY: &str = "browser-tree";
/// The search field's element key (`Ctrl+Shift+F` focuses it).
pub const BROWSER_SEARCH_KEY: &str = "browser-search";
/// The row height the virtualised tree assumes.
pub const BROWSER_ROW_HEIGHT: f32 = 24.0;
/// Folder search waits this long after the last key (F-13).
pub const SEARCH_DEBOUNCE: f64 = 0.25;
/// The article selection a search prefill may use, in characters (R-17).
pub const SEARCH_PREFILL_MAX: usize = 100;
/// The context menu's key (`browser-menu-item0`, …).
pub const BROWSER_MENU_KEY: &str = "browser-menu";
/// The menu's zero-size anchor in the layer.
pub const BROWSER_MENU_ANCHOR: &str = "browser-menu-anchor";
/// The transparent window layer that closes the menu on an outside press.
pub const BROWSER_MENU_SCRIM: &str = "browser-menu-scrim";
/// A keyboard-opened menu starts this far into the row from its left edge (F-12).
pub const BROWSER_MENU_INSET: f32 = 12.0;
/// And this far below its bottom edge.
pub const BROWSER_MENU_GAP: f32 = 2.0;

/// One visible row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserRow {
    /// The absolute path, also the row's identity and element key.
    pub path: String,
    /// The file or folder name.
    pub name: String,
    /// Nesting level, 0 for the root's children.
    pub depth: u32,
    /// Whether this is a folder row.
    pub dir: bool,
    /// A folder's open state.
    pub expanded: bool,
}

/// One row of the folder-search results: a file header (line 0) or a line hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRow {
    pub path: PathBuf,
    pub name: String,
    /// 1-based line; 0 for a file header.
    pub line: u32,
    pub preview: String,
}

impl SearchRow {
    /// Whether this row opens the file without a line.
    fn is_file(&self) -> bool {
        self.line == 0
    }
}

/// The file or folder F-12's context menu acts on.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowserMenu {
    /// The row's absolute path.
    pub path: PathBuf,
    /// The row's name, used for a Markdown link's label.
    pub name: String,
    /// Whether the row is a folder (no Open entries).
    pub dir: bool,
    /// Where the surface opens, in window points. A keyboard menu follows
    /// its focused row: the view re-reads the row's rect every frame.
    point: std::cell::Cell<PointerPress>,
    /// Shift+F10/the Menu key opened it; the pointer did not choose the point.
    from_keyboard: bool,
    /// The view has not focused the first row yet (a scene opens a menu
    /// before any handler can ask; the next `poll` does it).
    needs_focus: bool,
}

impl BrowserMenu {
    /// Where the menu opens, in window points.
    pub fn point(&self) -> PointerPress {
        self.point.get()
    }

    /// Whether the keyboard opened it.
    pub fn is_keyboard(&self) -> bool {
        self.from_keyboard
    }

    /// Moves a keyboard menu under its row's latest rect.
    fn follow_row(&self, rect: [f32; 4]) {
        self.point.set(PointerPress { x: rect[0] + BROWSER_MENU_INSET, y: rect[1] + rect[3] + BROWSER_MENU_GAP });
    }
}

/// One row of the F-12 menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserMenuAction {
    /// Open a background tab (files only).
    OpenTab,
    /// Open in a new window (files only).
    OpenWindow,
    /// `xdg-open` the row's folder (F-12's Show in File Explorer).
    Reveal,
    /// Copy the absolute path.
    CopyPath,
    /// Copy `[name](relative path)`, percent-encoded and relative to the open
    /// document (files only).
    CopyLink,
}

/// A menu row as the view draws it, so its contents, order and enabled state
/// are testable without a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowserMenuEntry {
    pub action: BrowserMenuAction,
    pub label: &'static str,
    pub hint: Option<&'static str>,
    pub description: Option<&'static str>,
    pub enabled: bool,
    /// A separator goes before this row.
    pub separator_before: bool,
}

/// F-12's rows for a file or folder, in order.
///
/// A file gets the two Open rows, a separator, then Show in File Explorer,
/// Copy path and Copy as Markdown link. A folder gets only the last two: it has
/// no document to open. The desktop's file manager needs a parent folder, so
/// the root folder's Show row is disabled.
pub fn browser_menu_entries(menu: &BrowserMenu) -> Vec<BrowserMenuEntry> {
    let mut entries = Vec::new();
    if !menu.dir {
        entries.push(BrowserMenuEntry {
            action: BrowserMenuAction::OpenTab,
            label: lmdv_i18n::t!("file.openNewTab"),
            hint: Some(lmdv_i18n::t!("file.ctrlClick")),
            description: None,
            enabled: true,
            separator_before: false,
        });
        entries.push(BrowserMenuEntry {
            action: BrowserMenuAction::OpenWindow,
            label: lmdv_i18n::t!("file.openNewWindow"),
            hint: Some(lmdv_i18n::t!("file.shiftClick")),
            description: None,
            enabled: true,
            separator_before: false,
        });
    }
    // The file rows' Open group stands apart from the path actions.
    let after_open = !menu.dir;
    entries.push(BrowserMenuEntry {
        action: BrowserMenuAction::Reveal,
        label: lmdv_i18n::t!("file.reveal"),
        hint: None,
        description: None,
        enabled: menu.path.parent().is_some(),
        separator_before: after_open,
    });
    entries.push(BrowserMenuEntry {
        action: BrowserMenuAction::CopyPath,
        label: lmdv_i18n::t!("file.copyPath"),
        hint: None,
        description: None,
        enabled: true,
        separator_before: false,
    });
    if !menu.dir {
        entries.push(BrowserMenuEntry {
            action: BrowserMenuAction::CopyLink,
            label: lmdv_i18n::t!("file.copyLink"),
            hint: None,
            description: Some(lmdv_i18n::t!("file.copyLinkHint")),
            enabled: true,
            separator_before: false,
        });
    }
    entries
}

/// The key of the first enabled row: the menu opens with it focused, so the
/// arrow keys work and Tab/Escape close from inside (A-08).
///
/// A file's `Show in File Explorer` row stands after a separator, so its key
/// is `browser-menu-item3`, not `-item2`: the separator takes an entry
/// position in `Menu`'s list.
pub fn browser_menu_first_item(entries: &[BrowserMenuEntry]) -> Option<String> {
    let mut index = 0;
    for entry in entries {
        if entry.separator_before {
            index += 1;
        }
        if entry.enabled {
            return Some(format!("{BROWSER_MENU_KEY}-item{index}"));
        }
        index += 1;
    }
    None
}

/// `to` relative to `from_dir`, with forward slashes, for a Markdown link (F-12).
///
/// The original compares path segments case-insensitively and falls back to
/// the whole path when the two share no leading segment.
pub fn relative_path(from_dir: &Path, to: &Path) -> String {
    let from = path_segments(from_dir);
    let target = path_segments(to);
    let mut common = 0;
    while common < from.len() && common < target.len() && from[common].eq_ignore_ascii_case(&target[common]) {
        common += 1;
    }
    if common == 0 {
        return to.to_string_lossy().replace('\\', "/");
    }
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), from.len() - common).collect();
    parts.extend(target[common..].iter().cloned());
    parts.join("/")
}

fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// A Markdown link to `target`: `[name](relative path)`, without the
/// Markdown extension in the label and percent-encoded in the destination.
///
/// Without an open document folder, the path stays absolute, as the original
/// does for pasted text. `/` stays literal; `#` and `?` are encoded so a file
/// name never reads as a fragment or a query.
pub fn markdown_link(from_dir: Option<&Path>, target: &Path, name: &str) -> String {
    let path = match from_dir {
        Some(dir) => relative_path(dir, target),
        None => target.to_string_lossy().replace('\\', "/"),
    };
    format!("[{}]({})", link_label(name), encode_uri_path(&path))
}

/// The link label: the file name minus a Markdown extension, as the original's
/// `fileName(path).replace(MD_EXT_RE, "")` reads it.
fn link_label(name: &str) -> &str {
    if let Some((stem, extension)) = name.rsplit_once('.')
        && !stem.is_empty()
        && crate::commands::MARKDOWN_EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known))
    {
        return stem;
    }
    name
}

/// Percent-encodes a relative path for a Markdown destination.
///
/// RFC 3986's path characters stay literal; a space, `%`, `#`, `?`, `[`, `]`
/// and every non-ASCII byte take `%XX`, so the link survives a Markdown renderer.
fn encode_uri_path(path: &str) -> String {
    const LITERAL: &[u8] = b"-._~!$&'()*+,;=:@/";
    let mut out = String::with_capacity(path.len());
    for &byte in path.as_bytes() {
        if byte.is_ascii_alphanumeric() || LITERAL.contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0').to_ascii_uppercase());
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0').to_ascii_uppercase());
        }
    }
    out
}

/// A message to the folder-watch thread.
enum WatchMsg {
    /// Follow this root, or none.
    Root(Option<PathBuf>),
    /// Reinstall the recursive watch after focus returns (Linux watchers can
    /// be dropped silently).
    Rearm,
}

/// One window's recursive folder watch, installed off the UI thread (D7).
///
/// The thread owns the `TreeWatcher`; a change posts [`JobMsg::TreeChanged`]
/// with the watched root so a stale watcher from a previous root is ignored.
pub(crate) struct TreeWatch {
    tx: Sender<WatchMsg>,
}

impl TreeWatch {
    fn start(mailbox: Arc<WorkerMailbox>) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<WatchMsg>();
        let spawned = std::thread::Builder::new().name("lmdv-tree-watch".to_owned()).spawn(move || {
            let mut watcher: Option<TreeWatcher> = None;
            while let Ok(message) = rx.recv() {
                match message {
                    WatchMsg::Root(None) => watcher = None,
                    WatchMsg::Root(Some(root)) => {
                        watcher = None;
                        let notify = mailbox.clone();
                        let watched = root.clone();
                        match TreeWatcher::new(&root, move |_| {
                            notify.post(JobMsg::TreeChanged { root: watched.clone() })
                        }) {
                            Ok(installed) => watcher = Some(installed),
                            Err(error) => mailbox.post(JobMsg::TreeWatchFailed { root, error: error.to_string() }),
                        }
                    }
                    WatchMsg::Rearm => {
                        if let Some(watcher) = watcher.as_mut() {
                            let root = watcher.root().to_path_buf();
                            if let Err(error) = watcher.rearm() {
                                // A dropped watch that cannot come back is a
                                // change the pane will not see; say so once
                                // (OI-129). `rearm` used to lose this.
                                mailbox.post(JobMsg::TreeWatchFailed { root, error: error.to_string() });
                            }
                        }
                    }
                }
            }
        });
        spawned.ok().map(|_| Self { tx })
    }

    fn set_root(&self, root: Option<PathBuf>) {
        let _ = self.tx.send(WatchMsg::Root(root));
    }

    fn rearm(&self) {
        let _ = self.tx.send(WatchMsg::Rearm);
    }
}

/// The file browser's per-window state (not the pane's geometry).
#[derive(Default)]
pub struct BrowserState {
    pub(crate) root: Option<PathBuf>,
    /// The canonical spelling of the root, from the last listing. A symlinked
    /// root differs from its target; "inside the root" must accept both.
    pub(crate) canonical_root: Option<PathBuf>,
    pub(crate) root_name: String,
    pub(crate) root_error: Option<String>,
    pub(crate) listing: Option<TreeListing>,
    pub(crate) rows: Vec<BrowserRow>,
    /// Lowercased absolute folder paths, persisted in `ui.json` (F-07).
    pub(crate) collapsed: BTreeSet<String>,
    /// The row with the keyboard cursor, by path.
    pub(crate) focus: Option<String>,
    /// The active document's path.
    pub(crate) active: Option<String>,
    /// The last active document the pane reacted to.
    pub(crate) last_active: Option<String>,
    /// Changes whenever the drawn rows, their focus or the active row change,
    /// so the virtual row list notices an in-place change.
    pub(crate) row_epoch: u64,
    pub(crate) tree_generation: u64,
    pub(crate) tree_pending: bool,
    /// A folder-link's previous root while its listing is in flight (L-05).
    /// `Some(None)` returns to "no folder open"; `None` is not a folder link.
    pub(crate) link_revert: Option<Option<PathBuf>>,
    pub(crate) search_generation: u64,
    pub(crate) search_due: Option<f64>,
    pub(crate) search_query: String,
    pub(crate) search_open: bool,
    pub(crate) search_pending: bool,
    pub(crate) results: Vec<SearchRow>,
    pub(crate) search_selected: Option<usize>,
    pub(crate) watch: Option<TreeWatch>,
    /// The root the watch thread follows, so a poll does not send the same
    /// root every frame.
    pub(crate) watched_root: Option<PathBuf>,
    pub(crate) watch_error: Option<String>,
    /// The watch limit was reported once (OI-129).
    pub(crate) watch_limit_noticed: bool,
    /// The open context menu (F-12).
    pub(crate) menu: Option<BrowserMenu>,
    /// The stored pane choices this window last pulled from `ui.json`
    /// (F-18/F-20, OI-129). `None` until the first poll has read them.
    pub(crate) seen_ui: Option<SeenUi>,
    /// Which pane's edge was dragged on the previous poll, so its release
    /// persists the width (F-20).
    pub(crate) resizing: [bool; 2],
    /// The panes F-19's auto-fit folded, so only its own folds are reopened.
    pub(crate) auto_folded: [bool; 2],
    /// The user opened a folded pane anyway; auto-fit leaves it alone until the
    /// whole shell fits again (F-19's peek is not wired yet).
    pub(crate) fit_override: [bool; 2],
    /// The shell's width, as the last drawn frame measured it, for the auto-fold
    /// decision the next poll makes (F-19).
    pub(crate) shell_width: std::cell::Cell<f32>,
    /// The magnify gesture's accumulated delta over the pane (F-21).
    pub(crate) magnify: f32,
}

/// The shared `ui.json` fields a window applies when another window writes
/// them (OI-129). The root is not here: it follows each window's document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SeenUi {
    pub(crate) left_hidden: bool,
    pub(crate) right_hidden: bool,
    pub(crate) left_width: f32,
    pub(crate) right_width: f32,
    /// The stored collapsed set's size: equal sizes plus membership is the set.
    pub(crate) collapsed_len: usize,
}

impl BrowserState {
    /// The visible rows in display order.
    pub fn rows(&self) -> &[BrowserRow] {
        &self.rows
    }

    /// The folder the pane is rooted at.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The header's folder name.
    pub fn root_name(&self) -> &str {
        if self.root.is_none() || self.root_name.is_empty() { lmdv_i18n::t!("pane.files") } else { &self.root_name }
    }

    /// The row with the keyboard cursor.
    pub fn focused(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    /// The active document's row, when it is listed.
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// Whether search results are on screen.
    pub fn is_searching(&self) -> bool {
        self.search_open
    }

    /// The grouped search results on screen.
    pub fn results(&self) -> &[SearchRow] {
        &self.results
    }

    /// The highlighted result, if any.
    pub fn selected_result(&self) -> Option<usize> {
        self.search_selected
    }

    /// Whether `path` names a visible row.
    fn has_row(&self, path: &str) -> bool {
        self.rows.iter().any(|row| row.path == path)
    }

    /// The row a screen reader should announce: the focused row, else the
    /// active document's, else the first row (F-10).
    fn active_descendant(&self) -> Option<String> {
        self.focus
            .clone()
            .or_else(|| self.active.clone())
            .filter(|path| self.has_row(path))
            .or_else(|| self.rows.first().map(|row| row.path.clone()))
    }
}

/// The lowercased absolute path a collapsed set stores (F-07).
pub fn collapse_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// Whether a watcher error is the system's watch limit (OI-129): inotify's
/// "No space left on device" (`ENOSPC`), a descriptor limit or an explicit
/// `max_user_watches` message. Those have one remedy the user can act on; every
/// other failure keeps the generic notice.
pub fn is_watch_limit(error: &str) -> bool {
    let error = error.to_lowercase();
    ["no space left", "os error 28", "too many open files", "os error 24", "max_user_watches", "watch limit"]
        .iter()
        .any(|hint| error.contains(hint))
}

/// The first line of an article selection, trimmed and capped, for `Ctrl+Shift+F`
/// (R-17, F-13).
pub fn search_prefill(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().chars().take(SEARCH_PREFILL_MAX).collect()
}

impl WindowState {
    /// Whether the up button can move the root to its parent (F-02).
    pub(crate) fn browser_can_go_up(&self) -> bool {
        self.browser.root.as_deref().is_some_and(|root| root.parent().is_some())
    }

    /// Roots the pane at `root`, persists the choice and lists it (F-01).
    pub(crate) fn set_browser_root(&mut self, root: PathBuf) {
        self.adopt_browser_root(root, true);
    }

    /// Restores the stored root on startup without rewriting `ui.json`.
    ///
    /// F-18/F-20: the pane's stored visibility and width come back with it, so
    /// a hidden tree stays hidden and a widened one keeps its width.
    pub(crate) fn adopt_browser_root_for_startup(&mut self, root: PathBuf) {
        // The first root a window adopts also restores the pane choices it
        // stored (F-18/F-20); a window born without a saved root reaches this
        // as soon as its first document gives it one.
        self.adopt_browser_root(root, false);
        if self.browser.seen_ui.is_none() {
            self.restore_pane_choices();
        }
    }

    /// Makes the pane follow an opened document (F-01): a document outside the
    /// root moves it to the document's folder; one inside keeps the root.
    ///
    /// The follow is this window's own view of the document and is not written
    /// back: `ui.json`'s root is the folder a new window starts in, chosen
    /// explicitly (F-18/OI-129), so two windows on two projects do not
    /// overwrite each other's folder.
    pub(crate) fn follow_document_root(&mut self, path: &Path) {
        let Some(parent) = path.parent() else { return };
        let inside = self.browser.root.as_deref().is_some_and(|root| path.starts_with(root))
            || self.browser.canonical_root.as_deref().is_some_and(|root| path.starts_with(root));
        if inside {
            return;
        }
        if self.browser.root.as_deref() == Some(parent) {
            return;
        }
        self.adopt_browser_root(parent.to_path_buf(), false);
    }

    fn adopt_browser_root(&mut self, root: PathBuf, persist: bool) {
        let root = lmdv_io::absolute_lexical(&root);
        self.browser.root_error = None;
        self.close_browser_menu();
        if self.browser.root.as_deref() == Some(root.as_path()) {
            // Same root: keep the rows and scroll, just refresh what is on disk.
            // An explicit choice of that same root still records it as the
            // folder a new window starts in (OI-129).
            if persist {
                self.persist_root_seed(&root);
            }
            self.refresh_browser();
            return;
        }
        self.browser.root = Some(root.clone());
        self.browser.canonical_root = None;
        self.browser.root_name = lmdv_io::tree::folder_name(&root);
        self.browser.listing = None;
        self.browser.rows.clear();
        self.browser.row_epoch += 1;
        self.browser.link_revert = None;
        self.browser.focus = None;
        self.clear_browser_search();
        if persist {
            self.persist_root_seed(&root);
        }
        // F-18/F-20: the first folder a window gets applies the stored pane
        // visibility and width, so `lmdv file.md` shows the tree when the last
        // session had it shown.
        if self.browser.seen_ui.is_none() {
            self.restore_pane_choices();
        }
        self.sync_tree_watch();
        self.request_tree_listing();
    }

    /// Records the folder a new window starts in (OI-129); the follow never
    /// calls this, only an explicit Open folder/Up/folder-link choice.
    fn persist_root_seed(&mut self, root: &Path) {
        let mut ui = self.shared.ui().clone();
        if ui.tree_root.as_deref() != Some(root) {
            ui.tree_root = Some(root.to_path_buf());
            self.write_ui(ui);
        }
    }

    /// Returns to “no folder open” after a failed folder link (L-05).
    fn forget_browser_root(&mut self) {
        self.close_browser_menu();
        self.browser.root = None;
        self.browser.canonical_root = None;
        self.browser.root_name.clear();
        self.browser.root_error = None;
        self.browser.listing = None;
        self.browser.rows.clear();
        self.browser.row_epoch += 1;
        self.browser.focus = None;
        if let Some(watch) = &self.browser.watch {
            watch.set_root(None);
        }
        self.browser.watched_root = None;
        self.clear_browser_search();
        let mut ui = self.shared.ui().clone();
        if ui.tree_root.is_some() {
            ui.tree_root = None;
            self.write_ui(ui);
        }
    }

    /// Moves the root to its parent (F-02). False at a filesystem root.
    pub(crate) fn browser_up(&mut self) -> bool {
        let Some(parent) = self.browser.root.as_deref().and_then(Path::parent).map(Path::to_path_buf) else {
            return false;
        };
        self.set_browser_root(parent);
        true
    }

    /// Lists the root again without changing it (the header's refresh).
    pub(crate) fn refresh_browser(&mut self) {
        if self.browser.root.is_some() {
            self.request_tree_listing();
        }
    }

    /// A watcher report: list again unless a listing is already on its way.
    pub(crate) fn request_tree_refresh_from_watch(&mut self) {
        if self.browser.root.is_some() && !self.browser.tree_pending {
            self.request_tree_listing();
        }
    }

    fn request_tree_listing(&mut self) {
        let Some(root) = self.browser.root.clone() else { return };
        self.browser.tree_generation = self.browser.tree_generation.wrapping_add(1);
        let generation = self.browser.tree_generation;
        self.browser.tree_pending = true;
        if self.shared.is_headless() {
            // One settled frame: `poll_browser` lists inline, so no worker race.
            return;
        }
        self.shared.jobs().list_tree(self.worker().clone(), root, generation);
    }

    /// Points this window's recursive watch at the root, but only while the pane
    /// is visible (OI-129): a hidden pane needs no live updates, and a process
    /// with several windows then keeps inotify watches only for visible roots.
    fn sync_tree_watch(&mut self) {
        if self.shared.is_headless() {
            // A one-frame gate does not need inotify; tests drive listings by hand.
            return;
        }
        let wanted = if self.left.is_open() || self.left.is_peeking() { self.browser.root.clone() } else { None };
        if wanted.is_none() && self.browser.watch.is_none() {
            return;
        }
        if self.browser.watch.is_none() {
            self.browser.watch = TreeWatch::start(self.worker().clone());
        }
        if self.browser.watched_root == wanted {
            return;
        }
        self.browser.watched_root = wanted.clone();
        if let Some(watch) = &self.browser.watch {
            watch.set_root(wanted);
        }
    }

    /// Reinstalls the recursive watch (the window regained focus).
    pub(crate) fn rearm_browser_watch(&mut self) {
        if !self.left.is_open() && !self.left.is_peeking() {
            return;
        }
        if let Some(watch) = &self.browser.watch {
            watch.rearm();
        }
    }

    /// Opens one document (or a search hit) with the placement modifiers the
    /// original file tree uses: Ctrl a tab, Ctrl+Shift a front tab, Shift a
    /// window, plain here (F-11, F-16).
    pub(crate) fn open_browser_file(&mut self, path: PathBuf, line: Option<usize>, mods: Mods, tx: &mut Tx) {
        self.open_browser_file_with_query(path, line, mods, None, tx);
    }

    /// [`Self::open_browser_file`] with a folder-search query to carry into the
    /// opened document's reading find bar (F-16).
    fn open_browser_file_with_query(
        &mut self,
        path: PathBuf,
        line: Option<usize>,
        mods: Mods,
        query: Option<&str>,
        tx: &mut Tx,
    ) {
        // F-19: choosing an item ends a peeking pane.
        self.left.end_peek();
        if mods.shift && !mods.ctrl {
            self.open_path_in_new_window(path, line);
            return;
        }
        let background = mods.ctrl && !mods.shift;
        // T-13: a background open must not consume a front start page; the
        // placement-aware open handles it, so no switch-back is needed.
        let opened = if background { self.open_path_background(path) } else { self.open_path(path) };
        // Whichever tab is in front now draws on this frame with its own
        // place; the article viewport must not keep the other document's.
        self.apply_pending_scroll(tx);
        if let Some(line) = line
            && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == opened)
        {
            tab.view.line = Some(line.max(1));
        }
        // F-16: a folder-search hit hands its query to the new document's
        // reading find bar. A new window cannot carry it (its launch has no
        // query), which the original accepts too.
        if let Some(query) = query.filter(|query| !query.is_empty()) {
            self.prefill_reader_find(opened, query, !background, tx);
        }
    }

    fn open_path_in_new_window(&mut self, path: PathBuf, line: Option<usize>) {
        let position = self.shared.cascade_position(self.id());
        let request = WindowOpenRequest {
            launch: Launch { path: Some(path), line, ..Launch::default() },
            kind: WindowKind::Normal,
            position,
        };
        if let Err(error) = std::rc::Rc::clone(&self.shared).request_new_window(request) {
            self.notice(lmdv_i18n::t!("msg.openWindowFailed", "err" => error));
        }
    }

    /// Toggles a folder row and persists the collapsed set (F-07).
    pub(crate) fn toggle_browser_folder(&mut self, path: &str) {
        let key = collapse_key(Path::new(path));
        if !self.browser.collapsed.remove(&key) {
            self.browser.collapsed.insert(key);
        }
        self.persist_collapsed();
        self.rebuild_browser_rows();
        // A row inside a closing folder cannot keep the focus; its folder does.
        if let Some(focus) = self.browser.focus.clone()
            && !self.browser.has_row(&focus)
            && self.browser.has_row(path)
        {
            self.browser.focus = Some(path.to_owned());
        }
        self.browser.row_epoch += 1;
    }

    fn persist_collapsed(&mut self) {
        let folders: Vec<PathBuf> = self.browser.collapsed.iter().map(PathBuf::from).collect();
        let mut ui = self.shared.ui().clone();
        if let Some(seen) = self.browser.seen_ui.as_mut() {
            seen.collapsed_len = folders.len();
        }
        if ui.collapsed_folders != folders {
            ui.collapsed_folders = folders;
            self.write_ui(ui);
        }
    }

    // ---- shared pane state (F-18–F-21, OI-129) -----------------------------

    /// Writes `ui.json` and asks every other window to redraw, so it pulls the
    /// change on its next poll (OI-129). One writer, one broadcast.
    fn write_ui(&mut self, ui: UiPrefs) {
        self.shared.set_ui(ui);
        let me = self.id();
        let others: Vec<_> = self.shared.windows().iter().map(|entry| entry.id).filter(|id| *id != me).collect();
        for id in others {
            // `PrefsChanged` is the existing "shared state changed, draw again"
            // message; the window's `poll_browser` rereads what it needs.
            let _ = self.shared.try_post_window(id, UiMsg::PrefsChanged);
        }
    }

    /// Restores the stored pane visibility and width once, when a window starts
    /// with a saved root (F-18/F-20).
    fn restore_pane_choices(&mut self) {
        let (left_hidden, right_hidden, left_width, right_width) = {
            let ui = self.shared.ui();
            (ui.left_hidden, ui.right_hidden, ui.left_width, ui.right_width)
        };
        self.left.set_open(!left_hidden);
        self.left.set_width(left_width);
        self.right.set_width(right_width);
        if right_hidden {
            for tab in &mut self.tabs {
                tab.view.outline_hidden = true;
            }
            if !self.is_editing() {
                self.right.set_open(false);
            }
        }
        let collapsed_len = self.shared.ui().collapsed_folders.len();
        self.browser.seen_ui = Some(SeenUi { left_hidden, right_hidden, left_width, right_width, collapsed_len });
    }

    /// Applies what other windows wrote to `ui.json` and notices this window's
    /// own geometry changes (OI-129). Returns whether anything was redrawn.
    ///
    /// The root is not synced: F-01 makes it per window. The collapsed set, the
    /// pane widths, the hidden choices and the panel's text size are global
    /// (F-18/F-21, P-09), so one window's change reaches all of them.
    pub(crate) fn sync_browser_ui(&mut self) -> bool {
        let (left_hidden, right_hidden, left_width, right_width, collapsed_len) = {
            let ui = self.shared.ui();
            (ui.left_hidden, ui.right_hidden, ui.left_width, ui.right_width, ui.collapsed_folders.len())
        };
        let editing = self.is_editing();
        let mut changed = false;

        // F-20: a drag that ended since the last poll is a choice.
        let resizing = [self.left.is_resizing(), self.right.is_resizing()];
        if (self.browser.resizing[0] && !resizing[0]) || (self.browser.resizing[1] && !resizing[1]) {
            self.persist_widths();
        }
        self.browser.resizing = resizing;

        let seen = match self.browser.seen_ui {
            Some(seen) => seen,
            None => {
                // A window without a saved root has nothing to restore into;
                // snapshot the choices and apply only the widths, which every
                // pane can take without showing anything.
                self.left.set_width(left_width);
                self.right.set_width(right_width);
                let seen = SeenUi { left_hidden, right_hidden, left_width, right_width, collapsed_len };
                self.browser.seen_ui = Some(seen);
                seen
            }
        };

        // F-18/P-09: the tree's hidden choice is one shared value, so a toggle
        // in another window shows or hides this one too. Auto-fit may fold a
        // shown pane again at once if the shell is narrow.
        if left_hidden != seen.left_hidden && !editing {
            if left_hidden {
                if self.left.is_peeking() {
                    self.left.end_peek();
                }
                self.left.set_open(false);
            } else {
                self.left.set_open(true);
                self.refresh_browser();
            }
            changed = true;
            self.browser.seen_ui = Some(SeenUi { left_hidden, right_hidden, ..seen });
        }
        if right_hidden != seen.right_hidden {
            // Hiding is global; showing clears the flag and `sync_outline`
            // opens the pane in this same poll, so the shared choice travels.
            self.set_outline_tabs_hidden(right_hidden);
            if right_hidden && !editing && self.right.is_open() {
                self.right.set_open(false);
            }
            changed = true;
            if !right_hidden {
                self.browser.auto_folded[1] = false;
            }
            self.browser.seen_ui = Some(SeenUi { right_hidden, left_hidden, ..seen });
        }
        // A stored "hidden" stays true for tabs created after the choice.
        if right_hidden && !editing {
            changed |= self.set_outline_tabs_hidden(true);
        }

        // F-20: widths are global; a resize in another window is applied unless
        // this one is dragging its own edge.
        if left_width != seen.left_width && !resizing[0] {
            self.left.set_width(left_width);
            changed = true;
        }
        if right_width != seen.right_width && !resizing[1] {
            self.right.set_width(right_width);
            changed = true;
        }
        let mut current = self.browser.seen_ui.unwrap_or(seen);
        current.left_width = left_width;
        current.right_width = right_width;
        current.collapsed_len = collapsed_len;
        self.browser.seen_ui = Some(current);

        // F-07/F-18: a collapse or expansion in another window rebuilds here.
        if collapsed_len != seen.collapsed_len {
            let folders: BTreeSet<String> =
                self.shared.ui().collapsed_folders.iter().map(|folder| collapse_key(folder)).collect();
            if folders != self.browser.collapsed {
                self.browser.collapsed = folders;
                self.rebuild_browser_rows();
                changed = true;
            }
        }

        // OI-129: a watch that hit the system's limit says so once.
        if self.browser.watch_error.is_some() {
            if let Some(error) = self.browser.watch_error.clone()
                && is_watch_limit(&error)
                && !self.browser.watch_limit_noticed
            {
                self.notice(lmdv_i18n::t!("msg.watchLimit"));
                self.browser.watch_limit_noticed = true;
                changed = true;
            }
        } else {
            self.browser.watch_limit_noticed = false;
        }
        changed
    }

    /// Sets every tab's outline flag (F-18/P-09's global `toc-hidden`).
    fn set_outline_tabs_hidden(&mut self, hidden: bool) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            if tab.view.outline_hidden != hidden {
                tab.view.outline_hidden = hidden;
                changed = true;
            }
        }
        changed
    }

    /// Writes this window's pane widths to `ui.json` (F-20).
    fn persist_widths(&mut self) {
        let left = self.left.width();
        let right = self.right.width();
        if let Some(seen) = self.browser.seen_ui.as_mut() {
            seen.left_width = left;
            seen.right_width = right;
        }
        let mut ui = self.shared.ui().clone();
        if ui.left_width != left || ui.right_width != right {
            ui.left_width = left;
            ui.right_width = right;
            self.write_ui(ui);
        }
    }

    /// Writes the browser pane's hidden choice to `ui.json` (F-18/P-09).
    ///
    /// The outline's choice is written by [`Self::persist_outline_visibility`]:
    /// it follows the active tab's outline state, which a document without
    /// headings can close without that being a choice.
    fn persist_browser_visibility(&mut self) {
        let left_hidden = !self.left.is_open() && !self.left.is_peeking();
        if let Some(seen) = self.browser.seen_ui.as_mut() {
            seen.left_hidden = left_hidden;
        }
        let mut ui = self.shared.ui().clone();
        if ui.left_hidden != left_hidden {
            ui.left_hidden = left_hidden;
            self.write_ui(ui);
        }
    }

    /// Writes the outline's hidden choice, which is global (F-18/P-09): every
    /// tab takes the same choice, so a new tab cannot pop the pane open.
    pub(crate) fn persist_outline_visibility(&mut self) {
        let hidden = self.active_tab().is_some_and(|tab| tab.view.outline_hidden);
        // The user showed the outline: keep it, as F-19's peek would, until
        // the whole shell fits again. A hide clears both.
        self.browser.fit_override[1] = !hidden;
        self.browser.auto_folded[1] = false;
        for tab in &mut self.tabs {
            tab.view.outline_hidden = hidden;
        }
        if let Some(seen) = self.browser.seen_ui.as_mut() {
            seen.right_hidden = hidden;
        }
        let mut ui = self.shared.ui().clone();
        if ui.right_hidden != hidden {
            ui.right_hidden = hidden;
            self.write_ui(ui);
        }
    }

    /// Whether the document in front is in edit mode (E-02 hides the panes
    /// while editing, and that is not a visibility choice).
    fn is_editing(&self) -> bool {
        self.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor)
    }

    // ---- pane operations (F-18–F-21) ---------------------------------------

    /// `Ctrl+B`, the toolbar's tree button and the palette's toggle (F-18).
    ///
    /// A pane the keyboard opens takes the keyboard; a pointer press leaves the
    /// focus where it was. The choice is written to `ui.json`, so the next
    /// launch restores it. While editing, E-02 keeps both panes hidden: the
    /// command only records the choice.
    pub(crate) fn toggle_browser_pane(&mut self, tx: &mut Tx, keyboard: bool) {
        if self.is_editing() {
            let hidden = !self.shared.ui().left_hidden;
            if let Some(seen) = self.browser.seen_ui.as_mut() {
                seen.left_hidden = hidden;
            }
            let mut ui = self.shared.ui().clone();
            ui.left_hidden = hidden;
            self.write_ui(ui);
            return;
        }
        if self.left.is_peeking() {
            self.left.end_peek();
        } else if self.left.is_open() {
            self.left.set_open(false);
            self.browser.auto_folded[0] = false;
            self.browser.fit_override[0] = false;
        } else {
            // F-19: a pane auto-fit folded opens over the text as a peek; one
            // the user closed docks. Either way auto-fit leaves it alone until
            // the whole shell fits.
            let folded = self.browser.auto_folded[0];
            self.browser.auto_folded[0] = false;
            self.browser.fit_override[0] = true;
            if folded {
                self.left.peek();
            } else {
                self.left.set_open(true);
            }
            self.refresh_browser();
            if keyboard {
                // The pane slides in and its content mounts with it: wait for
                // the focusable element rather than dropping the request on the
                // closed frame (F-18).
                let key = self.browser_pane_focus_key();
                tx.focus_when_shown(key, true);
            }
        }
        self.persist_browser_visibility();
        self.sync_tree_watch();
    }

    /// The pane element a keyboard open should focus (F-18): the tree when it
    /// is drawn, else the search box, else the empty state's Open folder.
    fn browser_pane_focus_key(&self) -> &'static str {
        if !self.browser.search_open && !self.browser.rows.is_empty() {
            BROWSER_TREE_KEY
        } else if self.browser.root.is_some() {
            BROWSER_SEARCH_KEY
        } else {
            "browser-open-folder"
        }
    }

    /// Shows the browser pane because the app is using it (Open folder, folder
    /// search, a folder link); the choice is stored like a toggle's (F-13).
    pub(crate) fn show_browser_pane(&mut self) {
        let folded = self.browser.auto_folded[0];
        if folded {
            self.left.peek();
        } else {
            self.left.set_open(true);
        }
        self.browser.auto_folded[0] = false;
        self.browser.fit_override[0] = true;
        self.persist_browser_visibility();
        self.sync_tree_watch();
    }

    /// F-20's arrow step; `large` is Shift+arrow.
    pub(crate) fn nudge_browser_pane(&mut self, direction: i32, large: bool) {
        let step = if large { PANE_STEP_LARGE } else { PANE_STEP };
        self.left.set_open(true);
        self.left.set_width(self.left.width() + direction as f32 * step);
        self.persist_widths();
    }

    /// F-20's Home: the width the pane was made with.
    pub(crate) fn reset_browser_pane_width(&mut self) {
        self.left.reset_width();
        self.left.set_open(true);
        self.persist_widths();
    }

    /// F-20's End: the widest the pane may get.
    pub(crate) fn maximize_browser_pane(&mut self) {
        let max = self.left.max_width();
        self.left.set_width(max);
        self.left.set_open(true);
        self.persist_widths();
    }

    /// The outline's arrow step (F-20).
    pub(crate) fn nudge_outline_pane(&mut self, direction: i32, large: bool) {
        let step = if large { PANE_STEP_LARGE } else { PANE_STEP };
        self.right.set_open(true);
        self.right.set_width(self.right.width() + direction as f32 * step);
        self.persist_widths();
    }

    /// The outline's Home (F-20).
    pub(crate) fn reset_outline_pane_width(&mut self) {
        self.right.reset_width();
        self.right.set_open(true);
        self.persist_widths();
    }

    /// The outline's End (F-20).
    pub(crate) fn maximize_outline_pane(&mut self) {
        let max = self.right.max_width();
        self.right.set_width(max);
        self.right.set_open(true);
        self.persist_widths();
    }

    /// `Ctrl`+wheel or a pinch over the file browser changes the side panels'
    /// text size (F-21), never the reader's. One step per gesture notch.
    pub(crate) fn pane_magnify(&mut self, delta: f32) -> bool {
        if !delta.is_finite() || delta == 0.0 {
            return true;
        }
        self.browser.magnify += delta;
        if self.browser.magnify.abs() < 0.35 {
            return true;
        }
        let direction = if self.browser.magnify > 0.0 { 1 } else { -1 };
        self.browser.magnify = 0.0;
        let sizes = crate::settings::SIDE_SIZES;
        let current = self.shared.prefs().reader.side_size;
        let index = sizes.iter().position(|size| *size == current).unwrap_or(1) as i32 + direction;
        if let Some(size) = usize::try_from(index).ok().and_then(|index| sizes.get(index)).copied() {
            self.set_side_size(size);
        }
        true
    }

    /// F-19: folds the outline, then the files, when the article would fall
    /// below [`ARTICLE_MIN_WIDTH`], and reopens only what it folded once the
    /// shell fits again. The stored hidden choice always wins.
    ///
    /// The width is the last drawn `kit-root` (the whole content), so opening a
    /// pane never makes the next frame believe there is less room than there is.
    pub(crate) fn apply_pane_fit(&mut self) -> bool {
        if self.is_editing() {
            return false;
        }
        let width = self.browser.shell_width.get();
        if !width.is_finite() || width <= 0.0 {
            return false;
        }
        let left_width = self.left.width();
        let right_width = self.right.width();
        let room = |left: bool, right: bool| {
            let panes = if left { left_width } else { 0.0 } + if right { right_width } else { 0.0 };
            width - panes - PANE_GUTTER >= ARTICLE_MIN_WIDTH
        };
        let mut changed = false;
        if self.right.is_open() && !room(self.left.is_open(), true) && !self.browser.fit_override[1] {
            self.browser.auto_folded[1] = true;
            // `sync_outline` would reopen a tab whose flag still says shown, so
            // the fold sets the flag too; it is a display state here, and only
            // an explicit toggle writes `toc-hidden`.
            self.set_outline_tabs_hidden(true);
            self.right.set_open(false);
            changed = true;
        }
        if self.left.is_open() && !room(true, self.right.is_open()) && !self.browser.fit_override[0] {
            self.browser.auto_folded[0] = true;
            self.left.set_open(false);
            changed = true;
        }
        if self.browser.auto_folded[1] && !self.right.is_open() && room(self.left.is_open(), true) {
            self.browser.auto_folded[1] = false;
            self.set_outline_tabs_hidden(false);
            self.right.set_open(true);
            changed = true;
        }
        if self.browser.auto_folded[0] && !self.left.is_open() && room(true, self.right.is_open()) {
            self.browser.auto_folded[0] = false;
            self.left.set_open(true);
            changed = true;
        }
        if room(true, true) {
            self.browser.fit_override = [false, false];
        }
        changed
    }

    /// Rebuilds the visible rows from the last listing and the collapsed set.
    fn rebuild_browser_rows(&mut self) {
        let Some(listing) = self.browser.listing.as_ref() else {
            self.browser.rows.clear();
            return;
        };
        self.browser.rows = visible_rows(listing, &self.browser.collapsed);
        self.browser.row_epoch += 1;
    }

    /// Applies one listing result (F-08, F-09). Returns whether the rows changed.
    pub(crate) fn finish_tree_listing(
        &mut self,
        generation: u64,
        root: PathBuf,
        result: Result<TreeListing, lmdv_io::TreeError>,
    ) -> bool {
        // The generation identifies the request: the job returns the canonical
        // root, which may differ from the lexical root the pane stores (a
        // symlinked folder), so comparing the two would drop every listing.
        if generation != self.browser.tree_generation {
            return false;
        }
        self.browser.tree_pending = false;
        match result {
            Ok(listing) => {
                self.browser.root_error = None;
                self.browser.root_name = listing.name.clone();
                self.browser.canonical_root = Some(listing.root.clone());
                self.browser.link_revert = None;
                if self.browser.listing.as_ref() == Some(&listing) {
                    // F-08: same listing, no rebuild: scroll and focus survive.
                    return false;
                }
                let focus = self.browser.focus.clone();
                self.browser.listing = Some(listing);
                self.rebuild_browser_rows();
                self.restore_browser_focus(focus);
                true
            }
            Err(error) => {
                if let Some(previous) = self.browser.link_revert.take() {
                    let name = root.file_name().unwrap_or(root.as_os_str()).to_string_lossy().into_owned();
                    let message = match &error {
                        lmdv_io::TreeError::NotADirectory(_) => lmdv_i18n::t!("msg.notMarkdown", "name" => name),
                        failure if failure.is_not_found() => lmdv_i18n::t!("msg.notFound", "name" => name),
                        failure => lmdv_i18n::t!("msg.openFailed", "name" => name, "err" => failure),
                    };
                    self.notice(message);
                    match previous {
                        Some(previous) => self.adopt_browser_root(previous, true),
                        None => self.forget_browser_root(),
                    }
                    return true;
                }
                self.browser.listing = None;
                self.browser.rows.clear();
                self.browser.root_error = Some(error.to_string());
                self.browser.row_epoch += 1;
                if !error.is_not_found() {
                    self.notice(lmdv_i18n::t!("msg.listFolderFailed", "err" => error));
                }
                true
            }
        }
    }

    /// Restores the focused row after a rebuild, without scrolling (F-09).
    fn restore_browser_focus(&mut self, previous: Option<String>) {
        let mut target = previous.or_else(|| self.browser.active.clone());
        while let Some(path) = target {
            if self.browser.has_row(&path) {
                self.browser.focus = Some(path);
                return;
            }
            target = Path::new(&path).parent().map(|parent| parent.to_string_lossy().into_owned());
        }
        self.browser.focus = self.browser.rows.first().map(|row| row.path.clone());
    }

    /// Follows the active document: highlights it and opens its ancestors
    /// (F-07, F-10). Returns whether anything changed.
    pub(crate) fn follow_browser_active(&mut self) -> bool {
        let path = self
            .active_tab()
            .and_then(|tab| tab.document())
            .and_then(|doc| doc.key.borrow().path().map(|path| path.to_string_lossy().into_owned()));
        if path == self.browser.last_active {
            return false;
        }
        self.browser.last_active = path.clone();
        self.browser.active = path.clone();
        let Some(path) = path else {
            self.browser.row_epoch += 1;
            return true;
        };

        let mut expanded = false;
        if let Some(root) = self.browser.root.clone() {
            let canonical = self.browser.canonical_root.clone();
            let target = PathBuf::from(&path);
            let inside_root = |parent: &Path| {
                (parent.starts_with(&root) && parent != root)
                    || canonical
                        .as_deref()
                        .is_some_and(|canonical| parent.starts_with(canonical) && parent != canonical)
            };
            let ancestors = target.ancestors().skip(1).take_while(|parent| inside_root(parent));
            for ancestor in ancestors {
                if self.browser.collapsed.remove(&collapse_key(ancestor)) {
                    expanded = true;
                }
            }
        }
        if expanded {
            self.persist_collapsed();
            self.rebuild_browser_rows();
        }
        if self.browser.has_row(&path) {
            self.browser.focus = Some(path);
        }
        self.browser.row_epoch += 1;
        true
    }

    /// The browser's frame work: follow the active document, run a due search,
    /// and settle async work inline in a headless gate.
    pub(crate) fn poll_browser(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = self.sync_browser_ui();
        changed |= self.apply_pane_fit();
        self.sync_tree_watch();
        changed |= self.follow_browser_active();
        // A menu whose pane (and row) is gone has nothing to act on.
        if self.browser.menu.is_some() && !self.left.is_open() {
            self.browser.menu = None;
            changed = true;
        }
        // A scene opens a menu outside any handler; its first row takes focus
        // here so the arrows and A-08's Tab work from the first frame.
        if self.browser.menu.as_ref().is_some_and(|menu| menu.needs_focus) {
            let ring = self.browser.menu.as_ref().is_some_and(BrowserMenu::is_keyboard);
            self.focus_browser_menu(tx, ring);
            changed = true;
        }
        if let Some(due) = self.browser.search_due
            && now >= due
        {
            self.browser.search_due = None;
            self.start_browser_search();
            changed = true;
        }
        if self.shared.is_headless() {
            if self.browser.tree_pending {
                let (Some(root), generation) = (self.browser.root.clone(), self.browser.tree_generation) else {
                    return changed;
                };
                let result = lmdv_io::tree::list_tree(&root);
                changed |= self.finish_tree_listing(generation, root, result);
            }
            if self.browser.search_pending {
                let generation = self.browser.search_generation;
                let root = self.browser.root.clone();
                let query = self.browser.search_query.clone();
                if let Some(root) = root {
                    let result = lmdv_io::tree::search_tree(&root, &query);
                    changed |= self.finish_search(generation, root, query, result);
                }
            }
        }
        let _ = tx;
        changed
    }

    /// Starts the debounced search on a worker (or marks it for the headless
    /// inline pass).
    fn start_browser_search(&mut self) {
        let Some(root) = self.browser.root.clone() else { return };
        let query = self.browser.search_query.clone();
        if query.is_empty() {
            self.clear_browser_search();
            return;
        }
        self.browser.search_generation = self.browser.search_generation.wrapping_add(1);
        let generation = self.browser.search_generation;
        self.browser.search_pending = true;
        if self.shared.is_headless() {
            let result = lmdv_io::tree::search_tree(&root, &query);
            let _ = self.finish_search(generation, root, query, result);
            return;
        }
        self.shared.jobs().search_tree(self.worker().clone(), root, query, generation);
    }

    /// A search field change: schedules the 250 ms debounce (F-13).
    pub(crate) fn browser_search_changed(&mut self, now: f64, tx: &mut Tx) {
        self.browser.search_query = self.browser_search.text.trim().to_owned();
        self.browser.search_generation = self.browser.search_generation.wrapping_add(1);
        self.browser.search_pending = false;
        if self.browser.search_query.is_empty() {
            self.close_browser_search();
            return;
        }
        let due = now + SEARCH_DEBOUNCE;
        self.browser.search_due = Some(due);
        tx.wake_at(due);
    }

    /// Applies one search result (grouped by file, F-15).
    pub(crate) fn finish_search(
        &mut self,
        generation: u64,
        _root: PathBuf,
        query: String,
        result: Result<Vec<SearchHit>, lmdv_io::TreeError>,
    ) -> bool {
        if generation != self.browser.search_generation || self.browser.search_query != query {
            return false;
        }
        self.browser.search_pending = false;
        let hits = result.unwrap_or_default();
        self.browser.results = group_hits(hits);
        self.browser.search_open = true;
        self.browser.search_selected = (!self.browser.results.is_empty()).then_some(0);
        true
    }

    /// Clears the query, the results and the tree's search mode (F-17).
    pub(crate) fn clear_browser_search(&mut self) {
        self.browser_search.replace_text("");
        self.close_browser_search();
    }

    fn close_browser_search(&mut self) {
        self.browser.search_query.clear();
        self.browser.search_generation = self.browser.search_generation.wrapping_add(1);
        self.browser.search_pending = false;
        self.browser.search_due = None;
        self.browser.search_open = false;
        self.browser.results.clear();
        self.browser.search_selected = None;
    }

    /// Up/Down/Enter/Escape typed into the search field (F-13/F-15).
    pub(crate) fn browser_search_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        match event.key {
            Key::Down => {
                if !self.browser.search_open || self.browser.results.is_empty() {
                    return false;
                }
                let next = self
                    .browser
                    .search_selected
                    .map_or(0, |index| (index + 1).min(self.browser.results.len().saturating_sub(1)));
                self.browser.search_selected = Some(next);
                true
            }
            Key::Up => {
                if !self.browser.search_open || self.browser.results.is_empty() {
                    return false;
                }
                let next = self.browser.search_selected.map_or(0, |index| index.saturating_sub(1));
                self.browser.search_selected = Some(next);
                true
            }
            Key::Enter => {
                let Some(index) = self.browser.search_selected else { return false };
                self.open_search_row(index, event.mods, tx);
                true
            }
            Key::Escape => {
                self.clear_browser_search();
                tx.blur();
                true
            }
            _ => false,
        }
    }

    /// Opens one grouped result: a file header at the top, a line hit at its
    /// line (F-15/F-16).
    pub(crate) fn open_search_row(&mut self, index: usize, mods: Mods, tx: &mut Tx) {
        let Some(row) = self.browser.results.get(index).cloned() else { return };
        let line = (!row.is_file()).then_some(row.line as usize);
        let query = self.browser.search_query.clone();
        let carried = (!query.is_empty()).then_some(query.as_str());
        self.open_browser_file_with_query(row.path, line, mods, carried, tx);
    }

    /// The tree's keyboard: arrows, Home/End, Left/Right, Return/Space
    /// (F-11), plus F-20's resize steps while the tree has focus: `Ctrl+←`/`→`
    /// 16 points, with Shift 64, `Ctrl+Home` the default width and `Ctrl+End`
    /// the maximum. Plain arrows stay the tree's, since its rows use them.
    pub(crate) fn browser_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if event.mods.ctrl && !event.mods.alt && !event.mods.logo && !event.mods.shift {
            match event.key {
                Key::Home => {
                    self.reset_browser_pane_width();
                    return true;
                }
                Key::End => {
                    self.maximize_browser_pane();
                    return true;
                }
                _ => {}
            }
        }
        if event.mods.ctrl && !event.mods.alt && !event.mods.logo {
            match event.key {
                Key::Left => {
                    self.nudge_browser_pane(-1, event.mods.shift);
                    return true;
                }
                Key::Right => {
                    self.nudge_browser_pane(1, event.mods.shift);
                    return true;
                }
                _ => {}
            }
        }
        if event.mods.alt || event.mods.logo || (event.mods.ctrl && !matches!(&event.key, Key::Enter)) {
            return false;
        }
        let len = self.browser.rows.len();
        if len == 0 {
            return false;
        }
        // Tab lands on the tree with the active file as its roving row (F-10),
        // so arrows and Return start there before the first key sets a focus.
        // With neither, Down takes the first visible row and Up the last: the
        // implicit first row is not a selection to step from.
        let at = self
            .browser
            .focus
            .clone()
            .filter(|path| self.browser.has_row(path))
            .or_else(|| self.browser.active.clone().filter(|path| self.browser.has_row(path)))
            .and_then(|path| self.browser.rows.iter().position(|row| row.path == path));
        let page = 10usize;
        let target = match &event.key {
            Key::Up => Some(at.map_or(len - 1, |index| index.saturating_sub(1))),
            Key::Down => Some(at.map_or(0, |index| (index + 1).min(len - 1))),
            Key::Home => Some(0),
            Key::End => Some(len - 1),
            Key::PageUp => Some(at.map_or(0, |index| index.saturating_sub(page))),
            Key::PageDown => Some(at.map_or(0, |index| (index + page).min(len - 1))),
            Key::Right => {
                let Some(index) = at else { return self.set_browser_focus(0, tx) };
                let row = &self.browser.rows[index];
                if !row.dir {
                    return true;
                }
                if !row.expanded {
                    self.toggle_browser_folder(&row.path.clone());
                    return true;
                }
                let first_child = index + 1;
                if first_child < len && self.browser.rows[first_child].depth == row.depth + 1 {
                    return self.set_browser_focus(first_child, tx);
                }
                return true;
            }
            Key::Left => {
                let Some(index) = at else { return true };
                let row = &self.browser.rows[index];
                if row.dir && row.expanded {
                    self.toggle_browser_folder(&row.path.clone());
                    return true;
                }
                if row.depth == 0 {
                    return true;
                }
                let parent_depth = row.depth - 1;
                let parent = (0..index).rev().find(|&earlier| self.browser.rows[earlier].depth == parent_depth);
                return match parent {
                    Some(parent) => self.set_browser_focus(parent, tx),
                    None => true,
                };
            }
            Key::Enter => {
                if tx.key_repeat() {
                    return true;
                }
                let Some(index) = at else { return true };
                self.activate_browser_row(index, event.mods, tx);
                return true;
            }
            Key::Text(typed) if typed == " " => {
                if tx.key_repeat() {
                    return true;
                }
                let Some(index) = at else { return true };
                self.activate_browser_row(index, event.mods, tx);
                return true;
            }
            _ => None,
        };
        match target {
            Some(index) => self.set_browser_focus(index, tx),
            None => false,
        }
    }

    fn set_browser_focus(&mut self, index: usize, tx: &mut Tx) -> bool {
        let Some(row) = self.browser.rows.get(index) else { return false };
        let path = row.path.clone();
        self.browser.focus = Some(path.clone());
        self.browser.row_epoch += 1;
        tx.scroll_into_view_in(BROWSER_TREE_KEY, path);
        true
    }

    fn activate_browser_row(&mut self, index: usize, mods: Mods, tx: &mut Tx) {
        let Some(row) = self.browser.rows.get(index).cloned() else { return };
        self.browser.focus = Some(row.path.clone());
        if row.dir {
            self.toggle_browser_folder(&row.path);
        } else {
            self.open_browser_file(PathBuf::from(row.path), None, mods, tx);
        }
    }

    /// The palette's “Search the folder for …” (C-07): shows the pane, fills the
    /// field, starts the search at once and puts the keyboard in it.
    pub(crate) fn search_folder_for(&mut self, query: &str, tx: &mut Tx) {
        self.show_browser_pane();
        self.browser_search.replace_text(query.to_owned());
        self.browser.search_query = query.trim().to_owned();
        if self.browser.search_query.is_empty() {
            self.clear_browser_search();
            return;
        }
        self.start_browser_search();
        let length = self.browser_search.text.len();
        self.browser_search.set_selection(0, length);
        tx.focus(BROWSER_SEARCH_KEY, false);
    }

    /// A row press: a folder opens or closes, a file opens in a tab (F-01).
    pub(crate) fn browser_row_pressed(&mut self, path: &str, dir: bool, mods: Mods, tx: &mut Tx) {
        self.left.end_peek();
        self.browser.focus = Some(path.to_owned());
        self.browser.row_epoch += 1;
        if dir {
            self.toggle_browser_folder(path);
        } else {
            self.open_browser_file(PathBuf::from(path), None, mods, tx);
        }
    }

    /// Opens the F-12 context menu for a file or folder at a pointer point.
    pub(crate) fn show_browser_menu(&mut self, path: PathBuf, dir: bool, point: PointerPress) {
        let name = browser_row_name(&path);
        let focused = path.to_string_lossy().into_owned();
        if self.browser.has_row(&focused) {
            self.browser.focus = Some(focused);
            self.browser.row_epoch += 1;
        }
        self.browser.menu = Some(BrowserMenu {
            path,
            name,
            dir,
            point: std::cell::Cell::new(point),
            from_keyboard: false,
            needs_focus: true,
        });
    }

    /// Shift+F10 or the Menu key on the tree: the focused row's menu, which
    /// the view anchors under that row once its rect is known (F-12).
    pub(crate) fn show_browser_menu_keyboard(&mut self) {
        let Some(path) = self.browser.focus.clone().or_else(|| self.browser.active_descendant()) else { return };
        let Some(row) = self.browser.rows.iter().find(|row| row.path == path).cloned() else { return };
        self.browser.focus = Some(path.clone());
        self.browser.row_epoch += 1;
        self.browser.menu = Some(BrowserMenu {
            path: PathBuf::from(path),
            name: row.name,
            dir: row.dir,
            point: std::cell::Cell::new(PointerPress { x: BROWSER_MENU_INSET, y: BROWSER_MENU_GAP }),
            from_keyboard: true,
            needs_focus: true,
        });
    }

    /// [`Self::show_browser_menu_keyboard`] for a named row before its listing
    /// has landed (the headless `browser-menu` scene draws one frame after the
    /// scene runs); the view still anchors it under the drawn row.
    pub(crate) fn show_browser_menu_keyboard_for(&mut self, path: PathBuf, dir: bool) {
        let name = browser_row_name(&path);
        let focused = path.to_string_lossy().into_owned();
        if self.browser.has_row(&focused) {
            self.browser.focus = Some(focused);
            self.browser.row_epoch += 1;
        }
        self.browser.menu = Some(BrowserMenu {
            path,
            name,
            dir,
            point: std::cell::Cell::new(PointerPress { x: BROWSER_MENU_INSET, y: BROWSER_MENU_GAP }),
            from_keyboard: true,
            needs_focus: true,
        });
    }

    /// The open context menu, for the view.
    pub fn browser_menu(&self) -> Option<&BrowserMenu> {
        self.browser.menu.as_ref()
    }

    /// Closes the context menu (Escape, Tab, an outside press, a choice, a new root).
    pub fn close_browser_menu(&mut self) {
        self.browser.menu = None;
    }

    /// Focuses the menu's first enabled row after it opens (A-08: the arrows
    /// work at once and Tab/Escape close from inside). `ring` shows the
    /// keyboard highlight at once for a keyboard opening.
    pub(crate) fn focus_browser_menu(&mut self, tx: &mut Tx, ring: bool) {
        let Some(menu) = self.browser.menu.as_mut() else { return };
        menu.needs_focus = false;
        if let Some(key) = browser_menu_first_item(&browser_menu_entries(menu)) {
            tx.focus(key, ring);
        }
    }

    /// Runs one F-12 row on the menu's row.
    pub(crate) fn run_browser_menu_action(&mut self, action: BrowserMenuAction, tx: &mut Tx) {
        let Some(menu) = self.browser.menu.take() else { return };
        match action {
            // F-12's Open in new tab is the original's background open: the tab
            // joins the strip without taking the front (F-11's Ctrl+click).
            BrowserMenuAction::OpenTab => {
                self.open_browser_file(menu.path, None, Mods { ctrl: true, ..Mods::default() }, tx);
            }
            BrowserMenuAction::OpenWindow => self.open_path_in_new_window(menu.path, None),
            // The original reveals the item in its folder, so the desktop
            // handler opens the parent (a user-initiated job, never in the frame).
            BrowserMenuAction::Reveal => {
                if let Some(parent) = menu.path.parent() {
                    self.shared.open_external(self.worker().clone(), parent.to_path_buf());
                }
            }
            BrowserMenuAction::CopyPath => tx.copy(menu.path.to_string_lossy().into_owned()),
            BrowserMenuAction::CopyLink => {
                let root = self.active_document_root();
                tx.copy(markdown_link(root.as_deref(), &menu.path, &menu.name));
            }
        }
    }

    /// Opens the folder dialog (`file.open-folder`). A chosen folder roots the
    /// pane and is shown (F-01).
    pub(crate) fn open_folder_dialog(&mut self, tx: &mut Tx) {
        let options = FileDialogOptions {
            title: lmdv_i18n::t!("dialog.openFolder").to_owned(),
            directory: self.browser.root.clone(),
            ..FileDialogOptions::default()
        };
        tx.open_folder(options, |state: &mut WindowState, result, _| match result {
            Ok(Some(path)) => {
                state.show_browser_pane();
                state.set_browser_root(path);
            }
            Ok(None) => {}
            Err(error) => state.notice(lmdv_i18n::t!("msg.folderDialogFailed", "err" => error)),
        });
    }

    /// `Ctrl+Shift+F`: show the pane, focus the search box and search the
    /// selection when there is one (F-13).
    pub(crate) fn open_folder_search(&mut self, tx: &mut Tx) {
        self.show_browser_pane();
        let selected = tx.text_selection().map(|selection| search_prefill(selection.text())).unwrap_or_default();
        if !selected.is_empty() {
            self.browser_search.replace_text(selected.clone());
            self.browser.search_query = selected;
            let length = self.browser_search.text.len();
            self.browser_search.set_selection(0, length);
            self.start_browser_search();
        } else {
            let length = self.browser_search.text.len();
            self.browser_search.set_selection(0, length);
        }
        tx.focus(BROWSER_SEARCH_KEY, false);
    }

    /// A folder link (`[x](nested/)`, L-05): roots the pane there, or returns to
    /// the previous root when the target is not a folder.
    pub(crate) fn open_folder_link(&mut self, folder: PathBuf) {
        let previous = self.browser.root.clone();
        self.show_browser_pane();
        self.adopt_browser_root(folder, true);
        self.browser.link_revert = Some(previous);
    }
}

/// The row name a path shows, for a menu opened before its listing landed.
fn browser_row_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

/// The visible rows for a listing and collapsed set.
fn visible_rows(listing: &TreeListing, collapsed: &BTreeSet<String>) -> Vec<BrowserRow> {
    let mut rows = Vec::new();
    let mut open: Vec<bool> = Vec::new();
    for entry in &listing.rows {
        let depth = entry.depth as usize;
        open.truncate(depth);
        let parent_open = open.iter().all(|open| *open);
        let expanded = !collapsed.contains(&collapse_key(&entry.path));
        if entry.dir {
            open.push(expanded);
        }
        if parent_open {
            rows.push(BrowserRow {
                path: entry.path.to_string_lossy().into_owned(),
                name: entry.name.clone(),
                depth: entry.depth,
                dir: entry.dir,
                expanded: entry.dir && expanded,
            });
        }
    }
    rows
}

/// Groups line hits under one file header each (F-15).
fn group_hits(hits: Vec<SearchHit>) -> Vec<SearchRow> {
    let mut rows = Vec::new();
    let mut current: Option<PathBuf> = None;
    for hit in hits {
        if current.as_deref() != Some(hit.path.as_path()) {
            rows.push(SearchRow { path: hit.path.clone(), name: hit.name.clone(), line: 0, preview: String::new() });
            current = Some(hit.path.clone());
        }
        if hit.line > 0 {
            rows.push(SearchRow { path: hit.path, name: hit.name, line: hit.line, preview: hit.preview });
        }
    }
    rows
}

/* ---- the pane ---- */

/// The file-browser pane: folder header, search box, tree or results.
pub(crate) fn pane(state: &WindowState, cx: &Cx<WindowState>, kit: &Kit) -> El {
    // F-19: remember the whole content's width for the next poll's auto-fold.
    // The value is the last drawn frame's, so a pane that opens never makes the
    // fit believe the article has less room than it does.
    if let Some([width, _]) = cx.layout_size("kit-root")
        && width > 0.0
    {
        state.browser.shell_width.set(width);
    }
    if !state.left.is_open() && !state.left.is_peeking() {
        // The closed pane is clipped; building a 2,000-row tree every frame
        // would be wasted work. A peek is visible, so it must build the real
        // pane; opening it also builds it on the next frame.
        return Sidebar::new().label(lmdv_i18n::t!("pane.files")).build(kit);
    }
    // F-12: a keyboard menu follows the focused row's rect, which the layout
    // knows after the row's first frame. Only read while the menu is open, so a
    // scrolling tree never re-runs this view for it.
    if let Some(menu) = state.browser.menu.as_ref().filter(|menu| menu.is_keyboard())
        && let Some(path) = state.browser.focused()
        && let Some(rect) = cx.window_layout(path)
    {
        menu.follow_row(rect);
    }
    let c = &kit.c;
    let root = state.browser.root();
    let name = state.browser.root_name().to_owned();
    let up = IconButton::new(&icons::UP)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(rune_kit::ControlSize::Small)
        .disabled(!state.browser_can_go_up())
        .label(lmdv_i18n::t!("pane.up"))
        .help(lmdv_i18n::t!("browser.upHint"))
        .on_press(|state: &mut WindowState| {
            state.browser_up();
        })
        .build(kit)
        .key("browser-up");
    let refresh = IconButton::new(&icons::REFRESH)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(rune_kit::ControlSize::Small)
        .disabled(root.is_none())
        .label(lmdv_i18n::t!("browser.refresh"))
        .help(lmdv_i18n::t!("browser.refreshHint"))
        .on_press(|state: &mut WindowState| state.refresh_browser())
        .build(kit)
        .key("browser-refresh");
    let header = row()
        .key("browser-header")
        .w_full()
        .h(36.0)
        .px(10.0)
        .gap(6.0)
        .center_y()
        .child(icon(&icons::FOLDER, 15.0).color(c.icon_muted))
        .child(text(name.clone()).font(kit.ty.ui_medium).color(c.text_base).truncate().grow(1.0))
        .child(up)
        .child(refresh);

    let mut search = TextInput::new(&state.browser_search)
        .placeholder(lmdv_i18n::t!("browser.search"))
        .label(lmdv_i18n::t!("browser.search"))
        .width(None)
        .plain()
        .disabled(root.is_none())
        .on_edit(|state: &mut WindowState| &mut state.browser_search)
        .on_change(|state: &mut WindowState, tx: &mut Tx| state.browser_search_changed(tx.clock(), tx))
        .on_key(|state: &mut WindowState, event, tx| state.browser_search_key(event, tx))
        .on_action(|state: &mut WindowState, _: &mut Tx| state.clear_browser_search())
        .active_descendant(
            state.browser.search_selected.filter(|index| *index < state.browser.results.len()).map(result_key),
        );
    if !state.browser_search.text.is_empty() {
        search = search.clear_button();
    }
    let search = search.build(kit).key(BROWSER_SEARCH_KEY);

    let body = if root.is_none() {
        empty_state(
            kit,
            lmdv_i18n::t!("browser.noFolder"),
            lmdv_i18n::t!("browser.noFolderDetail"),
            Some(
                Button::new(lmdv_i18n::t!("browser.openFolder"))
                    .variant(ButtonVariant::Outline)
                    .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
                        state.run_command("file.open-folder", tx);
                    })
                    .build(kit)
                    .key("browser-open-folder"),
            ),
        )
    } else if state.browser.is_searching() {
        results(state, kit)
    } else if state.browser.rows().is_empty() && state.browser.tree_pending {
        empty_state(kit, lmdv_i18n::t!("msg.loading"), lmdv_i18n::t!("browser.loadingDetail"), None)
    } else if state.browser.rows().is_empty() {
        let detail =
            state.browser.root_error.clone().unwrap_or_else(|| lmdv_i18n::t!("browser.noMarkdownDetail").to_owned());
        empty_state(kit, lmdv_i18n::t!("browser.noMarkdown"), &detail, None)
    } else {
        tree(state, kit)
    };

    col()
        .key("browser-pane")
        .w_full()
        .h_full()
        .min_h(0.0)
        // F-21: Ctrl+wheel (a pinch on Linux) over the pane changes the side
        // panels' text size, never the reader's. The outline and the article
        // need the same handler in their own views.
        .on_magnify(|state: &mut WindowState, event, _: &mut Tx| state.pane_magnify(event.delta))
        .child(header)
        .child(el().key("browser-separator").w_full().h(1.0).fill(c.border_muted))
        .child(row().key("browser-search-row").w_full().px(8.0).py(6.0).child(search))
        .child(body)
}

fn empty_state(kit: &Kit, title: &str, detail: &str, action: Option<El>) -> El {
    let mut state = col()
        .key("browser-empty")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .px(16.0)
        .pt(48.0)
        .gap(6.0)
        .child(text(title).font(kit.ty.ui_medium).color(kit.c.text_muted).w_full())
        .child(text(detail).font(kit.ty.small).color(kit.c.text_faint).wrap().w_full());
    if let Some(action) = action {
        state = state.child(action);
    }
    state
}

fn tree(state: &WindowState, kit: &Kit) -> El {
    let count = state.browser.rows().len();
    let kit_for_rows = kit.clone();
    let rows = col()
        .key("browser-rows")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows_version(state.browser.row_epoch)
        .virtual_rows(
            count,
            BROWSER_ROW_HEIGHT,
            |state: &WindowState, index| {
                std::borrow::Cow::Borrowed(state.browser.rows().get(index).map_or("", |row| row.path.as_str()))
            },
            move |state: &WindowState, index| browser_row(state, index, &kit_for_rows),
        );
    let viewport = col()
        .key("browser-viewport")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(rows);
    let mut root = col()
        .key(BROWSER_TREE_KEY)
        .role("tree")
        .label(lmdv_i18n::t!("pane.files"))
        .focusable(true)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .on_key(|state: &mut WindowState, event, tx| state.browser_key(event, tx))
        // F-12 from the keyboard: Shift+F10 or the Menu key. Rune reports it
        // as a secondary press at the focused element; the view then puts the
        // menu under the focused row. A right-click outside a row opens none.
        .on_secondary_press(|state: &mut WindowState, _, tx| {
            if tx.by_keys() {
                state.show_browser_menu_keyboard();
                state.focus_browser_menu(tx, true);
            }
        })
        .child(viewport);
    if let Some(id) = state.browser.active_descendant() {
        root = root.active_descendant(id);
    }
    root
}

fn browser_row(state: &WindowState, index: usize, kit: &Kit) -> El {
    let Some(row) = state.browser.rows().get(index) else { return el() };
    let focused = state.browser.focused() == Some(row.path.as_str());
    let active = state.browser.active() == Some(row.path.as_str());
    let open = row.dir.then_some(row.expanded);
    let mut style = TreeStyle::from_kit(kit).chevron_only_folders(true);
    style.row_height = BROWSER_ROW_HEIGHT;
    let mut item = TreeItem::new(row.name.clone()).icon(if row.dir {
        if row.expanded { &rune_kit::glyphs::FOLDER_OPEN } else { &rune_kit::glyphs::FOLDER }
    } else {
        file_icon(&row.name)
    });
    if active {
        // The open document keeps the accent even while the keyboard is on
        // another row (F-01's highlight).
        item = item.label_color(kit.c.text_accent).icon_color(kit.c.text_accent).dot(kit.c.text_accent);
    }
    let tree_row = TreeRow::new(&row.path, row.depth).expanded(open).selected(focused || active);
    let path = row.path.clone();
    let dir = row.dir;
    let menu_path = row.path.clone();
    item.build(kit, &tree_row, &style)
        .key(row.path.clone())
        .cursor(Cursor::Pointer)
        .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
            tx.focus(BROWSER_TREE_KEY, false);
            let mods = tx.mods();
            state.browser_row_pressed(&path, dir, mods, tx);
        })
        .on_secondary_press(move |state: &mut WindowState, point, tx| {
            state.show_browser_menu(PathBuf::from(&menu_path), dir, point);
            state.focus_browser_menu(tx, false);
        })
}

fn results(state: &WindowState, kit: &Kit) -> El {
    let c = &kit.c;
    let mut list =
        col().key("browser-results").w_full().grow(1.0).min_h(0.0).gap(1.0).scroll(ScrollSpec { stick_bottom: false });
    if state.browser.results().is_empty() {
        return list.child(
            text(lmdv_i18n::t!("pane.noMatches"))
                .key("browser-no-matches")
                .font(kit.ty.small)
                .color(c.text_faint)
                .px(16.0)
                .pt(24.0),
        );
    }
    for (index, hit) in state.browser.results().iter().enumerate() {
        let selected = state.browser.selected_result() == Some(index);
        let key = result_key(index);
        let row_index = index;
        let mut line = row()
            .key(key)
            .role("button")
            .label(if hit.is_file() {
                hit.name.clone()
            } else {
                lmdv_i18n::t!("browser.resultLine", "n" => hit.line, "preview" => hit.preview.clone())
            })
            .w_full()
            .h(26.0)
            .px(10.0)
            .gap(6.0)
            .center_y()
            .radius(kit.radius.sm)
            .focusable(false)
            .focus_selected(selected)
            .cursor(Cursor::Pointer)
            .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
                let mods = tx.mods();
                state.open_search_row(row_index, mods, tx);
            });
        line = if selected { line.fill(c.pressed) } else { line.hover(c.hover) };
        if hit.is_file() {
            line = line.child(icon(file_icon(&hit.name), 14.0).color(c.icon_muted)).child(
                highlight(&hit.name, &state.browser.search_query, kit)
                    .font(kit.ty.ui_medium)
                    .color(c.text_base)
                    .truncate()
                    .grow(1.0),
            );
        } else {
            line = line
                .pl(24.0)
                .child(
                    text(lmdv_i18n::t!("pane.lineN", "n" => hit.line))
                        .font(kit.ty.caption())
                        .color(c.text_faint)
                        .shrink(),
                )
                .child(
                    highlight(&hit.preview, &state.browser.search_query, kit)
                        .font(kit.ty.small)
                        .color(c.text_muted)
                        .truncate()
                        .grow(1.0),
                );
        }
        list = list.child(line);
    }
    list
}

fn result_key(index: usize) -> String {
    format!("browser-result-{index}")
}

/// `text` with the first case-insensitive match of `query` marked, as the
/// original wraps it in `<mark>`.
fn highlight(source: &str, query: &str, kit: &Kit) -> El {
    let Some((start, end)) = find_ci(source, query) else { return text(source.to_owned()) };
    let span = Span { bg: Some(kit.alpha(kit.c.text_accent, 0.22)), bg_pad: 1.0, ..Span::new(start, end) };
    rich(source.to_owned(), vec![span])
}

/// F-12's menu, opened under the row the app recorded.
///
/// It is `rune_kit::Menu`'s surface and rows (`role=menu` with `menuitem`s,
/// Up/Down wrap, Home/End, Enter/Space), but *not* its modal: A-08 closes a
/// menu on Escape **and Tab**, and Rune's modal menus consume Tab as focus
/// traversal first. So the window's root key handler closes it (view.rs), the
/// menu opens with its first enabled row focused, and a transparent layer
/// closes it on an outside press. Focus goes back to the tree on every close.
pub(crate) fn context_menu(state: &WindowState, kit: &Kit, layer: &mut Vec<El>) {
    let Some(menu) = state.browser_menu() else { return };
    // Under the surface: a press anywhere else closes it, as a scrim would.
    layer.push(
        el().key(BROWSER_MENU_SCRIM)
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .a11y_hidden()
            .transition(rune::Transition::none())
            .on_press(|state: &mut WindowState| state.close_browser_menu())
            .on_secondary_press(|state: &mut WindowState, _, _| state.close_browser_menu()),
    );
    let trigger = el().key(BROWSER_MENU_ANCHOR).absolute(0.0, 0.0).size(0.0, 0.0);
    let mut surface = rune_kit::context_menu(BROWSER_MENU_KEY, trigger, Some(menu.point()));
    for entry in browser_menu_entries(menu) {
        if entry.separator_before {
            surface = surface.separator();
        }
        let mut item = MenuItem::new(entry.label).icon(menu_icon(entry.action)).disabled(!entry.enabled);
        if let Some(hint) = entry.hint {
            item = item.shortcut(hint);
        }
        if let Some(description) = entry.description {
            item = item.description(description);
        }
        if entry.enabled {
            let action = entry.action;
            item = item.on_select_tx(move |state: &mut WindowState, tx: &mut Tx| {
                state.run_browser_menu_action(action, tx);
            });
        }
        surface = surface.item(item);
    }
    let trigger = surface.min_width(260.0).build(kit, layer);
    layer.push(trigger);
}

fn menu_icon(action: BrowserMenuAction) -> &'static rune::Icon {
    match action {
        BrowserMenuAction::OpenTab => &icons::TAB,
        BrowserMenuAction::OpenWindow => &icons::WINDOW,
        BrowserMenuAction::Reveal => &icons::FOLDER,
        BrowserMenuAction::CopyPath => &rune_kit::glyphs::COPY,
        BrowserMenuAction::CopyLink => &icons::LINK,
    }
}

/// The byte range of `query` in `source`, ignoring ASCII case.
fn find_ci(source: &str, query: &str) -> Option<(usize, usize)> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let lower = source.to_lowercase();
    let query_lower = query.to_lowercase();
    if lower.len() != source.len() || query_lower.len() != query.len() {
        return source.find(query).map(|start| (start, start + query.len()));
    }
    lower.find(&query_lower).map(|start| (start, start + query_lower.len()))
}
