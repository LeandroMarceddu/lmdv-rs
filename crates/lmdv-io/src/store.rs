//! Versioned JSON state with one coalescing writer thread.
//!
//! The store owns `prefs.json`, `recents.json`, `positions.json`, `ui.json`,
//! `session.json` and one file per draft. `StoreHandle::patch` is non-blocking;
//! the writer keeps at most one write per file per 300 ms and only `flush` on
//! close/quit forces pending values out. Every file is written atomically and
//! carries `schema` + `app`; a corrupt file is renamed to `.bad-<timestamp>`
//! and defaults are used.
//!
//! Loading is explicit and happens on a worker: [`load_recents`],
//! [`load_positions`], [`load_ui`], [`load_session`], [`load_prefs`],
//! [`load_drafts`] and [`load_draft`]. Each `*_reported` variant returns the
//! same value plus a [`StoreIssue`] (a [`DraftIssue`] for the two draft
//! loaders): a corrupt or oversized file was quarantined, while a
//! future-schema or foreign-app file is still on disk. Pass those issues to
//! [`StoreHandle::start_with`] / [`StoreHandle::start_with_drafts`] (or
//! [`StoreHandle::protect_files`] / [`StoreHandle::protect_drafts`] after a
//! late load) so the writer never overwrites settings or drafts it cannot
//! read.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lmdv_core::{
    DRAFT_MAX_CHARS, DRAFT_MAX_ENTRIES, Draft, LEFT_WIDTH_DEFAULT, LEFT_WIDTH_MAX, NavHistory, Prefs,
    RIGHT_WIDTH_DEFAULT, RIGHT_WIDTH_MAX, ScrollAnchor,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::atomic::write_atomic_with;
use crate::dirs::AppDirs;

/// The current state schema.
pub const STORE_SCHEMA: u32 = 1;
/// The value written in every file's `app` field.
pub const APP_NAME: &str = "lmdv";
/// Minimum time between two writes of the same state file.
pub const COALESCE: Duration = Duration::from_millis(300);
/// Maximum recents.
pub const RECENTS_MAX: usize = 12;
/// Maximum reading positions.
pub const POSITIONS_MAX: usize = 100;
/// Maximum closed tabs restored for one window.
pub const CLOSED_TABS_MAX: usize = 25;
/// Maximum command ids remembered by the palette.
pub const PALETTE_RECENT_MAX: usize = 8;

const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;

/// One of the five singleton state files the store owns.
///
/// Drafts are separate: each one has its own key and its own file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StateFile {
    /// `prefs.json`.
    Prefs,
    /// `recents.json`.
    Recents,
    /// `positions.json`.
    Positions,
    /// `ui.json`.
    Ui,
    /// `session.json`.
    Session,
}

/// Why a state file could not be loaded normally.
///
/// Every variant means the loaded value fell back to its default. A corrupt or
/// oversized file is quarantined; a future-schema or foreign-app file is left
/// exactly where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreIssue {
    /// Written by a newer lmdv with a schema this version cannot read.
    FutureSchema {
        /// The file that was ignored.
        file: StateFile,
        /// The schema version found in the file.
        schema: u64,
    },
    /// Written by another application that happens to use the same file name.
    ForeignApp {
        /// The file that was ignored.
        file: StateFile,
        /// The `app` value found in the file.
        app: String,
    },
    /// Larger than the state-size limit, so it was quarantined.
    Oversized {
        /// The file that was quarantined.
        file: StateFile,
    },
    /// Unreadable JSON or payload, so it was quarantined.
    Corrupt {
        /// The file that was quarantined.
        file: StateFile,
    },
}

impl StoreIssue {
    /// The file the issue belongs to.
    pub fn file(&self) -> StateFile {
        match self {
            Self::FutureSchema { file, .. }
            | Self::ForeignApp { file, .. }
            | Self::Oversized { file }
            | Self::Corrupt { file } => *file,
        }
    }

    /// Whether this file must survive the session untouched.
    ///
    /// A newer version's or another application's file is still on disk; this
    /// version loaded defaults, and writing them back would erase settings it
    /// cannot read. Pass the issues to [`StoreHandle::start_with`] (or
    /// [`StoreHandle::protect_files`] for a late load) so the writer skips
    /// them. A quarantine issue is not protected: the original was already
    /// moved aside and a fresh file is the normal outcome.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::FutureSchema { .. } | Self::ForeignApp { .. })
    }
}

/// A state value plus what happened to its file.
///
/// [`LoadedState::value`] is the usable value either way; `issue` is only set
/// when the file was ignored or quarantined.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedState<T> {
    /// The loaded value, or its default when the file was ignored or corrupt.
    pub value: T,
    /// The file problem, if any.
    pub issue: Option<StoreIssue>,
}

/// Why a draft file could not be loaded normally.
///
/// Drafts follow the singleton state files' rules even though each one is its
/// own file: every variant means the draft was not restored. A corrupt or
/// oversized draft is quarantined; a future-schema or foreign-app draft is
/// left exactly where it is and must survive the session untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftIssue {
    /// Written by a newer lmdv with a schema this version cannot read.
    FutureSchema {
        /// The draft file that was ignored.
        file: PathBuf,
        /// The key recorded in the file, when it could still be read.
        key: Option<String>,
        /// The schema version found in the file.
        schema: u64,
    },
    /// Written by another application that happens to use the same folder.
    ForeignApp {
        /// The draft file that was ignored.
        file: PathBuf,
        /// The key recorded in the file, when it could still be read.
        key: Option<String>,
        /// The `app` value found in the file.
        app: String,
    },
    /// Larger than the state-size limit, so it was quarantined.
    Oversized {
        /// The draft file that was quarantined.
        file: PathBuf,
    },
    /// Unreadable JSON or payload, so it was quarantined.
    Corrupt {
        /// The draft file that was quarantined.
        file: PathBuf,
    },
}

impl DraftIssue {
    /// The draft file the issue belongs to.
    pub fn file(&self) -> &Path {
        match self {
            Self::FutureSchema { file, .. }
            | Self::ForeignApp { file, .. }
            | Self::Oversized { file }
            | Self::Corrupt { file } => file,
        }
    }

    /// The draft key recorded in the file, when it was still readable.
    ///
    /// A newer schema may have moved the key, and another application may not
    /// have one at all, so this is a hint for the notice rather than the
    /// protection itself; [`Self::file`] names the file either way.
    pub fn key(&self) -> Option<&str> {
        match self {
            Self::FutureSchema { key, .. } | Self::ForeignApp { key, .. } => key.as_deref(),
            Self::Oversized { .. } | Self::Corrupt { .. } => None,
        }
    }

    /// Whether this file must survive the session untouched.
    ///
    /// A newer version's or another application's draft is still on disk; this
    /// version did not load it, and writing a same-key draft back would erase
    /// what it cannot read. Pass the issues to
    /// [`StoreHandle::start_with_drafts`] (or [`StoreHandle::protect_drafts`]
    /// for a late load) so the writer skips them. A quarantine issue is not
    /// protected: the original was already moved aside.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::FutureSchema { .. } | Self::ForeignApp { .. })
    }
}

/// One loaded draft plus what happened to its file.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedDraft {
    /// The restored draft, or `None` when it was missing, ignored or corrupt.
    pub value: Option<Draft>,
    /// The file problem, if any.
    pub issue: Option<DraftIssue>,
}

/// Every restorable draft plus the files that were ignored or quarantined.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedDrafts {
    /// Drafts newest first, capped at the entry and character limits.
    pub value: Vec<(String, Draft)>,
    /// One entry per draft file that was ignored or quarantined.
    pub issues: Vec<DraftIssue>,
}

/// A recently opened document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Recent {
    /// The document path, spelled as `DocumentKey::File` spells it.
    pub path: PathBuf,
    /// The file name, for display.
    pub name: String,
    /// The parent folder, for display.
    pub dir: PathBuf,
    /// Last open, milliseconds since the epoch.
    pub opened_at: u64,
}

impl Recent {
    /// Builds an entry from a path; the name and folder are derived for display.
    pub fn new(path: impl Into<PathBuf>, opened_at: u64) -> Self {
        let path = path.into();
        let name = path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let dir = path.parent().map_or_else(PathBuf::new, Path::to_path_buf);
        Self { path, name, dir, opened_at }
    }
}

/// One remembered reading position.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Position {
    /// The heading id; empty for a raw offset.
    pub id: String,
    /// How far the anchor sat above the viewport top.
    pub offset: f32,
    /// Last update, milliseconds since the epoch.
    pub at: u64,
}

impl Position {
    /// Captures an anchor for storage.
    pub fn from_anchor(anchor: &ScrollAnchor, at: u64) -> Self {
        let mut anchor = anchor.clone();
        anchor.sanitize();
        Self { id: anchor.id, offset: anchor.offset, at }
    }

