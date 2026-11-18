//! The command palette (C-01–C-13, P-10).
//!
//! One box that finds the app's commands, the headings of the document, the
//! window's other tabs, the files of the folder and the recent ones. It is a
//! modal over the window: the input holds the keyboard, the list is a listbox
//! the input points at with `aria-activedescendant`, and every other shortcut
//! is held back while it shows (C-09).
//!
//! # What is listed (C-02)
//!
//! * **Commands** that can run now, in the app's suggestion order: only what an
//!   enabled predicate allows, with the recently used ones first (C-05).
//! * **Headings** h1–h4 of the document in front, with the ancestors they are
//!   in (“Install › Windows”) searchable too (C-02/C-03).
//! * **The other tabs** of this window, the front one left out, most recently
//!   shown first.
//! * **Files**: the file browser's tree and the recent files, one entry per
//!   path, files open in this window left out, at most 2,000.
//!
//! Groups are capped exactly as the original caps them: at the start commands 6,
//! tabs 5, files 5; while typing commands 6, headings 6, tabs 5, files 8; a bare
//! `#` lists up to 200 outline rows around the heading being read.
//!
//! # Matching (C-03/C-05)
//!
//! [`crate::fuzzy`] does the scoring; this module keeps the caches, orders the
//! matches (score, then recency or document order, then the shorter label) and
//! remembers where the highlight was.
//!
//! # Keys (C-01/C-06/C-08/C-09)
//!
//! `Ctrl+K` opens it while reading and `Ctrl+Shift+P` anywhere (the editor's
//! `Ctrl+K` inserts a link); the opening key closes it again on a fresh press.
//! Up and Down wrap, Page Down goes to the next group's first row, Page Up to
//! the start of this group or the one before, Tab is swallowed, Enter runs or
//! opens (Ctrl a tab behind, Ctrl+Shift a tab in front, Shift a window for
//! files), Esc and the backdrop close. Nothing else reaches the window behind
//! it except the text box's own editing keys and F1, which closes the palette
//! and opens Help.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use lmdv_core::{DocumentId, TabId};
use rune::{Key, KeyEvent, Mods, TextEdit, Tx};

use crate::fuzzy;
use crate::window::{Mode, WindowState};

/// The palette's modal root.
pub const PALETTE_KEY: &str = "palette";
/// The palette's panel.
pub const PALETTE_PANEL: &str = "palette-panel";
/// The search field (`role=combobox`).
pub const PALETTE_INPUT_KEY: &str = "palette-input";
/// The list (`role=listbox`).
pub const PALETTE_LIST_KEY: &str = "palette-list";
/// The live result count.
pub const PALETTE_STATUS_KEY: &str = "palette-status";
/// The help line the input is described by.
pub const PALETTE_HELP_KEY: &str = "palette-help";
/// Option keys start with this; the row key follows.
pub const PALETTE_OPTION_PREFIX: &str = "palette-opt-";
/// Group keys start with this; the group index follows.
pub const PALETTE_GROUP_PREFIX: &str = "palette-group-";
/// One row's height, in points.
pub const PALETTE_ROW_HEIGHT: f32 = 34.0;
/// One group title's height, in points.
pub const PALETTE_GROUP_HEIGHT: f32 = 22.0;
/// How long after the last key the count is announced (C-12).
pub const PALETTE_ANNOUNCE_SECONDS: f64 = 0.300;
/// Commands listed while typing (`>` lists them all).
pub const COMMANDS_CAP: usize = 6;
/// Headings listed while typing.
pub const HEADINGS_CAP: usize = 6;
/// Tabs listed while typing.
pub const TABS_CAP: usize = 5;
/// Files listed while typing.
pub const FILES_CAP: usize = 8;
/// Commands listed before anything is typed.
pub const START_COMMANDS_CAP: usize = 6;
/// Tabs listed before anything is typed.
pub const START_TABS_CAP: usize = 5;
/// Recent files listed before anything is typed.
pub const START_FILES_CAP: usize = 5;
/// Outline rows a bare `#` lists.
pub const OUTLINE_CAP: usize = 200;
/// Recently used commands remembered (P-10).
pub const RECENT_MAX: usize = 8;
/// The most headings the palette builds entries for.
pub const HEADING_ENTRY_MAX: usize = 20_000;
/// The most files the palette lists (C-02).
pub const FILE_ENTRY_MAX: usize = 2_000;

/// The heading path separator (“Install › Windows”).
pub const PATH_SEPARATOR: &str = " › ";

// The suggestion order, as the original's appCommands gives it: what one does
// next on the start page, while reading and while editing.
const SUGGEST_START: &[&str] =
    &["file.open", "file.paste", "browser.search", "file.new-window", "app.reading-settings", "app.help"];
const SUGGEST_READING: &[&str] =
    &["find.open", "edit.toggle", "share.menu", "view.toggle-outline", "browser.search", "share.print"];
const SUGGEST_EDITING: &[&str] = &["edit.toggle", "file.save", "edit.find", "edit.bold", "share.menu", "share.print"];

/// The chord that opened (or would open) the palette (C-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteChord {
    /// `Ctrl+K` while reading.
    CtrlK,
    /// `Ctrl+Shift+P` everywhere.
    CtrlShiftP,
}

impl PaletteChord {
    /// Whether `event` is this chord: Ctrl, no Alt (AltGr types), no Super.
    pub fn matches(self, event: &KeyEvent) -> bool {
        if !event.mods.ctrl || event.mods.alt || event.mods.logo {
            return false;
        }
        match self {
            Self::CtrlK => event.is_command('k') && !event.mods.shift,
            Self::CtrlShiftP => event.is_command('p') && event.mods.shift,
        }
    }
}

/// The chord `event` would open the palette with, or none (C-01).
///
/// `Ctrl+K` opens it only while reading: in the editor it inserts a link
/// (`edit.link`). Nothing opens it while a dialog or menu is in front (C-13).
pub fn palette_open_chord(state: &WindowState, event: &KeyEvent) -> Option<PaletteChord> {
    if state.palette.open
        || state.editor.prompt.is_some()
        || state.help_view().is_some()
        || state.share.menu_open
        || state.tables.preview.is_some()
        || state.lightbox.is_some()
    {
        return None;
    }
    if !event.mods.ctrl || event.mods.alt || event.mods.logo {
        return None;
    }
    if PaletteChord::CtrlShiftP.matches(event) {
        return Some(PaletteChord::CtrlShiftP);
    }
    let editing = state.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor);
    if PaletteChord::CtrlK.matches(event) && !editing {
        return Some(PaletteChord::CtrlK);
    }
    None
}

/// What a row is, for its icon and its accessible kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Command,
    Heading,
    Tab,
    File,
    FindInDocument,
    SearchFolder,
}

/// What a row does when it is chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryAction {
    /// Run the command with this id.
    Command(&'static str),
    /// Jump to the heading with this rendered key (`h3`).
    Heading(String),
    /// Bring this tab to the front.
    Tab(TabId),
    /// Open this file with the placement keys' meaning.
    File(PathBuf),
    /// Find the query in the document.
    FindInDocument(String),
    /// Search the folder for the query.
    SearchFolder(String),
}

/// Where a chosen file opens (C-06).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaletteWhere {
    /// In this window, in front (`Enter`).
    #[default]
    Here,
    /// In a tab behind (`Ctrl+Enter`).
    BackgroundTab,
    /// In a tab in front (`Ctrl+Shift+Enter`).
    ForegroundTab,
    /// In a new window (`Shift+Enter`).
    NewWindow,
}

impl PaletteWhere {
    /// The modifiers the file-browser open path reads for this placement.
    pub fn mods(self) -> Mods {
        match self {
            Self::Here => Mods::default(),
            Self::BackgroundTab => Mods { cmd: true, ctrl: true, ..Mods::default() },
            Self::ForegroundTab => Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() },
            Self::NewWindow => Mods { shift: true, ..Mods::default() },
        }
    }

    /// The placement `event`'s modifiers ask for: Ctrl a tab behind, Ctrl+Shift
    /// a tab in front, Shift a window, otherwise here (C-06).
    pub fn from_key(event: &KeyEvent) -> Self {
        if event.mods.ctrl && event.mods.shift {
            Self::ForegroundTab
        } else if event.mods.ctrl {
            Self::BackgroundTab
        } else if event.mods.shift {
            Self::NewWindow
        } else {
            Self::Here
        }
    }
}

/// One row of the palette.
#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub kind: EntryKind,
    /// The same thing from one list to the next, so the highlight can stay on it.
    pub key: String,
    pub label: String,
    /// Muted text after the label: a heading's sections, a tab's path, a file's folder.
    pub detail: String,
    /// More words that find it: keywords, the sections, the folder.
    pub extra: String,
    /// The shortcut, as the UI writes it.
    pub keys: String,
    /// A chip after the label (“Editor”).
    pub tag: String,
    /// A setting: a ticked box when true, an empty one when false, none when absent.
    pub checked: Option<bool>,
    /// Headings: how deep in the outline.
    pub depth: u8,
    /// The one being read.
    pub current: bool,
    /// Recent files: when they were last opened (0: not recent).
    pub opened_at: u64,
    /// After the score: recent use, document order, recency.
    pub order: i64,
    pub action: EntryAction,
    /// The folded label and each word's score, made on first use.
    pub(crate) target: Option<fuzzy::Target>,
    pub score: f64,
    pub hits: Vec<usize>,
}

impl PaletteEntry {
    pub(crate) fn new(kind: EntryKind, key: String, label: String, action: EntryAction) -> Self {
        Self {
            kind,
            key,
            label,
            detail: String::new(),
            extra: String::new(),
            keys: String::new(),
            tag: String::new(),
            checked: None,
            depth: 0,
            current: false,
            opened_at: 0,
            order: 0,
            action,
            target: None,
            score: 0.0,
            hits: Vec::new(),
        }
    }

    fn with_extra(&mut self, extra: impl Into<String>) -> &mut Self {
        self.extra = extra.into();
        self
    }

    fn with_detail(&mut self, detail: impl Into<String>) -> &mut Self {
        self.detail = detail.into();
        self
    }

    fn with_keys(&mut self, keys: impl Into<String>) -> &mut Self {
        self.keys = keys.into();
        self
    }

    fn with_tag(&mut self, tag: impl Into<String>) -> &mut Self {
        self.tag = tag.into();
        self
    }

    fn with_check(&mut self, checked: bool) -> &mut Self {
        self.checked = Some(checked);
        self
    }

    /// The prepared target, made on first use.
    fn target(&mut self) -> &fuzzy::Target {
        if self.target.is_none() {
            let extra = (!self.extra.is_empty()).then_some(self.extra.as_str());
            self.target = Some(fuzzy::target(&self.label, extra));
        }
        self.target.as_ref().expect("the target was just made")
    }
}

/// Which list a shown row comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PalettePool {
    Commands,
    Headings,
    Tabs,
    Files,
    Search,
}

/// One row in the list: a pool and an index into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShownEntry {
    pub pool: PalettePool,
    pub index: usize,
}

/// What the list is for, which changes its groups and its highlight (C-04).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaletteMode {
    /// Nothing typed: commands, tabs and recent files.
    #[default]
    Start,
    /// Something typed: every group, then the search rows.
    All,
    /// `>`: commands only.
    Commands,
    /// A bare `#`: the outline in document order, around the heading being read.
    Outline,
    /// `#foo`: heading matches only.
    Headings,
}

/// One group of the drawn list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteGroup {
    pub title: String,
    /// The index of its first row in the flat list.
    pub first: usize,
    /// How many rows it holds.
    pub len: usize,
}

impl PaletteGroup {
    /// Every row index in the group.
    pub fn rows(&self) -> Range<usize> {
        self.first..self.first + self.len
    }
}

/// The list for what is typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaletteView {
    pub mode: PaletteMode,
    pub groups: Vec<PaletteGroup>,
    /// The row to highlight first.
    pub best: usize,
    /// How many rows were found (the search group not counted).
    pub results: usize,
    /// “Nothing matches …”, or empty.
    pub empty: String,
}

/// The tab and document the palette was opened on: any change closes it (C-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteAnchor {
    pub tab: TabId,
    pub document: Option<DocumentId>,
    pub revision: u64,
}