    /// Recovers the anchor, sanitizing corrupt offsets.
    pub fn to_anchor(&self) -> ScrollAnchor {
        let mut anchor = ScrollAnchor { id: self.id.clone(), offset: self.offset };
        anchor.sanitize();
        anchor
    }
}

/// Reading positions keyed by the document path.
///
/// JSON object keys are strings, so a non-UTF-8 Linux path uses its lossy
/// UTF-8 spelling; the application's canonical paths are UTF-8 in practice.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PositionTable(BTreeMap<String, Position>);

impl PositionTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// The remembered anchor, or `None` when the document starts at the top.
    pub fn get(&self, path: &Path) -> Option<ScrollAnchor> {
        self.0.get(&path_key(path)).map(Position::to_anchor)
    }

    /// Stores an anchor. An at-top anchor forgets the entry; the table is
    /// capped at [`POSITIONS_MAX`] with the oldest `at` evicted.
    pub fn remember(&mut self, path: &Path, anchor: &ScrollAnchor, at: u64) {
        let key = path_key(path);
        let mut anchor = anchor.clone();
        anchor.sanitize();
        if anchor.is_top() {
            self.0.remove(&key);
            return;
        }
        self.0.insert(key, Position::from_anchor(&anchor, at));
        self.normalize();
    }

    /// Forgets one document.
    pub fn forget(&mut self, path: &Path) {
        self.0.remove(&path_key(path));
    }

    /// Moves a position when its file is renamed.
    pub fn rename(&mut self, from: &Path, to: &Path) {
        let from = path_key(from);
        let to = path_key(to);
        if from == to {
            return;
        }
        if let Some(position) = self.0.remove(&from) {
            self.0.insert(to, position);
        }
    }

    /// Evicts the oldest entries above the cap, drops at-top entries and
    /// sanitizes offsets.
    pub fn normalize(&mut self) {
        for position in self.0.values_mut() {
            if !position.offset.is_finite() {
                position.offset = 0.0;
            }
        }
        self.0.retain(|_, position| !(position.id.is_empty() && position.offset <= 0.0));
        while self.0.len() > POSITIONS_MAX {
            let Some(oldest) = self.0.iter().min_by_key(|(_, position)| position.at).map(|(key, _)| key.clone()) else {
                break;
            };
            self.0.remove(&oldest);
        }
    }

    /// Number of stored positions.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no position is stored.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterates the table in path order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Position)> {
        self.0.iter().map(|(path, position)| (path.as_str(), position))
    }
}

/// Window and file-browser state that survives a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiPrefs {
    /// File-browser width, clamped to the real panel range.
    pub left_width: f32,
    /// Outline width, clamped to the real panel range.
    pub right_width: f32,
    pub left_hidden: bool,
    pub right_hidden: bool,
    /// Explicitly collapsed folders.
    pub collapsed_folders: Vec<PathBuf>,
    /// The commands used most recently from the palette, newest first.
    pub palette_recent: Vec<String>,
    /// The folder last shown by the file browser.
    pub tree_root: Option<PathBuf>,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            left_width: LEFT_WIDTH_DEFAULT as f32,
            right_width: RIGHT_WIDTH_DEFAULT as f32,
            left_hidden: false,
            right_hidden: false,
            collapsed_folders: Vec::new(),
            palette_recent: Vec::new(),
            tree_root: None,
        }
    }
}

impl UiPrefs {
    /// Clamps widths, de-duplicates folders and caps the palette list.
    pub fn normalize(&mut self) {
        if !self.left_width.is_finite() {
            self.left_width = LEFT_WIDTH_DEFAULT as f32;
        }
        if !self.right_width.is_finite() {
            self.right_width = RIGHT_WIDTH_DEFAULT as f32;
        }
        self.left_width = self.left_width.clamp(LEFT_WIDTH_DEFAULT as f32, LEFT_WIDTH_MAX as f32);
        self.right_width = self.right_width.clamp(RIGHT_WIDTH_DEFAULT as f32, RIGHT_WIDTH_MAX as f32);

        let mut seen = std::collections::HashSet::new();
        self.collapsed_folders.retain(|folder| seen.insert(folder.clone()));
        let mut seen = std::collections::HashSet::new();
        self.palette_recent.retain(|id| !id.is_empty() && seen.insert(id.clone()));
        self.palette_recent.truncate(PALETTE_RECENT_MAX);
    }
}

/// One window in the restored session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowSession {
    /// Native window position/size; `None` on Wayland or when unknown.
    pub placement: Option<WindowPlacement>,
    /// The window opened maximized. Kept beside `placement` because a session
    /// written on X11 may be restored on Wayland, where no placement exists.
    pub maximized: bool,
    /// Why the window exists, so a torn-off start page stays non-reusable.
    pub kind: SessionWindowKind,
    pub tabs: Vec<TabSession>,
    /// Index into `tabs`; clamped by [`SessionSnapshot::normalize`].
    pub active: usize,
    /// Closed tabs, most recent last.
    pub closed: Vec<ClosedTabSession>,
}

/// Why a window exists, as the original's `WindowKind` records it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionWindowKind {
    /// An ordinary window opened by the user or a launch.
    #[default]
    Normal,
    /// A window created by tearing out a start page; launch routing never
    /// reuses its lone start page.
    TornOffStart,
}

/// A window rectangle remembered for the next restore.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

/// What a restored tab shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TabSource {
    /// The start page.
    #[default]
    Start,
    /// A file.
    File { path: PathBuf },
    /// Pasted text, restored from its draft.
    Pasted { id: u64, title: String },
}

/// A tab in a restored session. Editor text is never stored here; drafts
/// handle recovery.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TabSession {
    pub source: TabSource,
    pub anchor: Option<ScrollAnchor>,
    /// A `--line` or link target, 1-based.
    pub line: Option<usize>,
    /// A heading id target.
    pub anchor_id: Option<String>,
    /// The tab's Back/Forward places, as reopened (O-15); empty when the schema
    /// predates them.
    pub nav: NavHistory,
    /// The mode the tab was in; text and undo always come from the file and
    /// its draft, never from the session.
    pub mode: TabMode,
}

/// A tab's reading mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TabMode {
    /// Read-only rendered view.
    #[default]
    Viewer,
    /// Source editor on the left, live preview on the right.
    Editor,
}

/// A closed tab that Ctrl+Shift+T can reopen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClosedTabSession {
    pub source: TabSource,
    pub index: usize,
    pub anchor: Option<ScrollAnchor>,
}

/// The whole restorable session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionSnapshot {
    /// `false` while running, `true` after a clean quit; a false value means
    /// the previous run crashed.
    pub clean: bool,
    pub windows: Vec<WindowSession>,
}

impl SessionSnapshot {
    /// Clamps active indexes, sanitizes anchors and caps closed tabs.
    pub fn normalize(&mut self) {
        for window in &mut self.windows {
            if window.tabs.is_empty() {
                window.active = 0;
            } else {
                window.active = window.active.min(window.tabs.len() - 1);
            }
            if let Some(placement) = &mut window.placement {
                placement.width = placement.width.max(0);
                placement.height = placement.height.max(0);
                window.maximized |= placement.maximized;
                placement.maximized = window.maximized;
            }
            for tab in &mut window.tabs {
                if let Some(anchor) = &mut tab.anchor {
                    anchor.sanitize();
                }
                tab.nav.sanitize();
            }
            if window.closed.len() > CLOSED_TABS_MAX {
                let excess = window.closed.len() - CLOSED_TABS_MAX;
                window.closed.drain(0..excess);
            }
            for tab in &mut window.closed {
                if let Some(anchor) = &mut tab.anchor {
                    anchor.sanitize();
                }
            }
        }
    }
}

/// A state change sent to the writer thread. Values are whole snapshots; the
/// writer replaces the previous pending value for the same file.
#[derive(Clone, Debug)]
pub enum StorePatch {
    /// `prefs.json`.
    Prefs(Prefs),
    /// `recents.json`, newest first.
    Recents(Vec<Recent>),
    /// `positions.json`.
    Positions(PositionTable),
    /// `session.json`.
    Session(SessionSnapshot),
    /// `ui.json`.
    Ui(UiPrefs),
    /// One draft, keyed by the document path or `pasted:<id>`.
    ///
    /// The core `Draft` carries no key, so this variant adds it; the writer
    /// hashes the key into the draft file name.
    Draft { key: String, draft: Draft },
    /// Removes one draft by key.
    RemoveDraft(String),
}

/// A handle to the writer thread. Cloning shares the same writer.
#[derive(Clone, Debug)]
pub struct StoreHandle {
    tx: Sender<StoreMsg>,
}

impl StoreHandle {
    /// Starts the writer for `dirs`. The directories are created if needed.
    ///
    /// The thread is detached; dropping every handle lets it drain pending
    /// values and exit. Call [`Self::flush`] on close/quit when the caller
    /// needs a guarantee.
    pub fn start(dirs: AppDirs) -> io::Result<Self> {
        Self::start_with(dirs, &[])
    }

    /// Starts the writer, never writing the files that `issues` mark read-only.
    ///
    /// Use the issues returned by the `*_reported` loaders: a future-schema or
    /// foreign-app file must keep its contents for a newer version or its own
    /// application. Corrupt and oversized issues stay writable — their file
    /// was quarantined, so fresh defaults are the intended result.
    pub fn start_with(dirs: AppDirs, issues: &[StoreIssue]) -> io::Result<Self> {
        Self::start_with_drafts(dirs, issues, &[])
    }

    /// Starts the writer, never writing the state files or drafts that `issues`
    /// and `drafts` mark read-only.
    ///
    /// This is [`Self::start_with`] plus the draft files that a newer version
    /// or another application owns; the five singleton state files keep their
    /// existing entry point. A draft that is protected this way is neither
    /// overwritten nor removed by the writer for the rest of the session.
    pub fn start_with_drafts(dirs: AppDirs, issues: &[StoreIssue], drafts: &[DraftIssue]) -> io::Result<Self> {
        dirs.ensure()?;
        let _ = enforce_draft_caps(&dirs);
        let read_only = read_only_keys(issues);
        let read_only_drafts = read_only_draft_paths(drafts).into_iter().collect();
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("lmdv-store".to_owned())
            .spawn(move || Writer::new(dirs, read_only, read_only_drafts, rx).run())?;
        Ok(Self { tx })
    }

    /// Queues a whole-file patch. This never blocks.
    pub fn patch(&self, patch: StorePatch) {
        let _ = self.tx.send(StoreMsg::Patch(patch));
    }

    /// Marks the files named by `issues` as read-only for the rest of the
    /// session, for a load that happened after [`Self::start`], such as the
    /// session file.
    ///
    /// Send this before the first patch of those files; the writer also drops
    /// a value that is still pending when the message arrives. It has no
    /// effect on values already written.
    pub fn protect_files(&self, issues: &[StoreIssue]) {
        let files: Vec<StateFile> = issues.iter().filter(|issue| issue.is_read_only()).map(StoreIssue::file).collect();
        if !files.is_empty() {
            let _ = self.tx.send(StoreMsg::Protect(files));
        }
    }

    /// Marks the draft files named by `issues` as read-only for the rest of the
    /// session, for a load that happened after [`Self::start_with`], such as a
    /// draft opened from the file browser.
    ///
    /// Send this before the first patch for that key; the writer also drops a
    /// draft that is still pending when the message arrives. It has no effect
    /// on drafts already written.
    pub fn protect_drafts(&self, issues: &[DraftIssue]) {
        let files = read_only_draft_paths(issues);
        if !files.is_empty() {
            let _ = self.tx.send(StoreMsg::ProtectDrafts(files));
        }
    }

    /// Writes every pending value and waits for the writer to acknowledge.
    ///
    /// Use only on close/quit, never in a frame. Returns `false` on timeout,
    /// when the writer is gone, or when a write failed. Drafts are already
    /// written on their own debounce, so data loss is bounded either way.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (ack, wait) = mpsc::sync_channel(1);
        if self.tx.send(StoreMsg::Flush(ack)).is_err() {
            return false;
        }
        wait.recv_timeout(timeout).unwrap_or(false)
    }
}

enum StoreMsg {
    Patch(StorePatch),
    Protect(Vec<StateFile>),
    ProtectDrafts(Vec<PathBuf>),
    Flush(SyncSender<bool>),
}

fn read_only_keys(issues: &[StoreIssue]) -> HashSet<Key> {
    issues.iter().filter(|issue| issue.is_read_only()).map(|issue| issue.file().key()).collect()
}

fn read_only_draft_paths(issues: &[DraftIssue]) -> Vec<PathBuf> {
    issues.iter().filter(|issue| issue.is_read_only()).map(|issue| issue.file().to_path_buf()).collect()
}

#[derive(Clone, Debug)]
enum Value {
    Prefs(Prefs),
    Recents(Vec<Recent>),
    Positions(PositionTable),
    Session(SessionSnapshot),
    Ui(UiPrefs),
    Draft(Draft),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Prefs,
    Recents,
    Positions,
    Session,
    Ui,
    Draft(String),
}

impl StateFile {
    fn key(self) -> Key {
        match self {
            Self::Prefs => Key::Prefs,
            Self::Recents => Key::Recents,
            Self::Positions => Key::Positions,
            Self::Ui => Key::Ui,
            Self::Session => Key::Session,
        }
    }
}

struct Pending {
    value: Value,
    due: Instant,
    attempts: u8,
}

struct Writer {
    dirs: AppDirs,
    rx: Receiver<StoreMsg>,
    pending: HashMap<Key, Pending>,
    last_write: HashMap<Key, Instant>,
    /// Files left in place for a newer version or another application; patches
    /// for them are dropped for the rest of the session.
    read_only: HashSet<Key>,
    /// Draft files left in place for a newer version or another application,
    /// keyed by path because a newer schema may spell the key differently.
    read_only_drafts: HashSet<PathBuf>,
    coalesce: Duration,
    failed: bool,
}

impl Writer {
    fn new(dirs: AppDirs, read_only: HashSet<Key>, read_only_drafts: HashSet<PathBuf>, rx: Receiver<StoreMsg>) -> Self {
        Self {
            dirs,
            rx,
            pending: HashMap::new(),
            last_write: HashMap::new(),
            read_only,
            read_only_drafts,
            coalesce: COALESCE,
            failed: false,
        }
    }

    fn run(&mut self) {
        loop {
            let now = Instant::now();
            let next_due = self.pending.values().map(|pending| pending.due).min();
            let message = match next_due {
                None => match self.rx.recv() {
                    Ok(message) => Some(message),
                    Err(_) => break,
                },
                Some(due) => match due.checked_duration_since(now) {
                    Some(wait) if !wait.is_zero() => match self.rx.recv_timeout(wait) {
                        Ok(message) => Some(message),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    },
                    _ => None,
                },
            };

            match message {
                Some(StoreMsg::Patch(patch)) => self.apply(patch),
                Some(StoreMsg::Protect(files)) => self.protect(files),
                Some(StoreMsg::ProtectDrafts(files)) => self.protect_drafts(files),
                Some(StoreMsg::Flush(ack)) => {
                    self.write_all();
                    let ok = self.pending.is_empty() && !self.failed;
                    self.failed = false;
                    let _ = ack.send(ok);
                }
                None => self.write_due(),
            }

            if let Some(due) = self.pending.values().map(|pending| pending.due).min()
                && due <= Instant::now()
            {
                self.write_due();
            }
        }
        // A dropped sender is a crash or a missing flush; write what remains.
        self.write_all();
    }

    fn apply(&mut self, patch: StorePatch) {
        let (key, value) = match patch {
            StorePatch::Prefs(mut prefs) => {
                prefs.sanitize();
                (Key::Prefs, Value::Prefs(prefs))
            }
            StorePatch::Recents(mut recents) => {
                normalize_recents(&mut recents);
                (Key::Recents, Value::Recents(recents))
            }
            StorePatch::Positions(mut positions) => {
                positions.normalize();
                (Key::Positions, Value::Positions(positions))
            }
            StorePatch::Session(mut session) => {
                session.normalize();
                (Key::Session, Value::Session(session))
            }
            StorePatch::Ui(mut ui) => {
                ui.normalize();
                (Key::Ui, Value::Ui(ui))
            }
            StorePatch::Draft { key, draft } => {
                let key = Key::Draft(key);
                if self.is_protected_draft(&key) {
                    // A newer version's or another application's draft stays.
                    self.pending.remove(&key);
                    return;
                }
                (key, Value::Draft(draft))
            }
            StorePatch::RemoveDraft(key) => {
                if self.is_protected_draft(&Key::Draft(key.clone())) {
                    // Never delete a draft this version cannot read.
                    return;
                }
                self.pending.remove(&Key::Draft(key.clone()));
                let _ = fs::remove_file(draft_path(&self.dirs, &key));
                return;
            }
        };

        if self.read_only.contains(&key) {
            // A newer version's or another application's file stays untouched.
            self.pending.remove(&key);
            return;
        }
        let due = self.due_for(&key);
        self.pending.insert(key, Pending { value, due, attempts: 0 });
    }

    fn protect(&mut self, files: Vec<StateFile>) {
        for file in files {
            let key = file.key();
            self.read_only.insert(key.clone());
            self.pending.remove(&key);
        }
    }

    fn protect_drafts(&mut self, files: Vec<PathBuf>) {
        for path in files {
            self.read_only_drafts.insert(path.clone());
            // A draft still waiting to be written would replace the protected
            // file; drop it before it reaches the writer.
            self.pending.retain(|key, _| match key {
                Key::Draft(name) => draft_path(&self.dirs, name) != path,
                _ => true,
            });
        }
    }