/// The palette's state for one window.
#[derive(Debug, Default)]
pub struct PaletteState {
    /// The modal is showing.
    pub open: bool,
    /// The chord that opened it; its fresh press closes it (C-01).
    pub chord: Option<PaletteChord>,
    /// The search box.
    pub query: TextEdit,
    opened_by_tab: Option<PaletteAnchor>,
    /// The page selection as it opened, restored on close (C-10).
    selection: Option<(String, Range<usize>)>,
    /// The selection to put back after the next poll.
    restore: Option<(String, Range<usize>)>,
    pub(crate) commands: Vec<PaletteEntry>,
    pub(crate) headings: Vec<PaletteEntry>,
    pub(crate) tabs: Vec<PaletteEntry>,
    pub(crate) files: Vec<PaletteEntry>,
    pub(crate) search: Vec<PaletteEntry>,
    pub(crate) shown: Vec<ShownEntry>,
    pub view: PaletteView,
    /// The highlighted row, as an index into [`Self::shown`].
    pub active: Option<usize>,
    /// The highlight was moved by the keys or the pointer since the text changed.
    pub chosen: bool,
    /// The last pointer position that moved the highlight, in window points.
    pub pointer: Option<(f32, f32)>,
    /// The heading being read when the palette opened.
    pub current_heading: Option<usize>,
    /// The screen reader's count, announced after the typing pauses (C-12).
    pub announce: String,
    pub(crate) announce_due: Option<f64>,
    /// The last action chosen, for tests and evidence.
    pub last_pick: Option<EntryAction>,
    /// The clock time the palette opened.
    pub opened_at: f64,
    /// The window body's size last frame, so a page step knows how far to move.
    pub(crate) viewport: std::cell::Cell<[f32; 2]>,
}

impl PaletteState {
    /// How many rows the list holds.
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// One row of the list.
    pub fn entry(&self, index: usize) -> Option<&PaletteEntry> {
        let shown = self.shown.get(index)?;
        self.pool(shown.pool).get(shown.index)
    }

    /// The pool behind a list.
    pub fn pool(&self, pool: PalettePool) -> &[PaletteEntry] {
        match pool {
            PalettePool::Commands => &self.commands,
            PalettePool::Headings => &self.headings,
            PalettePool::Tabs => &self.tabs,
            PalettePool::Files => &self.files,
            PalettePool::Search => &self.search,
        }
    }

    /// The pool behind a list, mutably.
    fn pool_mut(&mut self, pool: PalettePool) -> &mut Vec<PaletteEntry> {
        match pool {
            PalettePool::Commands => &mut self.commands,
            PalettePool::Headings => &mut self.headings,
            PalettePool::Tabs => &mut self.tabs,
            PalettePool::Files => &mut self.files,
            PalettePool::Search => &mut self.search,
        }
    }
}

impl WindowState {
    /// Whether the palette is showing.
    pub fn palette_is_open(&self) -> bool {
        self.palette.open
    }

    /// The palette's state.
    pub fn palette(&self) -> &PaletteState {
        &self.palette
    }

    /// Opens the palette: builds its lists from the window as it is now, focuses
    /// the search box and highlights the best row (C-01/C-05).
    pub fn open_palette(&mut self, chord: Option<PaletteChord>, tx: &mut Tx) {
        if self.palette.open {
            return;
        }
        self.palette.open = true;
        self.palette.chord = chord;
        self.palette.opened_by_tab = self.palette_anchor();
        self.palette.selection = palette_selection(tx);
        self.palette.restore = None;
        self.palette.query.replace_text(String::new());
        self.palette.query.set_selection(0, 0);
        self.palette.chosen = false;
        self.palette.pointer = None;
        self.palette.announce.clear();
        self.palette.announce_due = None;
        self.palette.last_pick = None;
        self.palette.opened_at = self.ui.last_now;
        self.palette.current_heading = self.palette_current_heading();
        self.build_palette_lists();
        self.palette_refresh(false);
        self.palette_reveal(tx);
        // The box takes the keyboard: the model's flag focuses the field on the
        // next commit (a scene opens the palette without a handler's `Tx`), and
        // the request covers the same change when there is one.
        self.palette.query.focused = true;
        tx.focus(PALETTE_INPUT_KEY, false);
    }

    /// Closes the palette and gives the page its selection back (C-10).
    ///
    /// The focus is Rune's modal stack's: it returns where it was before the
    /// palette mounted. A picked action runs after this, so what it focuses
    /// keeps the keyboard.
    pub fn close_palette(&mut self) {
        if !self.palette.open {
            return;
        }
        self.palette.open = false;
        self.palette.query.focused = false;
        self.palette.chord = None;
        self.palette.opened_by_tab = None;
        self.palette.commands.clear();
        self.palette.headings.clear();
        self.palette.tabs.clear();
        self.palette.files.clear();
        self.palette.search.clear();
        self.palette.shown.clear();
        self.palette.view = PaletteView::default();
        self.palette.active = None;
        self.palette.chosen = false;
        self.palette.pointer = None;
        self.palette.announce.clear();
        self.palette.announce_due = None;
        self.palette.current_heading = None;
        self.palette.restore = self.palette.selection.take();
    }