    /// Whether `key` names a draft file that must not be written or removed.
    fn is_protected_draft(&self, key: &Key) -> bool {
        match key {
            Key::Draft(name) => self.read_only_drafts.contains(&draft_path(&self.dirs, name)),
            _ => false,
        }
    }

    fn due_for(&self, key: &Key) -> Instant {
        let now = Instant::now();
        match self.last_write.get(key) {
            Some(last) => (*last + self.coalesce).max(now),
            None => now,
        }
    }

    fn write_due(&mut self) {
        let now = Instant::now();
        let due: Vec<Key> =
            self.pending.iter().filter(|(_, pending)| pending.due <= now).map(|(key, _)| key.clone()).collect();
        let had_draft = due.iter().any(|key| matches!(key, Key::Draft(_)));
        for key in due {
            self.write_key(&key);
        }
        if had_draft {
            let _ = enforce_draft_caps(&self.dirs);
        }
    }

    fn write_all(&mut self) {
        let keys: Vec<Key> = self.pending.keys().cloned().collect();
        let had_draft = keys.iter().any(|key| matches!(key, Key::Draft(_)));
        for key in keys {
            self.write_key(&key);
        }
        if had_draft {
            let _ = enforce_draft_caps(&self.dirs);
        }
    }

    fn write_key(&mut self, key: &Key) {
        if self.read_only.contains(key) || self.is_protected_draft(key) {
            self.pending.remove(key);
            return;
        }
        let Some(mut pending) = self.pending.remove(key) else {
            return;
        };
        self.last_write.insert(key.clone(), Instant::now());
        let result = match (key, &pending.value) {
            (Key::Prefs, Value::Prefs(prefs)) => write_json(&self.dirs.prefs_path(), prefs),
            (Key::Recents, Value::Recents(recents)) => write_json(&self.dirs.recents_path(), recents),
            (Key::Positions, Value::Positions(positions)) => write_json(&self.dirs.positions_path(), positions),
            (Key::Session, Value::Session(session)) => write_json(&self.dirs.session_path(), session),
            (Key::Ui, Value::Ui(ui)) => write_json(&self.dirs.ui_path(), ui),
            (Key::Draft(draft_key), Value::Draft(draft)) => write_draft(&self.dirs, draft_key, draft),
            _ => return,
        };

        if result.is_err() {
            pending.attempts = pending.attempts.saturating_add(1);
            if pending.attempts < 3 {
                pending.due = Instant::now() + Duration::from_secs(1);
                self.pending.insert(key.clone(), pending);
            } else {
                self.failed = true;
            }
        }
    }
}

#[derive(Serialize)]
struct Envelope<T> {
    schema: u32,
    app: String,
    data: T,
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    ensure_private_parent(path)?;
    let bytes = serde_json::to_vec_pretty(&Envelope { schema: STORE_SCHEMA, app: APP_NAME.to_owned(), data: value })
        .map_err(io::Error::other)?;
    write_private(path, &bytes)
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // State files are user-private where the platform has POSIX modes;
    // Windows keeps the ACL inherited from the profile folder.
    #[cfg(unix)]
    const PRIVATE: Option<u32> = Some(0o600);
    #[cfg(not(unix))]
    const PRIVATE: Option<u32> = None;

    write_atomic_with(path, PRIVATE, |file| {
        use std::io::Write;
        file.write_all(bytes)
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_private_parent(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let existed = fs::symlink_metadata(parent).is_ok();
    fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    if existed {
        // Never chmod an arbitrary existing parent (a misconfigured test root
        // could otherwise be `/tmp`); AppDirs::ensure owns the real app dirs.
        return Ok(());
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn ensure_private_parent(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent)
}

enum EnvelopeRead<T> {
    Data(T),
    /// A newer schema. The raw payload rides along so a draft issue can still
    /// name the key it records.
    FutureSchema {
        schema: u64,
        data: Option<serde_json::Value>,
    },
    /// Another application's file; the raw payload is kept for the same reason.
    ForeignApp {
        app: String,
        data: Option<serde_json::Value>,
    },
    Corrupt,
}

/// Inspects the envelope before deserializing the payload, so a future schema
/// or another application's file is ignored without being treated as corrupt.
fn read_envelope<T: DeserializeOwned>(bytes: &[u8]) -> EnvelopeRead<T> {
    match read_envelope_value(bytes) {
        EnvelopeRead::Data(data) => match serde_json::from_value(data) {
            Ok(data) => EnvelopeRead::Data(data),
            Err(_) => EnvelopeRead::Corrupt,
        },
        EnvelopeRead::FutureSchema { schema, data } => EnvelopeRead::FutureSchema { schema, data },
        EnvelopeRead::ForeignApp { app, data } => EnvelopeRead::ForeignApp { app, data },
        EnvelopeRead::Corrupt => EnvelopeRead::Corrupt,
    }
}

/// The envelope header plus the still-untyped `data` payload.
fn read_envelope_value(bytes: &[u8]) -> EnvelopeRead<serde_json::Value> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return EnvelopeRead::Corrupt;
    };
    let schema = value.get("schema").and_then(serde_json::Value::as_u64).unwrap_or(0);
    let data = value.get("data").cloned();
    if schema > u64::from(STORE_SCHEMA) {
        return EnvelopeRead::FutureSchema { schema, data };
    }
    if let Some(app) = value.get("app").and_then(serde_json::Value::as_str)
        && !app.is_empty()
        && app != APP_NAME
    {
        return EnvelopeRead::ForeignApp { app: app.to_owned(), data };
    }
    match data {
        Some(data) => EnvelopeRead::Data(data),
        None => EnvelopeRead::Corrupt,
    }
}

fn read_state<T>(path: &Path, file: StateFile) -> LoadedState<T>
where
    T: DeserializeOwned + Default,
{
    let default = || LoadedState { value: T::default(), issue: None };
    let metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return default(),
    };
    let oversized = || LoadedState { value: T::default(), issue: Some(StoreIssue::Oversized { file }) };
    if metadata.len() > MAX_STATE_BYTES {
        quarantine(path);
        return oversized();
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return default(),
    };
    if bytes.len() as u64 > MAX_STATE_BYTES {
        quarantine(path);
        return oversized();
    }
    match read_envelope(&bytes) {
        EnvelopeRead::Data(value) => LoadedState { value, issue: None },
        EnvelopeRead::FutureSchema { schema, .. } => {
            LoadedState { value: T::default(), issue: Some(StoreIssue::FutureSchema { file, schema }) }
        }
        EnvelopeRead::ForeignApp { app, .. } => {
            LoadedState { value: T::default(), issue: Some(StoreIssue::ForeignApp { file, app }) }
        }
        EnvelopeRead::Corrupt => {
            quarantine(path);
            LoadedState { value: T::default(), issue: Some(StoreIssue::Corrupt { file }) }
        }
    }
}

fn quarantine(path: &Path) -> Option<PathBuf> {
    let stamp = now_ms();
    for attempt in 0..100u32 {
        let candidate = PathBuf::from(format!(
            "{}.bad-{stamp}{}",
            path.display(),
            if attempt == 0 { String::new() } else { format!("-{attempt}") }
        ));
        if candidate.exists() {
            continue;
        }
        if fs::rename(path, &candidate).is_ok() {
            return Some(candidate);
        }
        return None;
    }
    None
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_millis() as u64)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn normalize_recents(recents: &mut Vec<Recent>) {
    let mut seen = std::collections::HashSet::new();
    recents.retain(|recent| !recent.path.as_os_str().is_empty() && seen.insert(recent.path.clone()));
    recents.truncate(RECENTS_MAX);
}

/// Loads recents, newest first, capped at [`RECENTS_MAX`].
pub fn load_recents(dirs: &AppDirs) -> Vec<Recent> {
    load_recents_reported(dirs).value
}

/// Loads recents and reports a future-schema, foreign, oversized or corrupt file.
pub fn load_recents_reported(dirs: &AppDirs) -> LoadedState<Vec<Recent>> {
    let mut loaded = read_state::<Vec<Recent>>(&dirs.recents_path(), StateFile::Recents);
    normalize_recents(&mut loaded.value);
    loaded
}

/// Loads reading positions, capped at [`POSITIONS_MAX`].
pub fn load_positions(dirs: &AppDirs) -> PositionTable {
    load_positions_reported(dirs).value
}

/// Loads reading positions and reports a future-schema, foreign, oversized or
/// corrupt file.
pub fn load_positions_reported(dirs: &AppDirs) -> LoadedState<PositionTable> {
    let mut loaded = read_state::<PositionTable>(&dirs.positions_path(), StateFile::Positions);
    loaded.value.normalize();
    loaded
}

/// Loads UI state with widths and caps normalized.
pub fn load_ui(dirs: &AppDirs) -> UiPrefs {
    load_ui_reported(dirs).value
}

/// Loads UI state and reports a future-schema, foreign, oversized or corrupt
/// file.
pub fn load_ui_reported(dirs: &AppDirs) -> LoadedState<UiPrefs> {
    let mut loaded = read_state::<UiPrefs>(&dirs.ui_path(), StateFile::Ui);
    loaded.value.normalize();
    loaded
}

/// Loads the restorable session with active indexes and closed tabs normalized.
pub fn load_session(dirs: &AppDirs) -> SessionSnapshot {
    load_session_reported(dirs).value
}

/// Loads the restorable session and reports a future-schema, foreign,
/// oversized or corrupt file.
pub fn load_session_reported(dirs: &AppDirs) -> LoadedState<SessionSnapshot> {
    let mut loaded = read_state::<SessionSnapshot>(&dirs.session_path(), StateFile::Session);
    loaded.value.normalize();
    loaded
}

/// Loads preferences. A corrupt file is quarantined and defaults are returned.
pub fn load_prefs(dirs: &AppDirs) -> Prefs {
    load_prefs_reported(dirs).value
}

/// Loads preferences and reports a future-schema, foreign, oversized or
/// corrupt file.
pub fn load_prefs_reported(dirs: &AppDirs) -> LoadedState<Prefs> {
    let mut loaded = read_state::<Prefs>(&dirs.prefs_path(), StateFile::Prefs);
    loaded.value.sanitize();
    loaded
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct DraftRecord {
    key: String,
    draft: Draft,
}

fn draft_path(dirs: &AppDirs, key: &str) -> PathBuf {
    let hash = blake3::hash(key.as_bytes()).to_hex();
    dirs.drafts_dir().join(format!("{hash}.json"))
}

fn write_draft(dirs: &AppDirs, key: &str, draft: &Draft) -> io::Result<()> {
    let bytes = serde_json::to_vec(&Envelope {
        schema: STORE_SCHEMA,
        app: APP_NAME.to_owned(),
        data: DraftRecord { key: key.to_owned(), draft: draft.clone() },
    })
    .map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(io::Error::other("draft exceeds the state file limit"));
    }
    let path = draft_path(dirs, key);
    ensure_private_parent(&path)?;
    write_private(&path, &bytes)
}

/// Loads every draft, newest first, capped at the entry and character limits.
///
/// A future-schema or foreign-app draft is skipped; use
/// [`load_drafts_reported`] to see why and keep it read-only.
pub fn load_drafts(dirs: &AppDirs) -> Vec<(String, Draft)> {
    load_drafts_reported(dirs).value
}

/// Loads every draft and reports each file that was ignored or quarantined.
///
/// A future-schema or foreign-app draft is not returned and stays on disk;
/// pass the issues to [`StoreHandle::start_with_drafts`] or
/// [`StoreHandle::protect_drafts`] so a same-key draft never replaces it.
pub fn load_drafts_reported(dirs: &AppDirs) -> LoadedDrafts {
    let (mut drafts, issues) = read_all_drafts(dirs);
    drafts.sort_by_key(|entry| std::cmp::Reverse(entry.1.at));
    let mut kept = Vec::new();
    let mut total = 0usize;
    for (key, draft, _) in drafts {
        let chars = draft.char_count();
        if kept.len() >= DRAFT_MAX_ENTRIES || (total + chars > DRAFT_MAX_CHARS && !kept.is_empty()) {
            continue;
        }
        total += chars;
        kept.push((key, draft));
    }
    LoadedDrafts { value: kept, issues }
}

/// Loads one draft by key.
///
/// A future-schema or foreign-app draft is `None`; use [`load_draft_reported`]
/// to see why and keep it read-only.
pub fn load_draft(dirs: &AppDirs, key: &str) -> Option<Draft> {
    load_draft_reported(dirs, key).value
}

/// Loads one draft by key and reports a future-schema, foreign, oversized or
/// corrupt file.
///
/// The key is the document's own spelling. When it is an absolute file path
/// and nothing is stored under it, the same file's canonical spelling is
/// tried: a document opened through a symlink is keyed by the canonical path
/// once it loads, so its draft lives there even after the file (and the link
/// target) is gone (OI-171). A draft found that way is re-keyed to the
/// requested spelling, so the caller's prompt, dismissal and save clean it up
/// like any other.
pub fn load_draft_reported(dirs: &AppDirs, key: &str) -> LoadedDraft {
    let loaded = read_draft(dirs, key);
    if loaded.value.is_some() || loaded.issue.is_some() {
        return loaded;
    }
    for alias in draft_keys(Path::new(key)).into_iter().skip(1) {
        let found = read_draft(dirs, &alias);
        if found.issue.is_some() {
            return found;
        }
        if let Some(draft) = found.value {
            adopt_draft(dirs, &alias, key, &draft);
            return LoadedDraft { value: Some(draft), issue: None };
        }
    }
    loaded
}

fn read_draft(dirs: &AppDirs, key: &str) -> LoadedDraft {
    let path = draft_path(dirs, key);
    match read_draft_file(&path) {
        DraftFileRead::Data(record) if record.key == key => LoadedDraft { value: Some(record.draft), issue: None },
        DraftFileRead::Data(_) | DraftFileRead::Absent => LoadedDraft { value: None, issue: None },
        other => LoadedDraft { value: None, issue: draft_issue(&path, other) },
    }
}

/// The keys a document path's draft may be stored under, most specific first
/// (OI-171).
///
/// The first entry is the spelling as given; a second appears when the path is
/// absolute and resolves to another spelling. The path itself is canonicalized
/// when it exists; a deleted file cannot be, so its nearest existing ancestor
/// is resolved and the missing tail re-attached (`/link/note.md` still yields
/// `/real/note.md` while `/link` is a symlink to `/real`). `pasted:<id>` and
/// relative keys have no alias.
pub fn draft_keys(path: &Path) -> Vec<String> {
    let mut keys = vec![path.to_string_lossy().into_owned()];
    if let Some(canonical) = canonical_draft_spelling(path) {
        let key = canonical.to_string_lossy().into_owned();
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

fn canonical_draft_spelling(path: &Path) -> Option<PathBuf> {
    canonical_draft_spelling_from(path, 0)
}

fn canonical_draft_spelling_from(path: &Path, depth: u8) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    if let Ok(canonical) = fs::canonicalize(path) {
        return Some(canonical);
    }
    // A link whose target is gone cannot be canonicalized, but the link itself
    // still names where the file was: follow it by hand, resolving a relative
    // target against the canonical parent so the result matches the key the
    // file had while it existed.
    if depth < 8
        && fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
        && let Ok(target) = fs::read_link(path)
    {
        let parent = path.parent()?;
        let base = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        let target = if target.is_absolute() { target } else { base.join(target) };
        return canonical_draft_spelling_from(&target, depth + 1);
    }
    // The file itself is gone or unreadable. Resolve the deepest ancestor that
    // still exists and re-attach the names below it.
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut ancestor = path;
    loop {
        missing.push(ancestor.file_name()?.to_os_string());
        ancestor = ancestor.parent()?;
        if let Ok(mut canonical) = fs::canonicalize(ancestor) {
            for name in missing.iter().rev() {
                canonical.push(name);
            }
            return Some(canonical);
        }
    }
}

/// Moves a draft found under the same file's canonical spelling to the
/// spelling the document uses (OI-171), so one key governs it from here on.
fn adopt_draft(dirs: &AppDirs, from: &str, to: &str, draft: &Draft) {
    if write_draft(dirs, to, draft).is_ok() {
        let _ = fs::remove_file(draft_path(dirs, from));
    }
}

fn read_all_drafts(dirs: &AppDirs) -> (Vec<(String, Draft, PathBuf)>, Vec<DraftIssue>) {
    let Ok(entries) = fs::read_dir(dirs.drafts_dir()) else {
        return (Vec::new(), Vec::new());
    };
    let mut drafts = Vec::new();
    let mut issues = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != std::ffi::OsStr::new("json")) {
            continue;
        }
        match read_draft_file(&path) {
            DraftFileRead::Data(record) if !record.key.is_empty() => {
                drafts.push((record.key, record.draft, path));
            }
            DraftFileRead::Data(_) | DraftFileRead::Absent => {}
            other => issues.extend(draft_issue(&path, other)),
        }
    }
    (drafts, issues)
}

enum DraftFileRead {
    Data(DraftRecord),
    FutureSchema {
        schema: u64,
        key: Option<String>,
    },
    ForeignApp {
        app: String,
        key: Option<String>,
    },
    Oversized,
    Corrupt,
    /// Missing, unreadable or not a regular file; not a problem to report.
    Absent,
}

fn draft_issue(path: &Path, read: DraftFileRead) -> Option<DraftIssue> {
    let file = path.to_path_buf();
    match read {
        DraftFileRead::Data(_) | DraftFileRead::Absent => None,
        DraftFileRead::FutureSchema { schema, key } => Some(DraftIssue::FutureSchema { file, key, schema }),
        DraftFileRead::ForeignApp { app, key } => Some(DraftIssue::ForeignApp { file, key, app }),
        DraftFileRead::Oversized => Some(DraftIssue::Oversized { file }),
        DraftFileRead::Corrupt => Some(DraftIssue::Corrupt { file }),
    }
}