    /// The palette's keys while its search box (or anything inside it) has the
    /// keyboard; true when the key was the palette's (C-01/C-08/C-09).
    pub fn palette_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.palette.open {
            return false;
        }
        // F1 closes the palette and opens the full Help guide (C-09/C-11).
        if matches!(event.key, Key::Function(1)) && !event.mods.ctrl && !event.mods.alt {
            self.close_palette();
            self.open_help(crate::window::HelpView::Full);
            return true;
        }
        match &event.key {
            Key::Escape => {
                self.close_palette();
                true
            }
            Key::Down => {
                if !event.mods.ctrl {
                    self.palette_step(1, tx);
                }
                true
            }
            Key::Up => {
                if !event.mods.ctrl {
                    self.palette_step(-1, tx);
                }
                true
            }
            Key::PageDown => {
                if !event.mods.ctrl {
                    self.palette_step_page(1, tx);
                }
                true
            }
            Key::PageUp => {
                if !event.mods.ctrl {
                    self.palette_step_page(-1, tx);
                }
                true
            }
            Key::Enter => {
                if let Some(active) = self.palette.active {
                    self.palette_pick(active, PaletteWhere::from_key(event), tx);
                }
                true
            }
            // Tab is swallowed: the list is the search box's (C-08).
            Key::Tab => true,
            // The text box's own keys stay its own (C-09): typing, Backspace and
            // Delete, the arrows, Home and End, and Ctrl with A, C, V, X, Z, Y.
            Key::Text(_) if !event.mods.ctrl && !event.mods.cmd => false,
            Key::Backspace | Key::Delete | Key::Left | Key::Right | Key::Home | Key::End => false,
            Key::Command(letter)
                if !event.mods.alt && !event.mods.logo && matches!(letter, 'a' | 'c' | 'v' | 'x' | 'z' | 'y') =>
            {
                false
            }
            _ => {
                // The opening key closes it again; a held key's auto-repeat does
                // not (C-01: a fresh press only).
                if self.palette.chord.is_some_and(|chord| chord.matches(event)) {
                    if !tx.key_repeat() {
                        self.close_palette();
                    }
                    return true;
                }
                // Every other unhandled shortcut is held back: print, save, zoom,
                // find and the tab keys do not act on the window behind (C-09).
                true
            }
        }
    }

    /// The search box changed: recompute the list and reset the highlight to the
    /// best row, keeping the chosen one when the list merely grew (C-05).
    pub fn palette_query_changed(&mut self, tx: &mut Tx) {
        if !self.palette.open {
            return;
        }
        self.palette.chosen = false;
        self.palette_refresh(false);
        self.palette_reveal(tx);
        self.palette_announce_soon(tx);
    }

    /// Sets the query as typing would (tests and scenes).
    pub fn set_palette_query(&mut self, query: &str, tx: &mut Tx) {
        if !self.palette.open {
            return;
        }
        self.palette.query.replace_text(query.to_owned());
        let length = self.palette.query.text.len();
        // The caret at the end, as typing leaves it: the script's next letter appends.
        self.palette.query.set_selection(length, length);
        self.palette_query_changed(tx);
    }

    /// Sets the heading being read as the outline mode's centre (tests and
    /// scenes; the section line usually supplies it when the palette opens).
    pub fn set_palette_current_heading(&mut self, index: Option<usize>, tx: &mut Tx) {
        if !self.palette.open {
            return;
        }
        self.palette.current_heading = index;
        self.palette.headings = self.palette_headings();
        self.palette_refresh(true);
        self.palette_reveal(tx);
    }

    /// A pointer moved over row `index`: the highlight follows only when the
    /// pointer really moved and left the row it was on (C-08).
    pub fn palette_pointer_moved(&mut self, index: usize, x: f32, y: f32) {
        if !self.palette.open {
            return;
        }
        if self.palette.pointer == Some((x, y)) {
            return;
        }
        self.palette.pointer = Some((x, y));
        if self.palette.active == Some(index) || index >= self.palette.len() {
            return;
        }
        self.palette.chosen = true;
        self.palette.active = Some(index);
    }

    /// A row was pressed: choose it here, as Enter does (C-06).
    pub fn palette_row_pressed(&mut self, index: usize, tx: &mut Tx) {
        if !self.palette.open {
            return;
        }
        self.palette_pick(index, PaletteWhere::Here, tx);
    }

    /// A middle click on a row: a file opens in a tab behind, as the original
    /// does (`src/palette.ts:960-965`); every other kind of
    /// row has no middle-click action. Focus and the highlight are untouched,
    /// so the keys keep working on the same row.
    pub fn palette_row_middle(&mut self, index: usize, tx: &mut Tx) {
        if !self.palette.open {
            return;
        }
        let Some(entry) = self.palette.entry(index) else { return };
        if entry.kind != EntryKind::File {
            return;
        }
        self.palette_pick(index, PaletteWhere::BackgroundTab, tx);
    }

    /// The live count after the typing pauses, the poll's frame timers and the
    /// anchor/modal checks that close the palette (C-12/C-13).
    pub(crate) fn poll_palette(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        if let Some((key, range)) = self.palette.restore.take() {
            tx.select_text(key, range);
            changed = true;
        }
        if !self.palette.open {
            return changed;
        }
        // C-13: a tab switch or a document change under the palette closes it.
        if self.palette.opened_by_tab != self.palette_anchor() {
            self.close_palette();
            return true;
        }
        // C-13: another modal, menu or prompt opening closes it too.
        if self.editor.prompt.is_some()
            || self.help_view().is_some()
            || self.share.menu_open
            || self.tables.preview.is_some()
            || self.lightbox.is_some()
            || self.ui.block_menu.is_some()
            || self.tab_menu().is_some()
        {
            self.close_palette();
            return true;
        }
        if let Some(due) = self.palette.announce_due
            && now >= due
        {
            self.palette.announce_due = None;
            let text = self.palette_announcement();
            if self.palette.announce != text {
                self.palette.announce = text;
                changed = true;
            }
        }
        changed
    }

    /// The count to announce, or why nothing matched (C-12).
    fn palette_announcement(&self) -> String {
        if !self.palette.view.empty.is_empty() {
            return self.palette.view.empty.clone();
        }
        let results = self.palette.view.results;
        if results == 1 {
            lmdv_i18n::t!("pal.countOne").to_owned()
        } else {
            lmdv_i18n::t!("pal.count", "n" => lmdv_i18n::number(results as i64))
        }
    }

    fn palette_announce_soon(&mut self, tx: &mut Tx) {
        let due = self.ui.last_now + PALETTE_ANNOUNCE_SECONDS;
        self.palette.announce_due = Some(due);
        tx.wake_at(due);
    }

    // ---- building the lists ------------------------------------------------

    fn build_palette_lists(&mut self) {
        let recent = self.shared.ui().palette_recent.clone();
        self.palette.commands = self.palette_commands(&recent);
        self.palette.headings = self.palette_headings();
        self.palette.tabs = self.palette_tabs();
        self.palette.files = self.palette_files();
        self.palette.search.clear();
    }

    /// The commands that can run now, in the suggestion order, with an `order`
    /// that puts the recently used ones first (C-02/C-05).
    fn palette_commands(&self, recent: &[String]) -> Vec<PaletteEntry> {
        let editing = self.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor);
        let has_document = self.active_document_id().is_some();
        let suggestion = if editing {
            SUGGEST_EDITING
        } else if has_document {
            SUGGEST_READING
        } else {
            SUGGEST_START
        };
        let rank = |id: &str| suggestion.iter().position(|wanted| *wanted == id).unwrap_or(suggestion.len());
        let mut ranked: Vec<(usize, usize, &'static crate::commands::CommandSpec)> = crate::commands::COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, command)| (command.enabled)(self))
            .map(|(index, command)| (rank(command.id), index, command))
            .collect();
        ranked.sort_by_key(|(rank, index, _)| (*rank, *index));
        let localized = lmdv_i18n::locale_id() != "en";
        ranked
            .into_iter()
            .enumerate()
            .map(|(position, (_, _, command))| {
                let label = crate::commands::command_title(self, command);
                let mut extra = command.keywords.join(" ");
                // The English label finds a command in another language too (C-03).
                if localized && label != command.title {
                    extra.push(' ');
                    extra.push_str(command.title);
                }
                let mut entry = PaletteEntry::new(
                    EntryKind::Command,
                    format!("c:{}", command.id),
                    label,
                    EntryAction::Command(command.id),
                );
                entry.with_extra(extra);
                if let Some(keys) = command.default_keys.first() {
                    entry.with_keys((*keys).to_owned());
                }
                if crate::commands::is_editor_command(command.id) {
                    entry.with_tag(lmdv_i18n::t!("pal.editor").to_owned());
                }
                if let Some(checked) = command.checked {
                    entry.with_check(checked(self));
                }
                entry.order = match recent.iter().position(|id| id == command.id) {
                    Some(index) => index as i64,
                    None => (RECENT_MAX + position) as i64,
                };
                entry
            })
            .collect()
    }

    /// The headings of the document in front, with the ancestors they are in.
    fn palette_headings(&self) -> Vec<PaletteEntry> {
        let Some(tab) = self.active_tab() else { return Vec::new() };
        let Some(model) = tab.view.reader.as_ref() else { return Vec::new() };
        let top = crate::reader::toc_headings(model).map(|(_, heading)| heading.level).min().unwrap_or(1);
        let mut entries = Vec::new();
        let mut ancestors: Vec<(u8, String)> = Vec::new();
        for (index, heading) in crate::reader::toc_headings(model).take(HEADING_ENTRY_MAX) {
            while ancestors.last().is_some_and(|(level, _)| *level >= heading.level) {
                ancestors.pop();
            }
            let path = ancestors.iter().map(|(_, text)| text.as_str()).collect::<Vec<_>>().join(PATH_SEPARATOR);
            let mut entry = PaletteEntry::new(
                EntryKind::Heading,
                format!("h:{index}"),
                heading.text.clone(),
                EntryAction::Heading(lmdv_render::heading_key(index)),
            );
            entry.with_detail(path.clone()).with_extra(path);
            entry.depth = heading.level.saturating_sub(top);
            entry.current = self.palette.current_heading == Some(index);
            entry.order = index as i64;
            entries.push(entry);
            ancestors.push((heading.level, heading.text.clone()));
        }
        entries
    }

    /// The other tabs of this window, most recently shown first.
    fn palette_tabs(&self) -> Vec<PaletteEntry> {
        let front = self.active_tab().map(|tab| tab.id);
        let mut seen = BTreeSet::new();
        let mut order: Vec<TabId> = Vec::new();
        for id in &self.tab_recent {
            if Some(*id) == front || !seen.insert(*id) || self.tab(*id).is_none() {
                continue;
            }
            order.push(*id);
        }
        // A tab the recency list does not know yet (a session restore) still lists.
        for tab in self.tabs() {
            if Some(tab.id) == front || !seen.insert(tab.id) {
                continue;
            }
            order.push(tab.id);
        }
        order
            .into_iter()
            .filter_map(|id| self.tab(id))
            .enumerate()
            .map(|(position, tab)| {
                let mut entry =
                    PaletteEntry::new(EntryKind::Tab, format!("t:{}", tab.id), tab.title(), EntryAction::Tab(tab.id));
                // A pasted tab's tooltip repeats its title: only a path says more.
                let detail = tab.tooltip();
                entry.with_detail(if detail == entry.label { String::new() } else { detail.clone() });
                entry.with_extra(detail);
                entry.order = position as i64;
                entry
            })
            .collect()
    }

    /// The browser's Markdown files and the recent ones, one entry per path,
    /// without the files this window has open (C-02).
    fn palette_files(&self) -> Vec<PaletteEntry> {
        let open: BTreeSet<PathBuf> = self
            .tabs()
            .iter()
            .filter_map(|tab| tab.document())
            .filter_map(|document| document.key.borrow().path().map(Path::to_path_buf))
            .collect();
        let root = self.browser.root().map(Path::to_path_buf);
        let mut entries: Vec<PaletteEntry> = Vec::new();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        let place = |entries: &mut Vec<PaletteEntry>, seen: &mut BTreeMap<String, usize>, entry: PaletteEntry| {
            let folded = entry.key.clone();
            if let Some(&at) = seen.get(&folded) {
                // One entry per file: the tree lists it, a recent says when.
                if entry.opened_at > entries[at].opened_at {
                    entries[at].opened_at = entry.opened_at;
                    entries[at].order = -(entry.opened_at as i64);
                }
                return;
            }
            seen.insert(folded, entries.len());
            entries.push(entry);
        };
        let mut count = 0usize;
        for row in self.browser.rows() {
            if row.dir || count >= FILE_ENTRY_MAX {
                continue;
            }
            let path = PathBuf::from(&row.path);
            if open.contains(&path) {
                continue;
            }
            let mut entry = PaletteEntry::new(
                EntryKind::File,
                format!("f:{}", row.path.to_lowercase()),
                row.name.clone(),
                EntryAction::File(path.clone()),
            );
            entry.with_detail(folder_detail(&path, root.as_deref())).with_extra(folder_detail(&path, root.as_deref()));
            entry.order = count as i64;
            count += 1;
            place(&mut entries, &mut seen, entry);
        }
        for recent in self.shared.recents().iter() {
            if open.contains(&recent.path) {
                continue;
            }
            let mut entry = PaletteEntry::new(
                EntryKind::File,
                format!("f:{}", recent.path.to_string_lossy().to_lowercase()),
                recent.name.clone(),
                EntryAction::File(recent.path.clone()),
            );
            entry
                .with_detail(folder_detail(&recent.path, root.as_deref()))
                .with_extra(folder_detail(&recent.path, root.as_deref()));
            entry.opened_at = recent.opened_at;
            entry.order = -(recent.opened_at as i64);
            place(&mut entries, &mut seen, entry);
        }
        entries
    }

    // ---- the list for what is typed ---------------------------------------

    fn palette_refresh(&mut self, keep: bool) {
        let keep_key = if keep && self.palette.chosen {
            self.palette.active.and_then(|active| self.palette.entry(active).map(|entry| entry.key.clone()))
        } else {
            None
        };
        self.palette.view = self.palette_compute();
        let mut next = self.palette.view.best;
        if let Some(key) = keep_key {
            match (0..self.palette.len()).find(|index| self.palette.entry(*index).is_some_and(|entry| entry.key == key))
            {
                Some(index) => next = index,
                None => self.palette.chosen = false,
            }
        }
        self.palette.active = (next < self.palette.len()).then_some(next);
    }

    /// The groups, caps and first highlight for the query (C-02/C-04/C-05).
    fn palette_compute(&mut self) -> PaletteView {
        let raw = self.palette.query.text.clone();
        let lead = raw.trim_start();
        let prefix = lead.chars().next().filter(|c| *c == '>' || *c == '#');
        let query = match prefix {
            Some(_) => lead[1..].trim().to_owned(),
            None => raw.trim().to_owned(),
        };
        let words = if query.is_empty() { Vec::new() } else { fuzzy::words(&query) };
        let mut shown: Vec<ShownEntry> = Vec::new();
        let mut groups: Vec<PaletteGroup> = Vec::new();
        let mut mode = PaletteMode::Start;
        match prefix {
            Some('>') => {
                mode = PaletteMode::Commands;
                let rows = if words.is_empty() {
                    self.palette_in_order(PalettePool::Commands)
                } else {
                    self.palette_ranked(PalettePool::Commands, &words, usize::MAX)
                };
                push_group(
                    &mut shown,
                    &mut groups,
                    lmdv_i18n::t!("pal.gCommands").to_owned(),
                    PalettePool::Commands,
                    rows,
                );
            }
            Some('#') if words.is_empty() => {
                mode = PaletteMode::Outline;
                let rows = self.palette_outline_around(OUTLINE_CAP);
                push_group(
                    &mut shown,
                    &mut groups,
                    lmdv_i18n::t!("pal.gHeadings").to_owned(),
                    PalettePool::Headings,
                    rows,
                );
            }
            Some('#') => {
                mode = PaletteMode::Headings;
                let rows = self.palette_ranked(PalettePool::Headings, &words, OUTLINE_CAP);
                push_group(
                    &mut shown,
                    &mut groups,
                    lmdv_i18n::t!("pal.gHeadings").to_owned(),
                    PalettePool::Headings,
                    rows,
                );
            }
            Some(_) => {}
            None if words.is_empty() => {
                mode = PaletteMode::Start;
                let mut rows = self.palette_in_order(PalettePool::Commands);
                rows.truncate(START_COMMANDS_CAP);
                push_group(
                    &mut shown,
                    &mut groups,
                    lmdv_i18n::t!("pal.gCommands").to_owned(),
                    PalettePool::Commands,
                    rows,
                );
                let rows: Vec<usize> = (0..self.palette.tabs.len()).take(START_TABS_CAP).collect();
                push_group(&mut shown, &mut groups, lmdv_i18n::t!("pal.gTabs").to_owned(), PalettePool::Tabs, rows);
                let mut recent: Vec<usize> =
                    (0..self.palette.files.len()).filter(|index| self.palette.files[*index].opened_at > 0).collect();
                recent.sort_by(|a, b| {
                    self.palette.files[*b].opened_at.cmp(&self.palette.files[*a].opened_at).then(a.cmp(b))
                });
                recent.truncate(START_FILES_CAP);
                push_group(&mut shown, &mut groups, lmdv_i18n::t!("pal.gFiles").to_owned(), PalettePool::Files, recent);
            }
            None => {
                mode = PaletteMode::All;
                let rows = self.palette_ranked(PalettePool::Commands, &words, COMMANDS_CAP);
                push_group(
                    &mut shown,
                    &mut groups,
                    lmdv_i18n::t!("pal.gCommands").to_owned(),
                    PalettePool::Commands,
                    rows,
                );
                let rows = self.palette_ranked(PalettePool::Headings, &words, HEADINGS_CAP);
                push_group(
                    &mut shown,
                    &mut groups,
                    lmdv_i18n::t!("pal.gHeadings").to_owned(),
                    PalettePool::Headings,
                    rows,
                );
                let rows = self.palette_ranked(PalettePool::Tabs, &words, TABS_CAP);
                push_group(&mut shown, &mut groups, lmdv_i18n::t!("pal.gTabs").to_owned(), PalettePool::Tabs, rows);
                let rows = self.palette_ranked(PalettePool::Files, &words, FILES_CAP);
                push_group(&mut shown, &mut groups, lmdv_i18n::t!("pal.gFiles").to_owned(), PalettePool::Files, rows);
            }
        }

        // The highlight starts on the best of all: the top of the group whose top
        // scores most (the earlier group of equals); in the outline, the heading
        // being read (C-05).
        let mut best = 0usize;
        let mut best_score = f64::NEG_INFINITY;
        let mut results = 0usize;
        for group in &groups {
            if mode == PaletteMode::All {
                let score = self.palette_score(shown[group.first]);
                if score > best_score {
                    best_score = score;
                    best = results;
                }
            }
            results += group.len;
        }
        if mode == PaletteMode::Outline {
            let current = (0..shown.len()).find(|index| {
                let shown = shown[*index];
                self.palette.pool(shown.pool).get(shown.index).is_some_and(|entry| entry.current)
            });
            best = current.unwrap_or(0);
        }

        // C-07: the search rows come after the normal groups.
        if mode == PaletteMode::All && !query.is_empty() {
            let mut search: Vec<PaletteEntry> = Vec::new();
            if self.palette_can_find() {
                let mut entry = PaletteEntry::new(
                    EntryKind::FindInDocument,
                    "s:doc".to_owned(),
                    lmdv_i18n::t!("pal.findInDoc", "q" => query.clone()),
                    EntryAction::FindInDocument(query.clone()),
                );
                entry.order = 0;
                search.push(entry);
            }
            if self.browser.root().is_some() {
                let mut entry = PaletteEntry::new(
                    EntryKind::SearchFolder,
                    "s:folder".to_owned(),
                    lmdv_i18n::t!("pal.searchFolder", "q" => query.clone()),
                    EntryAction::SearchFolder(query.clone()),
                );
                entry.order = 1;
                search.push(entry);
            }
            if !search.is_empty() {
                self.palette.search = search;
                let rows: Vec<usize> = (0..self.palette.search.len()).collect();
                push_group(&mut shown, &mut groups, lmdv_i18n::t!("pal.gSearch").to_owned(), PalettePool::Search, rows);
            }
        }

        let empty = if results == 0 && (!query.is_empty() || prefix.is_some()) {
            let asked = if query.is_empty() { prefix.map(|c| c.to_string()).unwrap_or_default() } else { query };
            lmdv_i18n::t!("pal.none", "q" => asked)
        } else {
            String::new()
        };
        let best = if results > 0 || !groups.is_empty() { best.min(shown.len().saturating_sub(1)) } else { 0 };
        self.palette.shown = shown;
        PaletteView { mode, groups, best, results, empty }
    }

    fn palette_score(&self, shown: ShownEntry) -> f64 {
        self.palette.pool(shown.pool).get(shown.index).map_or(f64::NEG_INFINITY, |entry| entry.score)
    }

    /// The rows of one pool in their given order.
    fn palette_in_order(&self, pool: PalettePool) -> Vec<usize> {
        let mut rows: Vec<usize> = (0..self.palette.pool(pool).len()).collect();
        rows.sort_by_key(|index| self.palette.pool(pool)[*index].order);
        rows
    }

    /// The rows of one pool that match every word, best first; equals keep their
    /// order (recent use, the document's, recency), then the shorter label.
    fn palette_ranked(&mut self, pool: PalettePool, words: &[String], cap: usize) -> Vec<usize> {
        let length = self.palette.pool(pool).len();
        let mut matched: Vec<(usize, f64, i64, usize)> = Vec::new();
        for index in 0..length {
            let entry = self.palette.pool_mut(pool).get_mut(index).expect("an index in range");
            let target = entry.target();
            let Some(found) = fuzzy::score(words, target) else { continue };
            entry.score = found.score;
            entry.hits = found.hits;
            matched.push((index, entry.score, entry.order, entry.label.chars().count()));
        }
        matched.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.2.cmp(&b.2)).then(a.3.cmp(&b.3))
        });
        matched.into_iter().take(cap).map(|(index, ..)| index).collect()
    }

    /// The whole outline, or for a longer one the part around the heading being
    /// read (C-04).
    fn palette_outline_around(&mut self, cap: usize) -> Vec<usize> {
        let rows = self.palette_in_order(PalettePool::Headings);
        if rows.len() <= cap {
            return rows;
        }
        let current = rows.iter().position(|index| self.palette.headings[*index].current).unwrap_or(0);
        let half = cap / 2;
        let from = current.saturating_sub(half).min(rows.len() - cap);
        rows.into_iter().skip(from).take(cap).collect()
    }

    /// The row's key, for the list's `aria-activedescendant`.
    fn palette_reveal(&mut self, tx: &mut Tx) {
        let Some(active) = self.palette.active else { return };
        let Some(entry) = self.palette.entry(active) else { return };
        let key = entry.key.clone();
        if self.palette.view.mode == PaletteMode::Outline {
            tx.scroll_center_in(PALETTE_LIST_KEY, key);
        } else {
            tx.scroll_into_view_in(PALETTE_LIST_KEY, key);
        }
    }

    // ---- navigation --------------------------------------------------------

    /// Up and Down, wrapping (C-08).
    fn palette_step(&mut self, direction: i32, tx: &mut Tx) {
        let count = self.palette.len();
        if count == 0 {
            return;
        }
        self.palette.chosen = true;
        let next = match self.palette.active {
            None if direction > 0 => 0,
            None => count - 1,
            Some(active) => {
                let step = if direction > 0 { active + 1 } else { active + count - 1 };
                step % count
            }
        };
        self.palette.active = Some(next);
        self.palette_reveal(tx);
    }

    /// Page Down: the next group's first row (the last row in the last group).
    /// Page Up: the start of this group, or of the one before. A list of one
    /// group (the outline) moves a page at a time (C-08).
    fn palette_step_page(&mut self, direction: i32, tx: &mut Tx) {
        let count = self.palette.len();
        if count == 0 {
            return;
        }
        let starts: Vec<usize> = self.palette.view.groups.iter().map(|group| group.first).collect();
        let current = self.palette.active.unwrap_or(0);
        let next = if starts.len() > 1 {
            let mut group = 0usize;
            while group + 1 < starts.len() && starts[group + 1] <= current {
                group += 1;
            }
            if direction > 0 {
                starts.get(group + 1).copied().unwrap_or(count - 1)
            } else if current > starts[group] {
                starts[group]
            } else {
                starts[group.saturating_sub(1)]
            }
        } else {
            let rows = self.palette_page_rows();
            let step = direction * rows;
            (current as i64 + i64::from(step)).clamp(0, count as i64 - 1) as usize
        };
        self.palette.chosen = true;
        self.palette.active = Some(next);
        self.palette_reveal(tx);
    }

    /// How many rows Page Up/Down move in a one-group list: a page of the drawn
    /// list, at least one row. The view reports the window's height; the list
    /// itself is capped below the top offset and the footer, which is what the
    /// page step measures here. Without a viewport (a test, the first frame) a
    /// page is ten rows.
    fn palette_page_rows(&self) -> i32 {
        let height = self.palette.viewport.get()[1];
        if height <= 0.0 {
            return 10;
        }
        let list_height = (height - crate::palette_ui::PALETTE_TOP - 170.0).clamp(120.0, 520.0);
        let rows = (list_height / PALETTE_ROW_HEIGHT) as i32 - 1;
        rows.max(1)
    }

    /// The tab and document in front, as the palette remembers them.
    fn palette_anchor(&self) -> Option<PaletteAnchor> {
        let tab = self.active_tab()?;
        let document = tab.document_id();
        let revision = tab.document().map_or(0, |document| document.revision());
        Some(PaletteAnchor { tab: tab.id, document, revision })
    }

    /// The heading being read, from the section line's last drawn path, else the
    /// document's first outline heading.
    fn palette_current_heading(&self) -> Option<usize> {
        if let Some(segment) = self.section_line.frame().segments.last() {
            return Some(segment.heading);
        }
        let tab = self.active_tab()?;
        let model = tab.view.reader.as_ref()?;
        crate::reader::toc_headings(model).next().map(|(index, _)| index)
    }

    /// Whether the row list can offer “Find … in the document” (C-07).
    fn palette_can_find(&self) -> bool {
        let Some(tab) = self.active_tab() else { return false };
        if tab.is_start() {
            return false;
        }
        tab.view.mode == Mode::Editor || tab.view.reader.is_some()
    }

    // ---- picking -----------------------------------------------------------

    /// Closes the palette, remembers a command's use and runs the row (C-06).
    pub(crate) fn palette_pick(&mut self, index: usize, where_: PaletteWhere, tx: &mut Tx) {
        let Some(entry) = self.palette.entry(index).cloned() else { return };
        self.palette.last_pick = Some(entry.action.clone());
        if let EntryAction::Command(id) = &entry.action {
            self.remember_palette_command(id);
        }
        self.close_palette();
        self.palette_activate(entry.action, where_, tx);
    }

    fn palette_activate(&mut self, action: EntryAction, where_: PaletteWhere, tx: &mut Tx) {
        match action {
            // Commands ignore the modifiers they were chosen with (C-06).
            EntryAction::Command(id) => {
                let _ = self.run_command(id, tx);
            }
            EntryAction::Heading(key) => self.jump_to_heading(&key, tx),
            EntryAction::Tab(tab) => {
                self.activate_tab_with_tx(tab, tx);
            }
            EntryAction::File(path) => self.open_browser_file(path, None, where_.mods(), tx),
            EntryAction::FindInDocument(query) => self.palette_find_in_document(&query, tx),
            EntryAction::SearchFolder(query) => self.search_folder_for(&query, tx),
        }
    }

    /// “Find “q” in the document”: the reading bar while reading, the editor's
    /// own bar while editing (C-07).
    fn palette_find_in_document(&mut self, query: &str, tx: &mut Tx) {
        let Some(tab) = self.active_tab() else { return };
        if tab.is_start() {
            return;
        }
        if tab.view.mode == Mode::Editor {
            self.open_find(false, tx);
            self.set_find_query(query);
            if let Some(tab) = self.active_tab_mut() {
                let length = tab.view.find_bar.query.text.len();
                tab.view.find_bar.query.set_selection(0, length);
            }
            tx.focus(crate::edit_ui::FIND_QUERY_KEY, false);
            return;
        }
        let tab = tab.id;
        self.prefill_reader_find(tab, query, true, tx);
    }

    /// Remembers a command run from the palette first, at most 8 (P-10).
    fn remember_palette_command(&mut self, id: &str) {
        let mut ui = self.shared.ui().clone();
        ui.palette_recent.retain(|recent| recent != id);
        ui.palette_recent.insert(0, id.to_owned());
        ui.palette_recent.truncate(RECENT_MAX);
        self.shared.set_ui(ui);
    }
}