/// The key a draft envelope records, when its payload is still an object with
/// a `key` string. A newer schema or another application may not have one;
/// protection uses the file path anyway.
fn draft_key_of(data: Option<&serde_json::Value>) -> Option<String> {
    data?.get("key")?.as_str().filter(|key| !key.is_empty()).map(str::to_owned)
}

fn read_draft_file(path: &Path) -> DraftFileRead {
    let metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return DraftFileRead::Absent,
    };
    if metadata.len() > MAX_STATE_BYTES {
        quarantine(path);
        return DraftFileRead::Oversized;
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return DraftFileRead::Absent,
    };
    if bytes.len() as u64 > MAX_STATE_BYTES {
        quarantine(path);
        return DraftFileRead::Oversized;
    }
    match read_envelope_value(&bytes) {
        EnvelopeRead::Data(data) => match serde_json::from_value::<DraftRecord>(data) {
            Ok(record) => DraftFileRead::Data(record),
            Err(_) => {
                quarantine(path);
                DraftFileRead::Corrupt
            }
        },
        EnvelopeRead::FutureSchema { schema, data } => {
            DraftFileRead::FutureSchema { schema, key: draft_key_of(data.as_ref()) }
        }
        EnvelopeRead::ForeignApp { app, data } => DraftFileRead::ForeignApp { app, key: draft_key_of(data.as_ref()) },
        EnvelopeRead::Corrupt => {
            quarantine(path);
            DraftFileRead::Corrupt
        }
    }
}

fn enforce_draft_caps(dirs: &AppDirs) -> io::Result<()> {
    let (mut drafts, _) = read_all_drafts(dirs);
    drafts.sort_by_key(|entry| std::cmp::Reverse(entry.1.at));
    let mut kept = 0usize;
    let mut total = 0usize;
    for (_, draft, path) in drafts {
        let chars = draft.char_count();
        let over_count = kept >= DRAFT_MAX_ENTRIES;
        let over_total = total + chars > DRAFT_MAX_CHARS && kept > 0;
        if !over_count && !over_total {
            kept += 1;
            total += chars;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;

    use super::*;
    use lmdv_core::ThemePref;

    fn dirs(temp: &TempDir) -> AppDirs {
        let root = temp.path();
        AppDirs {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
            logs: root.join("state/logs"),
            portable: false,
            instance_id: "test".to_owned(),
        }
    }

    #[test]
    fn recents_are_capped_and_deduplicated() {
        let mut recents: Vec<Recent> =
            (0..20).map(|index| Recent::new(format!("/tmp/{index}.md"), index as u64)).collect();
        recents.push(Recent::new("/tmp/5.md", 99));
        normalize_recents(&mut recents);
        assert_eq!(recents.len(), RECENTS_MAX);
        assert_eq!(recents[0].path, PathBuf::from("/tmp/0.md"));
        assert!(recents.iter().filter(|recent| recent.path == Path::new("/tmp/5.md")).count() <= 1);
    }

    #[test]
    fn positions_forget_the_top_evict_the_oldest_and_rename() {
        let mut positions = PositionTable::new();
        positions.remember(Path::new("/a.md"), &ScrollAnchor { id: "h".into(), offset: 10.0 }, 1);
        assert_eq!(positions.get(Path::new("/a.md")).unwrap().id, "h");
        positions.remember(Path::new("/a.md"), &ScrollAnchor::top(), 2);
        assert!(positions.get(Path::new("/a.md")).is_none(), "top forgets");

        for index in 0..(POSITIONS_MAX + 10) {
            positions.remember(
                Path::new(&format!("/{index}.md")),
                &ScrollAnchor { id: "h".into(), offset: 1.0 },
                index as u64,
            );
        }
        assert_eq!(positions.len(), POSITIONS_MAX);
        assert!(positions.get(Path::new("/0.md")).is_none(), "oldest evicted");
        assert!(positions.get(Path::new("/109.md")).is_some());

        positions.remember(Path::new("/old.md"), &ScrollAnchor { id: "x".into(), offset: 2.0 }, 200);
        positions.rename(Path::new("/old.md"), Path::new("/new.md"));
        assert!(positions.get(Path::new("/old.md")).is_none());
        assert_eq!(positions.get(Path::new("/new.md")).unwrap().id, "x");
    }

    #[test]
    fn ui_widths_clamp_and_palette_is_capped() {
        let mut ui = UiPrefs { left_width: 9999.0, right_width: 1.0, ..UiPrefs::default() };
        ui.palette_recent = (0..20).map(|index| format!("cmd.{index}")).collect();
        ui.normalize();
        assert_eq!(ui.left_width, LEFT_WIDTH_MAX as f32);
        assert_eq!(ui.right_width, RIGHT_WIDTH_DEFAULT as f32);
        assert_eq!(ui.palette_recent.len(), PALETTE_RECENT_MAX);
    }

    #[test]
    fn session_active_index_and_closed_tabs_are_bounded() {
        let mut session = SessionSnapshot {
            clean: false,
            windows: vec![WindowSession {
                tabs: vec![TabSession::default()],
                active: 9,
                closed: (0..40).map(|index| ClosedTabSession { index, ..ClosedTabSession::default() }).collect(),
                ..WindowSession::default()
            }],
        };
        session.normalize();
        assert_eq!(session.windows[0].active, 0);
        assert_eq!(session.windows[0].closed.len(), CLOSED_TABS_MAX);
        assert_eq!(session.windows[0].closed.first().unwrap().index, 15, "the newest closed tabs are kept");
        assert_eq!(session.windows[0].closed.last().unwrap().index, 39);
    }

    #[test]
    fn session_carries_mode_kind_and_maximized_with_defaults_for_old_files() {
        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                placement: Some(WindowPlacement { x: 10, y: 20, width: 800, height: 600, maximized: true }),
                maximized: true,
                kind: SessionWindowKind::TornOffStart,
                tabs: vec![
                    TabSession {
                        source: TabSource::File { path: PathBuf::from("/tmp/a.md") },
                        anchor: Some(ScrollAnchor { id: "intro".into(), offset: 12.0 }),
                        line: Some(4),
                        anchor_id: None,
                        nav: NavHistory::default(),
                        mode: TabMode::Editor,
                    },
                    TabSession {
                        source: TabSource::Pasted { id: 7, title: "Notes".to_owned() },
                        ..TabSession::default()
                    },
                ],
                active: 1,
                closed: Vec::new(),
            }],
        };
        let json = serde_json::to_string(&session).unwrap();
        let parsed: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, session);

        // A file written before mode/kind/maximized existed still loads.
        let old: SessionSnapshot = serde_json::from_str(
            r#"{"clean":false,"windows":[{"tabs":[{"source":{"kind":"file","path":"/tmp/a.md"}}]}]}"#,
        )
        .unwrap();
        assert!(!old.windows[0].maximized);
        assert_eq!(old.windows[0].kind, SessionWindowKind::Normal);
        assert_eq!(old.windows[0].tabs[0].mode, TabMode::Viewer);
    }

    #[test]
    fn normalize_keeps_the_maximized_flag_consistent_with_the_placement() {
        let mut session = SessionSnapshot {
            clean: false,
            windows: vec![
                WindowSession {
                    placement: Some(WindowPlacement { maximized: true, ..WindowPlacement::default() }),
                    ..WindowSession::default()
                },
                WindowSession {
                    maximized: true,
                    placement: Some(WindowPlacement::default()),
                    ..WindowSession::default()
                },
            ],
        };
        session.normalize();
        assert!(session.windows[0].maximized, "an old file's placement flag reaches the window flag");
        assert_eq!(
            session.windows[1].placement.map(|placement| placement.maximized),
            Some(true),
            "a restored maximized window is also recorded in its placement"
        );
    }

    #[test]
    fn values_round_trip_through_the_writer_and_a_reopen() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        let handle = StoreHandle::start(dirs.clone()).unwrap();

        let recents = vec![Recent::new("/tmp/a.md", 2), Recent::new("/tmp/b.md", 1)];
        let mut positions = PositionTable::new();
        positions.remember(Path::new("/tmp/a.md"), &ScrollAnchor { id: "intro".into(), offset: 8.0 }, 5);
        let prefs = Prefs { theme: ThemePref::Sepia, ..Prefs::default() };
        let ui = UiPrefs { tree_root: Some(PathBuf::from("/tmp/notes")), ..UiPrefs::default() };
        let session = SessionSnapshot {
            clean: true,
            windows: vec![WindowSession {
                placement: Some(WindowPlacement { x: 1, y: 2, width: 800, height: 600, maximized: false }),
                tabs: vec![TabSession {
                    source: TabSource::File { path: PathBuf::from("/tmp/a.md") },
                    anchor: Some(ScrollAnchor { id: "intro".into(), offset: 4.0 }),
                    line: Some(3),
                    anchor_id: None,
                    nav: NavHistory::default(),
                    mode: TabMode::Editor,
                }],
                active: 0,
                closed: Vec::new(),
                ..WindowSession::default()
            }],
        };

        handle.patch(StorePatch::Prefs(prefs.clone()));
        handle.patch(StorePatch::Recents(recents.clone()));
        handle.patch(StorePatch::Positions(positions.clone()));
        handle.patch(StorePatch::Ui(ui.clone()));
        handle.patch(StorePatch::Session(session.clone()));
        assert!(handle.flush(Duration::from_secs(2)));

        drop(handle);
        assert_eq!(load_prefs(&dirs), prefs);
        assert_eq!(load_recents(&dirs), recents);
        assert_eq!(load_positions(&dirs).get(Path::new("/tmp/a.md")).unwrap().id, "intro");
        assert_eq!(load_ui(&dirs), ui);
        assert_eq!(load_session(&dirs), session);

        for path in [dirs.prefs_path(), dirs.recents_path(), dirs.positions_path(), dirs.ui_path(), dirs.session_path()]
        {
            let text = fs::read_to_string(&path).unwrap();
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(json["schema"], STORE_SCHEMA);
            assert_eq!(json["app"], APP_NAME);
        }
    }

    #[test]
    fn drafts_survive_a_crash_style_reopen_and_can_be_removed() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        let key = "/tmp/note.md";
        handle.patch(StorePatch::Draft { key: key.to_owned(), draft: Draft::new("edited\n", "base\n", 42) });
        assert!(handle.flush(Duration::from_secs(2)));

        // Simulate a crash: drop the handle without a clean quit; the draft is
        // already on disk and a new process can read it.
        drop(handle);
        let draft = load_draft(&dirs, key).expect("draft after reopen");
        assert_eq!(draft.text, "edited\n");
        assert_eq!(draft.base, "base\n");
        assert_eq!(load_drafts(&dirs).len(), 1);

        let handle = StoreHandle::start(dirs.clone()).unwrap();
        handle.patch(StorePatch::RemoveDraft(key.to_owned()));
        assert!(handle.flush(Duration::from_secs(2)));
        assert!(load_draft(&dirs, key).is_none());
        assert!(load_drafts(&dirs).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_deleted_file_behind_a_symlink_finds_its_canonical_draft() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        let folder = temp.path().join("real");
        fs::create_dir(&folder).unwrap();
        let real = folder.join("note.md");
        fs::write(&real, "old\n").unwrap();
        let alias = temp.path().join("alias.md");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let canonical = fs::canonicalize(&real).unwrap();
        let canonical_key = canonical.to_string_lossy().into_owned();
        let alias_key = alias.to_string_lossy().into_owned();
        assert_ne!(canonical_key, alias_key, "the test needs two spellings");

        let handle = StoreHandle::start(dirs.clone()).unwrap();
        handle.patch(StorePatch::Draft { key: canonical_key.clone(), draft: Draft::new("edited\n", "old\n", 7) });
        assert!(handle.flush(Duration::from_secs(2)));
        drop(handle);

        // The file is deleted; the symlink stays but no longer resolves.
        fs::remove_file(&real).unwrap();
        assert!(fs::canonicalize(&alias).is_err(), "the link is broken");

        let loaded = load_draft_reported(&dirs, &alias_key);
        assert_eq!(loaded.value.map(|draft| draft.text), Some("edited\n".to_owned()));
        assert_eq!(loaded.issue, None, "the canonical draft is valid");
        assert!(load_draft(&dirs, &canonical_key).is_none(), "it moved to the spelling the document uses");
        assert_eq!(load_draft(&dirs, &alias_key).map(|draft| draft.text), Some("edited\n".to_owned()));

        // The direct path keeps working with its own key first.
        let direct = temp.path().join("direct.md");
        fs::write(&direct, "x\n").unwrap();
        let direct_key = direct.to_string_lossy().into_owned();
        let direct_keys = draft_keys(&direct);
        assert_eq!(direct_keys.first().map(String::as_str), Some(direct_key.as_str()));
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        handle.patch(StorePatch::Draft { key: direct_key.clone(), draft: Draft::new("y\n", "x\n", 8) });
        assert!(handle.flush(Duration::from_secs(2)));
        assert_eq!(load_draft(&dirs, &direct_key).map(|draft| draft.text), Some("y\n".to_owned()));
        assert_eq!(draft_keys(Path::new("pasted:3")), vec!["pasted:3".to_owned()]);
    }

    #[test]
    fn drafts_are_capped_by_count_and_characters() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        for index in 0..(DRAFT_MAX_ENTRIES + 3) {
            handle
                .patch(StorePatch::Draft { key: format!("/tmp/{index}.md"), draft: Draft::new("x", "", index as u64) });
        }
        assert!(handle.flush(Duration::from_secs(2)));
        let drafts = load_drafts(&dirs);
        assert_eq!(drafts.len(), DRAFT_MAX_ENTRIES);
        assert_eq!(drafts[0].1.at, (DRAFT_MAX_ENTRIES + 2) as u64, "newest first");
        assert!(drafts.iter().any(|(key, _)| key == &format!("/tmp/{}.md", DRAFT_MAX_ENTRIES + 2)), "newest kept");

        drop(handle);
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        let big = "x".repeat(1_100_000);
        for index in 0..3 {
            handle.patch(StorePatch::Draft {
                key: format!("/tmp/big{index}.md"),
                draft: Draft::new(big.clone(), "", 100 + index as u64),
            });
        }
        assert!(handle.flush(Duration::from_secs(5)));
        let drafts = load_drafts(&dirs);
        let total: usize = drafts.iter().map(|(_, draft)| draft.char_count()).sum();
        assert_eq!(drafts[0].0, "/tmp/big2.md", "the newest large draft is kept");
        assert!(total <= DRAFT_MAX_CHARS, "the total stays within the 2M character cap, got {total}");
        assert_eq!(drafts.iter().filter(|(_, draft)| draft.char_count() > 100_000).count(), 1);
    }

    #[test]
    fn a_future_schema_draft_is_reported_and_survives_a_same_key_write() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        let key = "/tmp/future-draft.md";
        let path = draft_path(&dirs, key);
        let future = serde_json::json!({
            "schema": 99,
            "app": "lmdv",
            "data": {"key": key, "draft": {"text": "from a newer version", "base": "", "at": 1}}
        });
        let bytes = serde_json::to_vec(&future).unwrap();
        fs::write(&path, &bytes).unwrap();

        let loaded = load_draft_reported(&dirs, key);
        assert!(loaded.value.is_none(), "an unknown future schema reads as no draft");
        assert_eq!(
            loaded.issue,
            Some(DraftIssue::FutureSchema { file: path.clone(), key: Some(key.to_owned()), schema: 99 })
        );
        assert!(loaded.issue.as_ref().unwrap().is_read_only());

        // The unreported loaders keep their behavior.
        assert!(load_draft(&dirs, key).is_none());
        let all = load_drafts_reported(&dirs);
        assert!(all.value.is_empty());
        assert_eq!(all.issues.len(), 1);
        assert!(path.exists(), "a future draft is not corrupt and is left alone");

        let issue = all.issues.into_iter().next().expect("reported");
        let handle = StoreHandle::start_with_drafts(dirs.clone(), &[], &[issue]).unwrap();
        handle.patch(StorePatch::Draft { key: key.to_owned(), draft: Draft::new("this version", "", 2) });
        handle.patch(StorePatch::RemoveDraft(key.to_owned()));
        assert!(handle.flush(Duration::from_secs(2)));
        assert_eq!(fs::read(&path).unwrap(), bytes, "a newer version's draft is untouched");
    }

    #[test]
    fn a_foreign_draft_is_reported_and_a_late_load_can_protect_it() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        let key = "/tmp/shared-name.md";
        let path = draft_path(&dirs, key);
        let foreign = serde_json::json!({
            "schema": 1,
            "app": "other-app",
            "data": {"key": key, "text": "not lmdv's draft"}
        });
        let bytes = serde_json::to_vec(&foreign).unwrap();
        fs::write(&path, &bytes).unwrap();

        // Drafts are loaded per document, after the writer started.
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        let loaded = load_draft_reported(&dirs, key);
        assert!(loaded.value.is_none());
        assert_eq!(
            loaded.issue,
            Some(DraftIssue::ForeignApp { file: path.clone(), key: Some(key.to_owned()), app: "other-app".to_owned() })
        );
        handle.protect_drafts(&[loaded.issue.expect("reported")]);
        handle.patch(StorePatch::Draft { key: key.to_owned(), draft: Draft::new("lmdv text", "", 3) });
        assert!(handle.flush(Duration::from_secs(2)));
        assert_eq!(fs::read(&path).unwrap(), bytes, "the other app's draft is untouched");
    }

    #[test]
    fn a_corrupt_draft_is_reported_and_quarantined() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        let good_key = "/tmp/good.md";
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        handle.patch(StorePatch::Draft { key: good_key.to_owned(), draft: Draft::new("keep", "", 1) });
        assert!(handle.flush(Duration::from_secs(2)));
        drop(handle);

        let bad_key = "/tmp/bad.md";
        let bad_path = draft_path(&dirs, bad_key);
        fs::write(&bad_path, b"{ not json").unwrap();

        let loaded = load_draft_reported(&dirs, bad_key);
        assert!(loaded.value.is_none());
        assert_eq!(loaded.issue, Some(DraftIssue::Corrupt { file: bad_path.clone() }));
        assert!(!loaded.issue.unwrap().is_read_only(), "a quarantined draft may be written fresh");
        assert!(!bad_path.exists());
        assert_eq!(load_drafts(&dirs).len(), 1, "other drafts still load");
        let quarantined = fs::read_dir(dirs.drafts_dir())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".bad-"));
        assert!(quarantined, "the unreadable draft is kept aside");
    }

    #[test]
    fn a_corrupt_file_is_quarantined_and_defaults_are_returned() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        fs::write(dirs.recents_path(), b"{ not json").unwrap();

        let loaded = load_recents_reported(&dirs);
        assert!(loaded.value.is_empty());
        assert_eq!(loaded.issue, Some(StoreIssue::Corrupt { file: StateFile::Recents }));
        assert!(!loaded.issue.unwrap().is_read_only(), "a quarantined file may be written fresh");
        assert!(!dirs.recents_path().exists());
        let quarantined: Vec<_> = fs::read_dir(&dirs.data)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("recents.json.bad-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "one quarantine copy");
    }

    #[test]
    fn a_future_schema_is_reported_but_not_overwritten_or_quarantined() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        let future = serde_json::json!({
            "schema": 99,
            "app": "lmdv",
            "data": [{"path": "/tmp/future.md", "name": "future.md", "dir": "/tmp", "openedAt": 1}]
        });
        let bytes = serde_json::to_vec(&future).unwrap();
        fs::write(dirs.recents_path(), &bytes).unwrap();

        let loaded = load_recents_reported(&dirs);
        assert!(loaded.value.is_empty(), "unknown future schema reads as defaults");
        assert_eq!(
            loaded.issue,
            Some(StoreIssue::FutureSchema { file: StateFile::Recents, schema: 99 }),
            "the app can warn instead of failing silently"
        );
        assert!(dirs.recents_path().exists(), "the future file is left alone");
        let quarantined = fs::read_dir(&dirs.data)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".bad-"));
        assert!(!quarantined, "a future schema is not corrupt");

        // The writer must keep that file for the newer version.
        let issue = loaded.issue.expect("reported");
        assert!(issue.is_read_only());
        let handle = StoreHandle::start_with(dirs.clone(), &[issue]).unwrap();
        handle.patch(StorePatch::Recents(vec![Recent::new("/tmp/this-version.md", 2)]));
        handle.patch(StorePatch::Prefs(Prefs::default()));
        assert!(handle.flush(Duration::from_secs(2)));
        assert_eq!(fs::read(dirs.recents_path()).unwrap(), bytes, "the newer version's file is untouched");
        assert!(dirs.prefs_path().exists(), "other state files still write");
    }

    #[test]
    fn a_foreign_app_file_is_reported_and_never_written() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        let foreign = serde_json::json!({"schema": 1, "app": "other-app", "data": {"theme": "dark"}});
        let bytes = serde_json::to_vec(&foreign).unwrap();
        fs::write(dirs.prefs_path(), &bytes).unwrap();

        let loaded = load_prefs_reported(&dirs);
        assert_eq!(loaded.value, Prefs::default(), "another app's prefs read as defaults");
        assert_eq!(loaded.issue, Some(StoreIssue::ForeignApp { file: StateFile::Prefs, app: "other-app".to_owned() }));

        let handle = StoreHandle::start_with(dirs.clone(), &[loaded.issue.expect("reported")]).unwrap();
        handle.patch(StorePatch::Prefs(Prefs { theme: ThemePref::Dark, ..Prefs::default() }));
        assert!(handle.flush(Duration::from_secs(2)));
        assert_eq!(fs::read(dirs.prefs_path()).unwrap(), bytes, "the other app's file is untouched");
    }

    #[test]
    fn a_late_load_can_protect_the_session_file() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        let future = serde_json::json!({"schema": 99, "app": "lmdv", "data": {"clean": true, "windows": []}});
        let bytes = serde_json::to_vec(&future).unwrap();
        fs::write(dirs.session_path(), &bytes).unwrap();

        // The app starts the writer before the session is loaded.
        let handle = StoreHandle::start(dirs.clone()).unwrap();
        let loaded = load_session_reported(&dirs);
        assert_eq!(loaded.issue, Some(StoreIssue::FutureSchema { file: StateFile::Session, schema: 99 }));
        let issue = loaded.issue.expect("reported");
        handle.protect_files(&[issue]);
        handle.patch(StorePatch::Session(SessionSnapshot { clean: true, ..SessionSnapshot::default() }));
        assert!(handle.flush(Duration::from_secs(2)));
        assert_eq!(fs::read(dirs.session_path()).unwrap(), bytes, "the newer session survives");
    }

    #[test]
    fn an_oversized_state_file_is_reported_and_quarantined() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        let file = fs::File::create(dirs.ui_path()).unwrap();
        file.set_len(MAX_STATE_BYTES + 1).unwrap();
        drop(file);

        let loaded = load_ui_reported(&dirs);
        assert_eq!(loaded.value, UiPrefs::default());
        assert_eq!(loaded.issue, Some(StoreIssue::Oversized { file: StateFile::Ui }));
        assert!(!loaded.issue.unwrap().is_read_only(), "a quarantined file may be written fresh");
        assert!(!dirs.ui_path().exists());
        let quarantined = fs::read_dir(&dirs.data)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().starts_with("ui.json.bad-"));
        assert!(quarantined, "the oversized file is kept aside");
    }

    #[test]
    fn a_missing_state_file_reports_no_issue() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        let loaded = load_ui_reported(&dirs);
        assert_eq!(loaded.value, UiPrefs::default());
        assert!(loaded.issue.is_none(), "first run is not a problem");
    }

    #[test]
    fn patches_are_coalesced_per_file() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        let handle = StoreHandle::start(dirs.clone()).unwrap();

        let first = vec![Recent::new("/tmp/first.md", 1)];
        let second = vec![Recent::new("/tmp/second.md", 2)];
        let third = vec![Recent::new("/tmp/third.md", 3)];
        handle.patch(StorePatch::Recents(first));
        std::thread::sleep(Duration::from_millis(40));
        handle.patch(StorePatch::Recents(second));
        std::thread::sleep(Duration::from_millis(40));
        handle.patch(StorePatch::Recents(third));
        assert!(handle.flush(Duration::from_secs(2)));

        let recents = load_recents(&dirs);
        assert_eq!(recents.len(), 1);
        assert_eq!(recents[0].path, PathBuf::from("/tmp/third.md"), "the last patch wins");
    }

    #[test]
    fn flush_works_through_a_clone() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        let handle = StoreHandle::start(dirs).unwrap();
        let clone = handle.clone();
        drop(handle);
        // The clone still holds the sender, so this is a live writer.
        assert!(clone.flush(Duration::from_secs(1)));
        drop(clone);
    }

    #[test]
    fn a_leftover_temp_file_is_ignored() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        fs::write(dirs.data.join(".recents.json.1.2.tmp"), b"not json").unwrap();
        assert!(load_recents(&dirs).is_empty());
        assert!(dirs.data.join(".recents.json.1.2.tmp").exists(), "a crash leftover is inert");
    }

    #[cfg(unix)]
    #[test]
    fn state_files_are_private_even_when_an_old_copy_was_loose() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        fs::write(dirs.recents_path(), br#"{"schema":1,"app":"lmdv","data":[]}"#).unwrap();
        fs::set_permissions(dirs.recents_path(), fs::Permissions::from_mode(0o644)).unwrap();

        let handle = StoreHandle::start(dirs.clone()).unwrap();
        handle.patch(StorePatch::Recents(vec![Recent::new("/tmp/a.md", 1)]));
        assert!(handle.flush(Duration::from_secs(2)));
        assert_eq!(fs::metadata(dirs.recents_path()).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn preferences_keep_their_wire_shape() {
        let temp = TempDir::new().unwrap();
        let dirs = dirs(&temp);
        dirs.ensure().unwrap();
        let mut file = fs::File::create(dirs.prefs_path()).unwrap();
        file.write_all(br#"{"schema":1,"app":"lmdv","data":{"theme":"dark"}}"#).unwrap();
        let prefs = load_prefs(&dirs);
        assert_eq!(prefs.theme, ThemePref::Dark);
        assert_eq!(prefs.editor, lmdv_core::EditorPrefs::default());
    }
}