/// Appends a group's rows to the flat list, in order.
fn push_group(
    shown: &mut Vec<ShownEntry>,
    groups: &mut Vec<PaletteGroup>,
    title: String,
    pool: PalettePool,
    rows: Vec<usize>,
) {
    if rows.is_empty() {
        return;
    }
    let first = shown.len();
    let len = rows.len();
    for index in rows {
        shown.push(ShownEntry { pool, index });
    }
    groups.push(PaletteGroup { title, first, len });
}

/// The page selection to restore when the palette closes (C-10).
///
/// Rune selects one text element at a time, so a selection across several of
/// them is remembered (and put back) at its first keyed text.
fn palette_selection(tx: &Tx) -> Option<(String, Range<usize>)> {
    let selection = tx.text_selection()?;
    selection
        .texts()
        .iter()
        .find_map(|text| text.key().filter(|_| !text.range().is_empty()).map(|key| (key.to_owned(), text.range())))
}

/// A file's folder as the palette shows it: relative to the browser's root when
/// the file is inside it, else its last two folders (`…/Projects/notes`).
pub fn folder_detail(path: &Path, root: Option<&Path>) -> String {
    if let Some(root) = root
        && let Ok(inside) = path.strip_prefix(root)
    {
        return join_components(inside.parent());
    }
    let mut parts = path_components(path.parent());
    let short = if parts.len() > 2 { parts.split_off(parts.len() - 2) } else { std::mem::take(&mut parts) };
    let joined = short.join("/");
    if parts.is_empty() { joined } else { format!("…/{joined}") }
}

fn path_components(path: Option<&Path>) -> Vec<String> {
    path.map(|path| {
        path.components()
            .filter_map(|part| match part {
                std::path::Component::RootDir | std::path::Component::Prefix(_) | std::path::Component::CurDir => None,
                part => Some(part.as_os_str().to_string_lossy().into_owned()),
            })
            .collect()
    })
    .unwrap_or_default()
}

fn join_components(path: Option<&Path>) -> String {
    path_components(path).join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_details_are_relative_to_the_root_or_the_last_two_folders() {
        let root = Path::new("/home/reader/docs");
        assert_eq!(folder_detail(Path::new("/home/reader/docs/a.md"), Some(root)), "");
        assert_eq!(folder_detail(Path::new("/home/reader/docs/notes/a.md"), Some(root)), "notes");
        assert_eq!(folder_detail(Path::new("/home/reader/docs/x/y/a.md"), Some(root)), "x/y");
        assert_eq!(folder_detail(Path::new("/other/projects/notes/a.md"), Some(root)), "…/projects/notes");
        assert_eq!(folder_detail(Path::new("/a.md"), None), "");
        assert_eq!(folder_detail(Path::new("/notes/a.md"), None), "notes");
    }

    #[test]
    fn chords_need_ctrl_without_alt_or_super() {
        let chord = |key: char, mods: Mods| KeyEvent { key: Key::Command(key), mods };
        let ctrl = Mods { cmd: true, ctrl: true, ..Mods::default() };
        assert!(PaletteChord::CtrlK.matches(&chord('k', ctrl)));
        assert!(!PaletteChord::CtrlK.matches(&chord('k', Mods { alt: true, ..ctrl })), "AltGr is left alone");
        assert!(!PaletteChord::CtrlK.matches(&chord('k', Mods { logo: true, ..ctrl })));
        assert!(!PaletteChord::CtrlShiftP.matches(&chord('p', ctrl)), "the plain Ctrl+P is not the palette");
        assert!(PaletteChord::CtrlShiftP.matches(&chord('p', Mods { shift: true, ..ctrl })));
    }

    #[test]
    fn placements_read_the_modifiers() {
        let key = |mods: Mods| KeyEvent { key: Key::Enter, mods };
        assert_eq!(PaletteWhere::from_key(&key(Mods::default())), PaletteWhere::Here);
        assert_eq!(
            PaletteWhere::from_key(&key(Mods { ctrl: true, cmd: true, ..Mods::default() })),
            PaletteWhere::BackgroundTab
        );
        assert_eq!(
            PaletteWhere::from_key(&key(Mods { ctrl: true, cmd: true, shift: true, ..Mods::default() })),
            PaletteWhere::ForegroundTab
        );
        assert_eq!(PaletteWhere::from_key(&key(Mods { shift: true, ..Mods::default() })), PaletteWhere::NewWindow);
    }
}
