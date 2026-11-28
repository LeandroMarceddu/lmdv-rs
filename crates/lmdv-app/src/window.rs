//! One window: its tabs, transient UI state, mailboxes and job completions.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use lmdv_core::{
    DocStatus, Document, DocumentId, DocumentKey, FileMeta, FindState, LEFT_WIDTH_DEFAULT, LEFT_WIDTH_MAX, NavHistory,
    RIGHT_WIDTH_DEFAULT, RIGHT_WIDTH_MAX, ScrollAnchor, SourceBuffer, TabId, WindowId,
};
use lmdv_io::Launch;
use lmdv_md::MdDoc;
use lmdv_render::{AreaWire, BlockWire, LinkWire, TaskControlWire};
use rune::{TextEdit, Tx, Waker};
use rune_kit::shell::{PaneState, TabDragState};

use crate::appearance::{Appearance, AppearanceKey, appearance};
use crate::images::{ImageCacheState, quiet_waker};
use crate::jobs::{GenerationTracker, JobMsg, WorkerMailbox};
use crate::math::MathCache;
use crate::reader;
use crate::reading::ReadingState;
use crate::shared::{AppShared, UiMailbox, UiMsg, unix_millis};
use crate::transfer::TransferState;
use crate::windows::{CloseDoc, WindowKind, WindowOpenRequest, WindowSnapshot, close_title};

/// How long a status notice stays visible, in virtual clock seconds.
pub const NOTICE_SECONDS: f64 = 4.0;

/// How long a tab opened behind keeps its Unread mark (T-15), in seconds. The
/// kit rings it once for 900 ms; the dot stays a little longer so a glance away
/// from the strip still finds the new tab.
pub const TAB_PULSE_SECONDS: f64 = 2.4;

/// How long a sticky notice stays: until the document changes, a save, a
/// reload, an explicit dismiss, or its tab closing (R-29/T-26).
/// `f64::INFINITY` keeps the ordinary deadline arithmetic honest.
pub const NOTICE_STICKY_SECONDS: f64 = f64::INFINITY;

/// A tab's rendering mode. `Editor` is legal only while the tab holds the
/// document's edit lease (D4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Read-only rendered view (source preview only on a render failure).
    #[default]
    Viewer,
    /// Source editor on the left, live preview on the right (E-01/E-02).
    Editor,
}

/// What a tab shows.
#[derive(Clone, Debug)]
pub enum TabSource {
    /// The start page: a tab, never a document.
    Start,
    /// A file or pasted document shared process-wide.
    Document(Rc<Document>),
}

impl TabSource {
    /// The document, when this is a document tab.
    pub fn document(&self) -> Option<&Rc<Document>> {
        match self {
            Self::Start => None,
            Self::Document(doc) => Some(doc),
        }
    }

    /// The document id, when this is a document tab.
    pub fn document_id(&self) -> Option<DocumentId> {
        self.document().map(|doc| doc.id)
    }

    /// Whether this is the start page.
    pub fn is_start(&self) -> bool {
        matches!(self, Self::Start)
    }
}

/// One background tab's T-15 mark: a kit Unread dot and one ring, until it is
/// activated or the interval ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabPulse {
    /// The ring's sequence: the kit re-rings a tab whose value changes.
    pub seq: u32,
    /// Virtual-clock time at which the mark is cleared.
    pub until: f64,
}

/// Per-tab view state that must survive tab switches.
///
/// Scroll, `--line`, outline visibility, the outline filter, the reading find
/// bar, the Back/Forward places, the tab's own sticky notice and the scheme its
/// last frame drew with are per tab; editor state joins them in edit mode.
#[derive(Clone, Debug, Default)]
pub struct TabView {
    pub mode: Mode,
    pub anchor: ScrollAnchor,
    pub scroll_target: f32,
    /// A `--line` or link target, 1-based, consumed by the viewer.
    pub line: Option<usize>,
    pub find: Option<FindState>,
    /// The reading find bar's query, hits and current hit (R-16–R-19).
    pub reader_find: crate::reader_find::ReaderFind,
    /// The tab's Back/Forward places (O-12–O-15).
    pub nav: NavHistory,
    /// A `file.md#section` link's heading, applied once the document is parsed.
    pub fragment: Option<String>,
    pub toc_filter: String,
    /// The tab's own sticky notice (T-26/R-29): an empty/error fact about this
    /// document stays with it while another tab is in front and comes back
    /// with it. Transient confirmations are the window's and never reach here.
    pub notice: Option<Notice>,
    /// The scheme this tab's last frame was rendered with (T-26). The theme is
    /// a live window preference (J-01), so the front tab records the current
    /// scheme at every render; a hidden tab keeps the one it last drew with
    /// until it is shown again.
    pub scheme: Cell<Option<AppearanceKey>>,
    /// The last good parsed model: the reader draws it while a newer revision
    /// is parsed on a worker (M-34 keeps the previous article on failure).
    pub reader: Option<Arc<MdDoc>>,
    /// The revision `reader` was parsed from; `None` while a (re)load waits for
    /// its first model.
    pub reader_revision: Option<u64>,
    /// Why the current revision could not be parsed (render failures and
    /// oversized documents keep the source preview).
    pub reader_error: Option<String>,
    /// The open `<details>` and front-matter cards, by key (M-22).
    pub details_open: BTreeSet<String>,
    /// The user closed the outline for this tab (O-06); the pane stays closed
    /// across switches until they open it again.
    pub outline_hidden: bool,
    /// The tab's editor session; present only in [`Mode::Editor`].
    pub editor: Option<lmdv_edit::EditorSession>,
    /// The find/replace bar's widgets, kept per tab (E-20).
    pub find_bar: crate::edit_ui::FindBar,
    /// The `/` block menu, kept per tab (E-28).
    pub slash_menu: crate::edit_ui::SlashMenu,
    /// The text the buffer was loaded with when editing started (E-13).
    pub edit_base: Option<String>,
    /// Pasted text as it was pasted, for T-07's reopen: “Don't save” must not
    /// bring discarded edits back through Ctrl+Shift+T.
    pub pasted_base: Option<String>,
    /// Cached dirty flag: hashing the whole buffer every frame would be wrong.
    pub dirty: bool,
    /// An edit waits for the preview debounce (E-05).
    pub preview_pending: bool,
    /// Virtual-clock time at which the preview may parse.
    pub preview_due: Option<f64>,
    /// An edit waits for the draft debounce (E-16).
    pub draft_pending: bool,
    /// Virtual-clock time at which the draft is written.
    pub draft_due: Option<f64>,
    /// This tab already wrote a draft that must be removed when clean.
    pub draft_written: bool,
    /// The watcher saw a change on disk for a dirty buffer.
    pub disk_changed: bool,
    /// The document's reading time (R-04), computed when its model lands while
    /// reading; the hover card and description keep it while editing.
    pub reading: Option<crate::reading_meta::ReadingStats>,
    /// The reader revision `reading` was computed from.
    pub reading_revision: Option<u64>,
    /// The reading time is drawn after the tab's name; this remembers whether it
    /// is on, so the strip's 2-point margin does not make it flicker (T-35).
    pub reading_meta: std::cell::Cell<bool>,
    /// A tab opened behind lights up briefly (T-15); activation clears it.
    pub pulse: Option<TabPulse>,
}

/// One tab in a window.
#[derive(Clone, Debug)]
pub struct Tab {
    pub id: TabId,
    pub source: TabSource,
    pub view: TabView,
    title_cache: String,
}

impl Tab {
    /// The start page, titled “New tab” like the original.
    pub fn start(id: TabId) -> Self {
        Self {
            id,
            source: TabSource::Start,
            view: TabView::default(),
            title_cache: lmdv_i18n::t!("tab.newTab").to_owned(),
        }
    }

    /// A document tab. The parsed model is adopted by `WindowState` after the
    /// tab is attached, from the shared parse cache.
    pub fn for_document(id: TabId, doc: Rc<Document>) -> Self {
        let mut tab =
            Self { id, source: TabSource::Document(doc.clone()), view: TabView::default(), title_cache: String::new() };
        tab.refresh_title();
        tab
    }

    /// The document, when this is a document tab.
    pub fn document(&self) -> Option<&Rc<Document>> {
        self.source.document()
    }

    /// The document id, when this is a document tab.
    pub fn document_id(&self) -> Option<DocumentId> {
        self.source.document_id()
    }

    /// Whether this is the start page.
    pub fn is_start(&self) -> bool {
        self.source.is_start()
    }

    /// The strip label: the file name stem for files, the full title for pasted
    /// content, “New tab” for the start page (T-02).
    pub fn title(&self) -> String {
        let full = self.full_title();
        match &self.source {
            TabSource::Start => full,
            TabSource::Document(doc) if matches!(*doc.key.borrow(), DocumentKey::File(_)) => Path::new(&full)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|stem| !stem.is_empty())
                .map(str::to_owned)
                .unwrap_or(full),
            TabSource::Document(_) => full,
        }
    }

    /// The full name, for the toolbar, hover card and accessible label.
    pub fn full_title(&self) -> String {
        self.title_cache.clone()
    }

    /// The accessible description: path for files, the start-page hint otherwise.
    pub fn tooltip(&self) -> String {
        match &self.source {
            TabSource::Start => lmdv_i18n::t!("tab.startPage").to_owned(),
            TabSource::Document(doc) => {
                doc.key.borrow().path().map(|path| path.display().to_string()).unwrap_or_else(|| self.full_title())
            }
        }
    }

    /// Refreshes the cached title from the document.
    pub fn refresh_title(&mut self) {
        self.title_cache = match &self.source {
            TabSource::Start => lmdv_i18n::t!("tab.newTab").to_owned(),
            TabSource::Document(doc) => {
                let title = doc.title.borrow();
                if title.trim().is_empty() { lmdv_i18n::t!("tab.untitled").to_owned() } else { title.clone() }
            }
        };
    }
}

/// A status notice's severity (H-06): the bar's icon and colour, and how a
/// screen reader is told (H-07/A-10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoticeKind {
    /// A neutral status: a refusal, or something to know.
    #[default]
    Info,
    /// A completed action: the save, a copied path or table.
    Success,
    /// Something was refused or is degraded, but the window still works.
    Warning,
    /// A failed action: read at once, as `role=alert`.
    Error,
}

impl NoticeKind {
    /// Errors interrupt: the bar is `role=alert` with an assertive live region.
    pub const fn is_error(self) -> bool {
        matches!(self, Self::Error)
    }
}

/// The confirmations raised through [`WindowState::notice`] by the handlers
/// that predate notice kinds: the save and the three copy actions (H-06).
///
/// Matching the catalog message keeps those call sites honest without a second
/// severity argument at every one of them; new code names its kind instead.
fn is_confirmation(text: &str) -> bool {
    ["msg.saved", "msg.pathCopied", "msg.tableCopied", "msg.imageCopied"]
        .iter()
        .any(|name| text == lmdv_i18n::text(lmdv_i18n::key(name)))
}

/// What a status notice says and when it disappears.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    /// Virtual clock seconds from `poll`; notices set before the first frame
    /// expire four seconds in. `f64::INFINITY` marks a sticky notice (R-29).
    pub deadline: f64,
    /// An action the notice offers, such as the task tick's Undo.
    pub action: Option<NoticeAction>,
    /// The notice's severity (H-06).
    pub kind: NoticeKind,
    /// The number of notices this window has shown (H-07). The bar alternates
    /// an invisible mark on it, so the same words twice are announced twice.
    pub seq: u64,
}

impl Notice {
    /// Whether this notice waits for the document or the user (R-29) instead
    /// of expiring on its own deadline.
    pub fn is_sticky(&self) -> bool {
        !self.deadline.is_finite()
    }
}

/// An action a notice offers. The payload is UI state, never a live job.
#[derive(Clone, Debug, PartialEq)]
pub enum NoticeAction {
    /// Reverses one viewer task tick and writes the file again.
    TaskUndo(crate::tasks::TaskUndo),
    /// Opens a folder in the desktop's file manager (X-05's Show in folder).
    ShowInFolder(PathBuf),
    /// Opens a non-Markdown file with the desktop's handler (L-07's Open).
    OpenFile(PathBuf),
    /// Asks for a path for a pasted document (L-09's Save as…).
    SaveAs(DocumentId),
    /// Re-runs `Print…` after a PDF failure (X-05's Use the print dialog).
    UsePrintDialog,
}

impl NoticeAction {
    /// The action button's label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::TaskUndo(_) => lmdv_i18n::t!("msg.undo"),
            Self::ShowInFolder(_) => lmdv_i18n::t!("msg.showInFolder"),
            Self::OpenFile(_) => lmdv_i18n::t!("msg.openFile"),
            Self::SaveAs(_) => lmdv_i18n::t!("tab.saveAs"),
            Self::UsePrintDialog => lmdv_i18n::t!("msg.usePrintDialog"),
        }
    }
}

/// Which Help surface is in front: the quick key list (`?`, `help.keys`) or
/// the full localized guide (F1, `app.help`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpView {
    /// The compact command/shortcut list.
    Shortcuts,
    /// The full help page from `locales/help`.
    Full,
}

/// Transient per-window UI state. It is not persisted.
#[derive(Debug, Default)]
pub struct UiState {
    /// The current status notice, if any.
    pub notice: Option<Notice>,
    /// How many notices this window has shown (H-07's clear-before-write).
    pub(crate) notice_seq: u64,
    /// The open Help surface, if any (`None`: no Help dialog).
    pub help: Option<HelpView>,
    /// The last dispatched command id (palette recents, debugging).
    pub last_command: Option<String>,
    /// Wall-clock milliseconds for the start page's relative times.
    pub now_unix_ms: u64,
    /// A command asked for the window to close; the next `poll` issues it.
    pub close_requested: bool,
    /// A command asked for the app to quit; the next `poll` issues it.
    pub quit_requested: bool,
    /// The preview block whose context menu is open (E-04).
    pub block_menu: Option<crate::preview_nav::BlockMenu>,
    /// The link whose context menu is open (OI-195).
    pub link_menu: Option<crate::links::LinkMenu>,
    /// A background tab to reveal in the strip on the next poll (T-15).
    pub(crate) reveal_tab: Option<TabId>,
    /// Clock time at which a changed session fragment is written.
    pub(crate) session_due: Option<f64>,
    pub(crate) last_now: f64,
    pub(crate) last_title: String,
    /// The element holding the keyboard, as `Tx::focused()` names it at poll
    /// time (OI-141).
    pub(crate) focused_key: Option<String>,
    /// The keyboard moved the focus (`focus-visible`), not a press.
    pub(crate) focus_visible: bool,
    /// A notice's **Save as…** action was pressed; the next poll opens the
    /// dialog for that pasted document, where a `Tx` exists (L-09).
    pub(crate) save_as_requested: Option<DocumentId>,
}

/// A file drag from another app over this window (D-01, D-02, H-12).
///
/// The hover flag drives the “Drop to open” card. The strip geometry is what
/// the last frame laid out, in window points, so a drop can turn its x into a
/// tab index: the kit keys every tab slot `t-{id}`, and `Cx::window_layout`
/// reads those rects. A strip of more than forty tabs builds only the ones near
/// its viewport; the index is exact on every built tab and resolves to a built
/// tab's neighbour otherwise.
#[derive(Debug, Default)]
pub struct DropState {
    /// A file drag this window takes is over it (D-01/H-12).
    over: bool,
    /// The tab band's rect, from the last frame.
    strip: Cell<Option<[f32; 4]>>,
    /// Each built tab's strip index and rect, from the last frame.
    tabs: RefCell<Vec<(usize, [f32; 4])>>,
}

impl DropState {
    /// Whether a drag of Markdown files is over the window (H-12).
    pub fn over(&self) -> bool {
        self.over
    }

    /// The hover the root's `on_drop_hover` reports (H-12).
    pub(crate) fn set_over(&mut self, over: bool) {
        self.over = over;
    }

    /// Remembers the strip's and its built tabs' last-frame rects (D-02).
    pub(crate) fn record_geometry(&self, strip: Option<[f32; 4]>, tabs: &[(usize, [f32; 4])]) {
        self.strip.set(strip);
        let mut slot = self.tabs.borrow_mut();
        slot.clear();
        slot.extend_from_slice(tabs);
    }

    /// The strip index a drop at `(x, y)` in window points inserts at: the
    /// first tab whose middle is right of `x`, or just past the last built one
    /// (D-02). `None` when the point is not on the tab band.
    pub(crate) fn strip_index(&self, x: f32, y: f32) -> Option<usize> {
        let strip = self.strip.get()?;
        if x < strip[0] || x > strip[0] + strip[2] || y < strip[1] || y > strip[1] + strip[3] {
            return None;
        }
        let tabs = self.tabs.borrow();
        if tabs.is_empty() {
            return None;
        }
        Some(
            tabs.iter()
                .find(|(_, rect)| x < rect[0] + rect[2] * 0.5)
                .map_or_else(|| tabs.last().map_or(0, |(index, _)| index + 1), |(index, _)| *index),
        )
    }
}

/// Where an open places its tab (D-02, T-13/T-14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenPlace {
    /// The normal open: in front, replacing a front start page (T-13).
    Front,
    /// T-14's background open: behind the front tab, and after the last
    /// background tab already queued from it.
    Background,
    /// At a strip index, in front (D-02's first dropped file).
    At(usize),
    /// At a strip index, behind the front tab (D-02's later files).
    AtBackground(usize),
}

impl OpenPlace {
    /// The strip index to insert at, when this placement names one.
    fn index(self) -> Option<usize> {
        match self {
            Self::At(index) | Self::AtBackground(index) => Some(index),
            Self::Front | Self::Background => None,
        }
    }

    /// Whether the tab joins behind the front tab.
    fn background(self) -> bool {
        matches!(self, Self::Background | Self::AtBackground(_))
    }
}

/// The result of closing a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseOutcome {
    /// The tab was removed (and a start page added if it was the last one).
    Closed,
    /// The tab was the lone start page, so the window should close instead.
    WindowClose,
    /// No tab with that id exists.
    NotFound,
    /// An export in flight pins this document's tab, so the close was refused
    /// (OI-105).
    Blocked,
}

/// E-15(c): puts an editor session's caret at the start of `line` (1-based),
/// without changing the buffer.
fn aim_editor_caret(editor: &mut lmdv_edit::EditorSession, line: usize) -> bool {
    let text = editor.text();
    let start = text.split('\n').take(line.saturating_sub(1)).map(|line| line.len() + 1).sum::<usize>().min(text.len());
    editor.set_selection(start..start);
    true
}

/// The application state of one window.
///
/// This is the `S` Rune's `AppSpec` runs. It is also the interface later slices
/// (viewer, editor, session restore) build on.
pub struct WindowState {
    pub(crate) shared: Rc<AppShared>,
    id: WindowId,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    /// The file-browser pane's visibility and width; its content lives in
    /// [`Self::browser`].
    pub left: PaneState,
    /// The file browser's root, rows, collapsed folders and folder search
    /// (F-01–F-14).
    pub browser: crate::browser::BrowserState,
    /// The folder-search field (its text is the query while typing).
    pub browser_search: TextEdit,
    /// The outline pane, built from the parsed headings.
    pub right: PaneState,
    /// Tab-strip gestures.
    pub drag: TabDragState,
    /// A file drag from another app, and the strip's last-frame geometry (D-01,
    /// D-02, H-12).
    pub drop: DropState,
    /// A torn-out tab's grab point and hover target (P4-02c).
    pub(crate) tear: crate::tear_out::TearState,
    /// Transient UI state.
    pub ui: UiState,
    /// Reading positions and the `--line` flash.
    pub(crate) reading: ReadingState,
    /// The reader's `Ctrl`+wheel/pinch accumulator (F-21).
    pub(crate) reader_magnify: f32,
    /// The reader's single-key state: the j/k target, the `g g` window and the
    /// pinned heading (R-06–R-09/O-04).
    pub(crate) reading_keys: crate::reading_keys::ReadingKeys,
    /// Middle-drag and empty-margin drag-to-scroll (D-06/R-28).
    pub(crate) reader_pan: crate::reader::ScrollPan,
    /// The section line's path, timers and double-press guard (O-07–O-10).
    pub(crate) section_line: crate::section::SectionLineState,
    /// The outline's filter field; its text mirrors the active tab's `toc_filter`.
    pub(crate) outline_filter: TextEdit,
    worker: Arc<WorkerMailbox>,
    ui_inbox: Rc<UiMailbox>,
    loads: GenerationTracker,
    /// Parse jobs in flight, by document, with the revision requested.
    pub(crate) parse_inflight: HashMap<DocumentId, u64>,
    /// The current `DocTheme`/`MarkdownStyle`, rebuilt only when the key changes.
    appearance: RefCell<Option<(AppearanceKey, Rc<Appearance>)>>,
    /// Formula rendering and its image/error caches (OI-020, OI-021).
    pub(crate) math: MathCache,
    /// The window's `rune::ImageCache`, created on the first picture.
    images: OnceCell<ImageCacheState>,
    /// The UI waker, installed by `AppSpec::setup`; the image cache needs it.
    waker: RefCell<Option<Waker>>,
    /// One link handler per window, shared by every article frame.
    link_wire: LinkWire,
    /// The editor preview's link handler: links open, but a secondary press
    /// stays the block's (E-04), so OI-195's menu is the reader's alone.
    preview_link_wire: LinkWire,
    /// The modifiers the window last reported, for link-click placement (L-11).
    link_mods: rune::Mods,
    /// One task-checkbox control handler per window, attached only to viewer
    /// articles.
    task_control_wire: TaskControlWire,
    /// One block handler per window: preview double-click and context menu.
    block_wire: BlockWire,
    /// One image-map `<area>` handler per window (L-12), attached to viewer
    /// articles only.
    area_wire: AreaWire,
    /// In-flight viewer task writes and their stale guards.
    pub(crate) tasks: crate::tasks::TaskState,
    /// Edit mode, save/conflict state, drafts and the file watcher's decisions.
    pub(crate) editor: crate::editor::EditorState,
    /// The Share menu, export job and its 1.2 s check (P5-04).
    pub share: crate::share::ShareState,
    /// The table action bar's target and the open table preview (P6-T).
    pub tables: crate::tables::TableState,
    /// The command palette's modal state (C-01–C-13, P-10).
    pub palette: crate::palette::PaletteState,
    /// The strip's all-tabs list (T-17).
    pub(crate) tab_list: crate::tab_list::TabListState,
    /// The hovered tab and its floating card (T-16).
    pub tab_card: crate::tab_card::TabCardState,
    /// The Back/Forward move in flight and its recording suppression (O-12–O-15).
    pub(crate) navigation: crate::nav::NavigationState,
    /// This window's tabs in the order they were last shown, newest first; the
    /// palette lists the other ones in that order (C-02).
    pub(crate) tab_recent: Vec<TabId>,
    /// The open full-window media preview (V-01–V-10); one per window.
    pub(crate) lightbox: Option<crate::lightbox::Lightbox>,
    /// The settings dialog's transient state (P6-PREF).
    pub settings: crate::settings::SettingsState,
    /// This window is kept on top of other windows (`view.pin`, X11).
    pub pinned: bool,
    /// The closed tabs Ctrl+Shift+T reopens and the close-others/right queue
    /// (T-07/T-18/T-23).
    pub(crate) tab_actions: crate::actions::TabActionsState,
    /// Tabs sent to or arriving from another window (P4-02b).
    pub(crate) transfer: TransferState,
    /// Tabs whose stored edit mode waits for their document (P4-02d).
    pub(crate) pending_editor_tabs: BTreeSet<TabId>,
    /// The session factory is replaying stored tabs in order: a file after a
    /// restored start page appends instead of replacing it, and reopening a
    /// file does not count as a new recent.
    pub(crate) restoring_tabs: bool,
    /// Documents whose first load came from the session; their completion must
    /// not rewrite the recent-file list.
    pub(crate) restored_docs: BTreeSet<DocumentId>,
    /// This window already wrote its session entry on close/quit.
    pub(crate) session_flushed: bool,
    /// The session fragment this window last published (OI-151).
    pub(crate) session_publish: RefCell<crate::session::PublishedSession>,
    /// The documents whose tabs show T-03's post-save check: those written by
    /// the save that armed `editor.saved_flash_until`.
    pub(crate) saved_flash_docs: BTreeSet<DocumentId>,
    /// T-14: the last background tab this window opened. The next background
    /// open joins after it; any tab coming forward forgets the queue.
    pub(crate) background_queue: Option<TabId>,
    /// The next T-15 ring's sequence; 0 is the kit's “no ring”.
    pub(crate) tab_pulse_seq: u32,
    next_tab: TabId,
}

impl WindowState {
    /// Creates a window with one start-page tab and registers it process-wide.
    pub fn new(shared: Rc<AppShared>) -> Self {
        let id = shared.allocate_window();
        Self::new_keyed(shared, id, crate::windows::window_key(id))
    }

    /// The open Help surface, if any (`None`: no Help dialog is showing).
    pub fn help_view(&self) -> Option<HelpView> {
        self.ui.help
    }

    /// Opens Help on `view`, replacing any other Help surface.
    pub fn open_help(&mut self, view: HelpView) {
        self.ui.help = Some(view);
    }

    /// Closes the Help dialog, whichever surface it shows.
    pub fn close_help(&mut self) {
        self.ui.help = None;
    }

    /// Creates a window with an explicit id and registry key.
    ///
    /// The key is fixed before the host exists so the registry, menus and the
    /// session name the same window from birth. The id comes from
    /// [`AppShared::allocate_window`].
    pub(crate) fn new_keyed(shared: Rc<AppShared>, id: WindowId, key: String) -> Self {
        Self::new_keyed_kind(shared, id, key, WindowKind::Normal)
    }

    /// [`Self::new_keyed`] with the kind the session will restore.
    pub(crate) fn new_keyed_kind(shared: Rc<AppShared>, id: WindowId, key: String, kind: WindowKind) -> Self {
        let worker = Arc::new(WorkerMailbox::new(id));
        let ui_inbox = Rc::new(UiMailbox::new());
        shared.register_window(id, key, ui_inbox.clone(), "lmdv", kind);

        let mut state = Self {
            shared,
            id,
            tabs: Vec::new(),
            active: 0,
            left: PaneState::new(f32::from(LEFT_WIDTH_DEFAULT))
                .range(f32::from(LEFT_WIDTH_DEFAULT), f32::from(LEFT_WIDTH_MAX))
                .collapsed(),
            browser: crate::browser::BrowserState::default(),
            browser_search: TextEdit::default(),
            right: PaneState::new(f32::from(RIGHT_WIDTH_DEFAULT))
                .range(f32::from(RIGHT_WIDTH_DEFAULT), f32::from(RIGHT_WIDTH_MAX))
                .collapsed(),
            drag: TabDragState::new(),
            drop: DropState::default(),
            tear: crate::tear_out::TearState::default(),
            ui: UiState::default(),
            reading: ReadingState::default(),
            reader_magnify: 0.0,
            reading_keys: crate::reading_keys::ReadingKeys::default(),
            reader_pan: crate::reader::ScrollPan::default(),
            section_line: crate::section::SectionLineState::default(),
            outline_filter: TextEdit::default(),
            worker,
            ui_inbox,
            loads: GenerationTracker::default(),
            parse_inflight: HashMap::new(),
            appearance: RefCell::new(None),
            math: MathCache::new(),
            images: OnceCell::new(),
            waker: RefCell::new(None),
            link_wire: Rc::new(|e: rune::El| {
                e.on_link(|state: &mut WindowState, dest: &str, tx: &mut Tx| state.open_link(dest, tx))
                    .on_link_secondary(|state: &mut WindowState, dest: &str, press: rune::PointerPress, _: &mut Tx| {
                        state.show_link_menu(dest, press);
                    })
            }),
            preview_link_wire: Rc::new(|e: rune::El| {
                e.on_link(|state: &mut WindowState, dest: &str, tx: &mut Tx| state.open_link(dest, tx))
            }),
            link_mods: rune::Mods::default(),
            task_control_wire: crate::tasks::task_control_wire(),
            block_wire: crate::preview_nav::block_wire(),
            area_wire: Rc::new(|element: rune::El, area: &lmdv_render::AreaRef| {
                let dest = area.dest.clone();
                element.on_press_tx(move |state: &mut WindowState, tx: &mut Tx| state.open_link(&dest, tx))
            }),
            tasks: crate::tasks::TaskState::default(),
            editor: crate::editor::EditorState::default(),
            share: crate::share::ShareState::default(),
            tables: crate::tables::TableState::default(),
            palette: crate::palette::PaletteState::default(),
            tab_list: crate::tab_list::TabListState::default(),
            tab_card: crate::tab_card::TabCardState::default(),
            navigation: crate::nav::NavigationState::default(),
            tab_recent: Vec::new(),
            lightbox: None,
            settings: crate::settings::SettingsState::default(),
            pinned: false,
            tab_actions: crate::actions::TabActionsState::default(),
            transfer: TransferState::default(),
            pending_editor_tabs: BTreeSet::new(),
            restoring_tabs: false,
            restored_docs: BTreeSet::new(),
            session_flushed: false,
            session_publish: RefCell::new(crate::session::PublishedSession::default()),
            saved_flash_docs: BTreeSet::new(),
            background_queue: None,
            tab_pulse_seq: 1,
            next_tab: 1,
        };
        state.push_start_tab();
        state.note_active_tab();
        // F-07: collapsed folders are global UI state; the browser root is
        // restored so the pane shows the last folder on startup.
        let ui = state.shared.ui().clone();
        state.browser.collapsed =
            ui.collapsed_folders.iter().map(|folder| crate::browser::collapse_key(folder)).collect();
        if let Some(root) = ui.tree_root.clone() {
            state.adopt_browser_root_for_startup(root);
        }
        if let Some(error) = state.shared.store_error() {
            let message = lmdv_i18n::t!("msg.settingsSaveFailed", "err" => error);
            state.notice_kind(message, NoticeKind::Error);
        }
        // OI-007/OI-012: a portable root that fell back to XDG and every state
        // file that was quarantined or left read-only are startup facts; the
        // first window says them once.
        let startup = state.shared.take_startup_notices();
        if !startup.is_empty() {
            match state.ui.notice.as_mut() {
                Some(existing) => {
                    existing.text.push(' ');
                    existing.text.push_str(&startup.join(" "));
                }
                None => state.notice_kind(startup.join(" "), NoticeKind::Warning),
            }
        }
        let _ = state.publish_session();
        state
    }

    /// Creates a window for a parsed launch: the start page, or the requested
    /// file with its pending line.
    pub fn start(shared: Rc<AppShared>, launch: &Launch) -> Self {
        let id = shared.allocate_window();
        let mut state = Self::new_keyed(shared, id, crate::windows::window_key(id));
        state.open_launch(launch);
        state.sync_registry();
        let _ = state.publish_session();
        state
    }

    /// [`Self::start`] with an explicit registry key, for the headless
    /// two-window gate's `second` window.
    pub(crate) fn start_keyed(shared: Rc<AppShared>, launch: &Launch, key: String) -> Self {
        let id = shared.allocate_window();
        let mut state = Self::new_keyed(shared, id, key);
        state.open_launch(launch);
        // The launch's file is already open before the first frame; publish it
        // so launch routing sees the new window as a holder, not an idle one.
        state.sync_registry();
        let _ = state.publish_session();
        state
    }

    /// The window identity.
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// Applies a launch in this window: open the file and its pending line.
    ///
    /// A launch without a file only brings the window forward; the start page
    /// stays as it is. An export can leave the new tab in the background
    /// (OI-105), so the line target goes to the tab that was opened rather than
    /// to whichever tab is in front.
    pub fn open_launch(&mut self, launch: &Launch) {
        let Some(path) = launch.path.clone() else { return };
        let tab = self.open_path(path);
        if let Some(line) = launch.line {
            if self.active_tab().map(|active| active.id) == Some(tab) {
                self.set_pending_line(line);
            } else if let Some(tab) = self.tabs.iter_mut().find(|candidate| candidate.id == tab) {
                tab.view.line = Some(line.max(1));
            }
        }
    }

    /// The process-wide state.
    pub fn shared(&self) -> &AppShared {
        &self.shared
    }

    /// A clone of the shared `Rc`, for callbacks that outlive a borrow of the
    /// window (the native-created hook).
    pub(crate) fn shared_rc(&self) -> Rc<AppShared> {
        Rc::clone(&self.shared)
    }

    /// The job mailbox for this window.
    pub fn worker(&self) -> &Arc<WorkerMailbox> {
        &self.worker
    }

    /// The same-thread cross-window inbox.
    pub fn ui_inbox(&self) -> &Rc<UiMailbox> {
        &self.ui_inbox
    }

    /// Installs the window waker; the image cache is built with it later.
    pub fn attach_waker(&self, waker: Waker) {
        *self.waker.borrow_mut() = Some(waker);
    }

    /// The reading appearance for this frame, cached until the scheme or a
    /// reader preference changes (OI-019).
    ///
    /// T-26: the front tab records the scheme it is drawn with, so switching
    /// back to it restores the look it last had. The theme itself stays a live
    /// window preference (J-01): a tab shown after a theme change draws the new
    /// scheme and records that one.
    pub(crate) fn current_appearance(&self, system_dark: bool) -> Rc<Appearance> {
        let key = AppearanceKey::for_prefs(&self.shared.prefs(), system_dark);
        if let Some(tab) = self.tabs.get(self.active) {
            tab.view.scheme.set(Some(key));
        }
        let mut slot = self.appearance.borrow_mut();
        if let Some((cached, look)) = slot.as_ref()
            && *cached == key
        {
            return look.clone();
        }
        let look = appearance(key);
        *slot = Some((key, look.clone()));
        look
    }

    /// The image-cache slot; the view fills it on the first picture.
    pub(crate) fn image_cell(&self) -> &OnceCell<ImageCacheState> {
        &self.images
    }

    /// Drops the cached reading appearance, so the next frame derives it again
    /// from the current preferences.
    pub(crate) fn invalidate_appearance(&self) {
        self.appearance.borrow_mut().take();
    }

    /// The waker an image cache would use, with a quiet default for tests.
    pub(crate) fn window_waker(&self) -> Waker {
        self.waker.borrow().clone().unwrap_or_else(quiet_waker)
    }

    /// The shared link wire.
    pub(crate) fn cached_link_wire(&self) -> &LinkWire {
        &self.link_wire
    }

    /// The editor preview's link wire (E-04 keeps its secondary press).
    pub(crate) fn cached_preview_link_wire(&self) -> &LinkWire {
        &self.preview_link_wire
    }

    /// The modifiers this window last reported, for link-click placement.
    pub(crate) fn link_modifiers(&self) -> rune::Mods {
        self.link_mods
    }

    /// Records the held modifiers; `WindowSpec::on_modifiers` calls this.
    pub(crate) fn set_link_modifiers(&mut self, mods: rune::Mods) {
        self.link_mods = mods;
    }

    /// The shared task-control wire, used only by viewer articles.
    pub(crate) fn cached_task_control_wire(&self) -> &TaskControlWire {
        &self.task_control_wire
    }

    /// The shared block wire: E-35's double press and E-04's context menu.
    pub(crate) fn cached_block_wire(&self) -> &BlockWire {
        &self.block_wire
    }

    /// The one image-map handler this window's article uses (L-12).
    pub(crate) fn area_wire(&self) -> AreaWire {
        self.area_wire.clone()
    }

    /// The document's folder, for relative links and images.
    pub(crate) fn document_root(&self, doc: &Document) -> Option<PathBuf> {
        let key = doc.key.borrow();
        key.path().and_then(Path::parent).map(Path::to_path_buf)
    }

    /// Every tab, left to right.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The active tab index.
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The active tab.
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The active tab, mutably.
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// A tab by id.
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    /// The document in front, for launch routing.
    pub fn active_document_id(&self) -> Option<DocumentId> {
        self.active_tab().and_then(Tab::document_id)
    }

    /// Whether `tab` shows T-03's check: it was written by the save that armed
    /// the saved flash and has no edits since. Typing clears it (the dirty dot
    /// takes the slot back), and the flash's expiry ends it in [`Self::poll`].
    pub fn tab_shows_saved_check(&self, tab: &Tab) -> bool {
        !tab.view.dirty
            && self.editor.saved_flash_until.is_some()
            && tab.document_id().is_some_and(|doc| self.saved_flash_docs.contains(&doc))
    }

    /// Allocates a tab id that this window never reuses.
    pub(crate) fn allocate_tab(&mut self) -> TabId {
        let id = self.next_tab;
        self.next_tab = self.next_tab.saturating_add(1);
        id
    }

    /// Appends a start page at the end and brings it forward (T-04).
    pub fn new_start_tab(&mut self) -> TabId {
        self.insert_start_tab(None)
    }

    /// Opens a start page right after `tab` and brings it forward: the tab
    /// menu's New tab inserts after the tab it was opened on (T-04), while
    /// `Ctrl+T` still appends at the end.
    pub fn new_start_tab_after(&mut self, tab: TabId) -> TabId {
        let at = self.tabs.iter().position(|candidate| candidate.id == tab).map(|index| index + 1);
        self.insert_start_tab(at)
    }

    /// [`Self::new_start_tab`] at `at`, or at the end when it is gone.
    ///
    /// While an export pins a document, the new start page is inserted in the
    /// background: the exporting tab stays front (OI-105).
    fn insert_start_tab(&mut self, at: Option<usize>) -> TabId {
        let pinned = self.export_pinned_document().and_then(|doc| self.index_of_document(doc));
        if pinned.is_none() {
            self.flush_active_draft();
            self.capture_active_position();
            self.save_outline_filter();
            // T-26: the document being left keeps its own sticky notice.
            self.stash_active_notice();
            self.lightbox = None;
        }
        let id = self.allocate_tab();
        let index = at.unwrap_or(self.tabs.len()).min(self.tabs.len());
        self.tabs.insert(index, Tab::start(id));
        match pinned {
            Some(active) => self.active = if index <= active { active + 1 } else { active },
            None => {
                self.active = index;
                self.load_active_notice();
                self.note_active_tab();
                self.sync_editor_panes();
                self.begin_restore(&ScrollAnchor::top());
                self.load_outline_filter();
                self.sync_outline();
            }
        }
        id
    }

    pub(crate) fn push_start_tab(&mut self) -> TabId {
        let id = self.allocate_tab();
        self.tabs.push(Tab::start(id));
        id
    }

    /// Returns to a single start page, releasing every document reference.
    pub fn reset_to_start_page(&mut self) {
        self.flush_all_drafts();
        self.capture_active_position();
        let tabs = std::mem::take(&mut self.tabs);
        for tab in tabs {
            self.release_tab(tab);
        }
        self.active = 0;
        self.loads.clear();
        self.ui.help = None;
        self.ui.notice = None;
        self.ui.block_menu = None;
        self.ui.link_menu = None;
        self.ui.reveal_tab = None;
        self.close_palette();
        self.close_tab_list();
        self.share.menu_open = false;
        self.editor.format_menu = None;
        self.tables.preview = None;
        self.tables.target = None;
        self.lightbox = None;
        self.reading.line_flash = None;
        self.reading.landing = None;
        self.reading.landing_flash = None;
        // A scene reset starts a new window's history, not the old one's.
        self.tab_actions.clear();
        self.outline_filter.replace_text("");
        self.begin_restore(&ScrollAnchor::top());
        self.push_start_tab();
        self.sync_editor_panes();
        self.sync_outline();
        self.shared.docs_mut().collect_unused();
    }

    /// Opens a path, or brings its existing tab forward.
    ///
    /// A document replaces the front start page (T-13); otherwise it is appended
    /// at the end, matching files opened from outside (T-14). Opening a file
    /// already in this window only activates its tab (T-11).
    ///
    /// A document other than the one in front is a new place: the place left
    /// goes on the opened tab's Back list (O-12), unless this open is itself a
    /// Back/Forward move or a session replay.
    pub fn open_path(&mut self, path: PathBuf) -> TabId {
        self.open_path_placed(path, OpenPlace::Front).0
    }

    /// Opens `path` behind the front tab (T-13/T-14): a start page or a pasted
    /// tab in front keeps its place, and the new tab joins the strip without
    /// taking the keyboard.
    ///
    /// T-14: it joins right after the tab it was opened from, and successive
    /// background tabs queue after each other; another tab coming forward
    /// starts a new queue. The browser and palette's background opens use this
    /// path instead of opening in front and switching back.
    pub fn open_path_background(&mut self, path: PathBuf) -> TabId {
        self.open_path_placed(path, OpenPlace::Background).0
    }

    /// The slot a background open takes (T-14): after the last queued
    /// background tab, else right after the front tab, else the end.
    fn background_insert_index(&self) -> usize {
        let anchor =
            self.background_queue.and_then(|id| self.tabs.iter().position(|tab| tab.id == id)).unwrap_or(self.active);
        (anchor + 1).min(self.tabs.len())
    }

    /// [`Self::open_path`] with D-02's drop placements, returning the tab and
    /// whether it was newly opened (false when an open tab came forward).
    fn open_path_placed(&mut self, path: PathBuf, place: OpenPlace) -> (TabId, bool) {
        let from = self.open_origin();
        let path = lmdv_io::absolute_lexical(&path);
        // F-01: a document outside the browser's root re-roots the pane there;
        // one inside keeps the root.
        self.follow_document_root(&path);
        let doc = {
            let mut docs = self.shared.docs_mut();
            docs.insert_loading(DocumentKey::File(path.clone()), title_for_path(&path))
        };

        if let Some(index) = self.index_of_document(doc.id) {
            if !place.background() {
                self.switch_to_index(index);
            } else if let Some(tab) = self.tabs.get(index).map(|tab| tab.id) {
                // T-15/T-11: a background open of an already-open document
                // pulses its tab and reveals it, so the reader sees where the
                // file already lives.
                self.pulse_tab(tab);
            }
            let tab = self.tabs[index].id;
            if !place.background() {
                self.record_open(tab, from);
            }
            // E-15: reopening a file that is already open is also a disk check.
            // A dirty buffer is never read over; a clean one reloads and keeps
            // its reading anchor.
            self.check_open_document(doc.id);
            return (tab, false);
        }

        let needs_load = !matches!(*doc.status.borrow(), DocStatus::Ready);
        let index = match place {
            OpenPlace::Background => Some(self.background_insert_index()),
            _ => place.index(),
        };
        let tab = self.attach_document_placed(doc.clone(), index, None, place.background());
        if place == OpenPlace::Background {
            self.background_queue = Some(tab);
        }
        if needs_load {
            self.start_load(doc.id, path);
        } else if !self.restoring_tabs {
            self.shared.record_recent(path, unix_millis());
        }
        if !place.background() {
            self.record_open(tab, from);
        }
        (tab, true)
    }

    /// Opens files dropped from another app (D-02).
    ///
    /// Only Markdown paths are opened; `files` has already passed
    /// `accepts_files`. The first opens in front and the rest join behind it.
    /// Dropped on the tab strip, they take the strip index the drop point names
    /// and successive files follow at the next indices.
    pub(crate) fn dropped_files(&mut self, files: &[PathBuf], tx: &mut Tx) {
        self.drop.set_over(false);
        let strip = tx.drop_point().and_then(|point| self.drop.strip_index(point.window_x, point.window_y));
        self.open_dropped_at(files, strip);
        self.apply_pending_scroll(tx);
    }

    /// [`Self::dropped_files`] without a pointer, for tests and the headless
    /// script path: `strip` is the drop index, `None` for the window itself.
    pub(crate) fn open_dropped_at(&mut self, files: &[PathBuf], strip: Option<usize>) {
        let markdown: Vec<PathBuf> = files.iter().filter(|path| is_markdown_path(path)).cloned().collect();
        if markdown.is_empty() {
            return;
        }
        let mut cursor = strip;
        for (index, path) in markdown.into_iter().enumerate() {
            let place = match cursor {
                Some(at) if index == 0 => OpenPlace::At(at),
                Some(at) => OpenPlace::AtBackground(at),
                None if index == 0 => OpenPlace::Front,
                // D-02: a drop on the window itself opens the rest behind the
                // first at the end, not through T-14's background queue.
                None => OpenPlace::AtBackground(self.tabs.len()),
            };
            let (tab, opened) = self.open_path_placed(path, place);
            if let Some(at) = cursor {
                // New files take the following indices; an existing document
                // opening in front moves the cursor past its tab once.
                if opened || index == 0 {
                    cursor = self.tabs.iter().position(|candidate| candidate.id == tab).map(|at| at + 1).or(Some(at));
                }
            }
        }
    }

    /// Says why a dropped file did not open (D-02: Markdown only).
    pub(crate) fn dropped_files_rejected(&mut self, files: &[PathBuf]) {
        self.drop.set_over(false);
        let Some(first) = files.first() else { return };
        let name = first
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| first.display().to_string());
        self.notice_kind(lmdv_i18n::t!("msg.notMarkdown", "name" => name), NoticeKind::Warning);
    }

    /// Adds a ready in-memory document, for scenes and tests. Never touches
    /// disk and never records a recent.
    ///
    /// Scenes and tests must be deterministic on the first frame, so the model
    /// is parsed here instead of through a job. Real loads and edits always go
    /// through [`WindowState::drive_parse`] and a worker.
    pub fn open_synthetic(&mut self, title: &str, text: &str) -> TabId {
        let paste = self.shared.allocate_paste();
        let id = self.shared.docs_mut().allocate_id();
        let doc = Rc::new(Document::ready(id, DocumentKey::Untitled(paste), title, SourceBuffer::new(text)));
        self.shared.docs_mut().insert_ready(doc.clone());
        let revision = doc.revision();
        let model = Arc::new(lmdv_md::parse_with_revision(text, revision));
        self.shared.store_parsed(id, revision, Ok(model));
        let tab = self.attach_document(doc);
        if let Some(tab) = self.tabs.iter_mut().find(|candidate| candidate.id == tab) {
            tab.view.pasted_base = Some(text.to_owned());
        }
        tab
    }

    pub(crate) fn attach_document(&mut self, doc: Rc<Document>) -> TabId {
        self.attach_document_at(doc, None, None)
    }

    /// Attaches `doc`, optionally at a strip index with a remembered reading
    /// position (T-07's reopen). `None` keeps the normal open placement: the
    /// front start page is replaced, else the tab is appended and focused.
    pub(crate) fn attach_document_at(
        &mut self,
        doc: Rc<Document>,
        index: Option<usize>,
        anchor: Option<ScrollAnchor>,
    ) -> TabId {
        self.attach_document_placed(doc, index, anchor, false)
    }

    /// [`Self::attach_document_at`] with D-02's background placement: the tab
    /// joins the strip and the front tab stays where it is.
    fn attach_document_placed(
        &mut self,
        doc: Rc<Document>,
        index: Option<usize>,
        anchor: Option<ScrollAnchor>,
        background: bool,
    ) -> TabId {
        let mut tab = Tab::for_document(self.allocate_tab(), doc.clone());
        self.adopt_cached_parse(&mut tab, &doc);
        if let Some(anchor) = anchor {
            tab.view.anchor = anchor;
        } else if let Some(path) = doc.key.borrow().path().map(Path::to_path_buf)
            && let Some(anchor) = self.shared.position_for(&path)
        {
            tab.view.anchor = anchor;
        }
        let tab_id = tab.id;
        let anchor = tab.view.anchor.clone();
        // OI-105: an export keeps its document front, so an opened tab joins the
        // strip in the background until the job ends. D-02's later dropped files
        // join the same way, without an export.
        let pinned = self.export_pinned_document().and_then(|doc| self.index_of_document(doc));
        let keep_front = background || pinned.is_some();
        // T-15: a tab that opens behind rings once and keeps an Unread dot for a
        // short interval, so the eye finds it in the strip; activation clears it.
        if keep_front && !self.restoring_tabs {
            tab.view.pulse = Some(TabPulse { seq: self.tab_pulse_seq, until: self.ui.last_now + TAB_PULSE_SECONDS });
            self.tab_pulse_seq = self.tab_pulse_seq.wrapping_add(1).max(1);
            self.ui.reveal_tab = Some(tab_id);
        }
        let replacing_start = !keep_front
            && index.is_none()
            && !self.restoring_tabs
            && self.tabs.get(self.active).is_some_and(Tab::is_start);
        if !replacing_start && !keep_front {
            self.capture_active_position();
            self.save_outline_filter();
            // T-26: the tab being left keeps its own sticky notice.
            self.stash_active_notice();
        }
        if replacing_start {
            self.tabs[self.active] = tab;
        } else if let Some(index) = index {
            let index = index.min(self.tabs.len());
            self.tabs.insert(index, tab);
            self.active =
                if keep_front { if index <= self.active { self.active + 1 } else { self.active } } else { index };
        } else {
            self.tabs.push(tab);
            if !keep_front {
                self.active = self.tabs.len().saturating_sub(1);
            }
        }
        doc.subscribers.borrow_mut().insert(self.id);
        if let Some(path) = doc.key.borrow().path().map(Path::to_path_buf) {
            self.shared.watch_path(&path);
        }
        if !keep_front {
            // T-26: a new document coming forward shows its own notice (usually
            // none), never the one the previous document left on screen.
            self.load_active_notice();
            self.note_active_tab();
            self.sync_editor_panes();
            self.begin_restore(&anchor);
            self.load_outline_filter();
            self.sync_outline();
            // A new tab in front is a tab switch: the table preview and the media
            // preview belonged to the tab that was showing (P6-T, V-01).
            self.tables.preview = None;
            self.tables.target = None;
            self.lightbox = None;
        }
        if self.shared.docs().get(doc.id).is_some_and(|document| {
            matches!(*document.status.borrow(), DocStatus::Ready) && document.key.borrow().path().is_some()
        }) {
            self.request_draft_check(doc.id);
        }
        tab_id
    }

    pub(crate) fn index_of_document(&self, id: DocumentId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.document_id() == Some(id))
    }

    pub(crate) fn start_load(&mut self, doc: DocumentId, path: PathBuf) {
        if let Some(document) = self.shared.docs().get(doc) {
            *document.status.borrow_mut() = DocStatus::Loading;
        }
        let generation = self.loads.begin(doc);
        self.shared.jobs().load(self.worker.clone(), doc, path, generation);
    }

    /// Activates a tab by id. False when it is gone.
    pub fn activate_tab(&mut self, id: TabId) -> bool {
        match self.tabs.iter().position(|tab| tab.id == id) {
            Some(index) => self.switch_to_index(index),
            None => false,
        }
    }

    /// [`Self::activate_tab`], issuing the remembered scroll in the same frame.
    pub fn activate_tab_with_tx(&mut self, id: TabId, tx: &mut Tx) -> bool {
        let activated = self.activate_tab(id);
        if activated {
            self.apply_pending_scroll(tx);
        }
        activated
    }

    /// Activates a tab by index. False when out of range.
    pub fn activate_index(&mut self, index: usize) -> bool {
        self.switch_to_index(index)
    }

    /// [`Self::activate_index`], issuing the remembered scroll in the same frame.
    pub fn activate_index_with_tx(&mut self, index: usize, tx: &mut Tx) -> bool {
        let activated = self.activate_index(index);
        if activated {
            self.apply_pending_scroll(tx);
        }
        activated
    }

    /// Moves the active tab, saving the old tab's place and filter and queueing
    /// the new tab's scroll restore.
    pub(crate) fn switch_to_index(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() {
            return false;
        }
        if index == self.active {
            return true;
        }
        // OI-105: an export pins the document it snapshotted to the front until
        // the job ends. Switching to another tab of the same document is fine.
        let leaving = self.active_tab().and_then(Tab::document_id);
        if leaving.is_some_and(|doc| self.export_pins(doc)) && self.tabs[index].document_id() != leaving {
            self.export_holds_tab();
            return false;
        }
        // T-26/R-29: the notice on screen belongs to the tab it was raised for.
        // A sticky one waits on that tab and comes back with it; the tab coming
        // forward shows its own, if it has one.
        self.stash_active_notice();
        self.flush_active_draft();
        self.capture_active_position();
        self.save_outline_filter();
        self.editor.goto_line = None;
        self.editor.format_menu = None;
        self.ui.block_menu = None;
        self.ui.link_menu = None;
        self.share.menu_open = false;
        self.tables.preview = None;
        self.tables.target = None;
        self.lightbox = None;
        self.active = index;
        self.load_active_notice();
        self.reset_scroll_sync();
        // A tab coming forward settles like an edit-mode entry: its first
        // scroll report must not move the preview.
        self.suppress_scroll_sync();
        self.sync_editor_panes();
        let anchor = self.tabs[index].view.anchor.clone();
        self.begin_restore(&anchor);
        // E-15(c): a line target that belongs to this tab also reaches the
        // source pane when the tab comes forward.
        let pending = self.tabs[index].view.line.filter(|_| self.tabs[index].view.editor.is_some());
        if let Some(line) = pending {
            if let Some(editor) = self.tabs[index].view.editor.as_mut() {
                aim_editor_caret(editor, line);
            }
            self.editor.line_reveal = Some(self.tabs[index].id);
        }
        self.load_outline_filter();
        self.sync_outline();
        self.note_active_tab();
        true
    }

    /// Records the active tab as the most recently shown (C-02's tab order) and
    /// forgets tabs that closed. A tab switch also drops the hover card (T-16)
    /// and ends T-14's background queue.
    pub(crate) fn note_active_tab(&mut self) {
        self.hide_tab_card();
        self.background_queue = None;
        // T-15: the mark was the strip's way of asking to be looked at; the tab
        // coming forward answers it.
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.view.pulse = None;
        }
        let Some(id) = self.tabs.get(self.active).map(|tab| tab.id) else { return };
        self.tab_recent.retain(|other| *other != id);
        self.tab_recent.insert(0, id);
        let live: BTreeSet<TabId> = self.tabs.iter().map(|tab| tab.id).collect();
        self.tab_recent.retain(|id| live.contains(id));
    }

    /// Sets the pending `--line` target on the active document tab.
    ///
    /// E-15(c): on a tab that is editing, the target also moves the source
    /// caret, so a launch, a search hit or a remembered line lands on both the
    /// article and the source pane. [`Self::poll_line_reveal`] scrolls the
    /// source once the frame holds the tab.
    pub fn set_pending_line(&mut self, line: usize) {
        let line = line.max(1);
        let Some(tab) = self.active_tab_mut().filter(|tab| !tab.is_start()) else { return };
        tab.view.line = Some(line);
        let id = tab.id;
        let aimed = match tab.view.editor.as_mut() {
            Some(editor) => aim_editor_caret(editor, line),
            None => false,
        };
        if aimed {
            self.editor.line_reveal = Some(id);
        }
    }

    /// Closes a tab. Closing the lone start page asks the window to close
    /// instead (T-06); closing the last document leaves a start page (T-05).
    pub fn close_tab(&mut self, id: TabId) -> CloseOutcome {
        self.close_tab_inner(id, true)
    }

    /// Closes a tab that must not enter the closed-tab history: a Back/Forward
    /// move's error tab is not a place Ctrl+Shift+T should bring back (O-13).
    pub(crate) fn close_tab_without_history(&mut self, id: TabId) -> CloseOutcome {
        self.close_tab_inner(id, false)
    }

    fn close_tab_inner(&mut self, id: TabId, remember: bool) -> CloseOutcome {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return CloseOutcome::NotFound;
        };
        // OI-105: the exporting document stays in the strip until its job ends.
        if self.tabs[index].document_id().is_some_and(|doc| self.export_pins(doc)) {
            self.export_holds_tab();
            return CloseOutcome::Blocked;
        }
        // A Back/Forward move that opened this tab has nowhere to land.
        self.cancel_travel_for(id);
        if self.tables.preview.as_ref().is_some_and(|preview| preview.tab == id) {
            self.tables.preview = None;
        }
        if self.tables.target.as_ref().is_some_and(|target| target.tab == id) {
            self.tables.target = None;
        }
        if self.lightbox.as_ref().is_some_and(|lightbox| lightbox.tab == id) {
            self.lightbox = None;
        }
        // The all-tabs list names the tabs as they are; one leaving closes it.
        self.close_tab_list();
        if self.tabs.len() == 1 && self.tabs[index].is_start() {
            return CloseOutcome::WindowClose;
        }

        let closing_active = index == self.active;
        if closing_active {
            self.flush_active_draft();
            self.capture_active_position();
        }
        // P-19: an unchanged pasted tab leaves no draft behind. A draft on
        // disk is what a restored pasted tab lives in, so a clean one closing
        // must not leave an orphan for the next launch.
        if let Some(document) = self.tabs[index].document().cloned()
            && matches!(*document.key.borrow(), DocumentKey::Untitled(_))
            && !document.is_dirty()
            && self.is_last_view(&document)
            && let Some(key) = crate::editor::draft_key(&document)
        {
            self.shared.queue_store(lmdv_io::store::StorePatch::RemoveDraft(key));
        }
        // T-07: a document tab that closes is remembered, with its place, so
        // Ctrl+Shift+T can bring it back where it was.
        if remember {
            let closed = crate::actions::closed_tab_record(&self.tabs[index], index);
            if let Some(record) = closed {
                self.remember_closed_tab(record);
            }
        }
        let tab = self.tabs.remove(index);
        self.release_tab(tab);
        if self.tabs.is_empty() {
            self.push_start_tab();
            self.active = 0;
        } else if index < self.active {
            self.active = self.active.saturating_sub(1);
        } else if index == self.active {
            self.active = index.min(self.tabs.len().saturating_sub(1));
        }
        if closing_active {
            // T-26: the closed tab's notice goes with it; the neighbour coming
            // forward brings its own.
            self.ui.notice = None;
            self.sync_editor_panes();
            let anchor = self.active_tab().map(|tab| tab.view.anchor.clone()).unwrap_or_default();
            self.begin_restore(&anchor);
            self.load_outline_filter();
            self.load_active_notice();
        }
        self.editor.format_menu = None;
        self.note_active_tab();
        self.sync_outline();
        self.shared.docs_mut().collect_unused();
        CloseOutcome::Closed
    }

    /// Closes the tab, asking first when it is the last view of a dirty
    /// document (E-13/T-05); a lone start page asks the window to close.
    pub fn request_close_tab(&mut self, id: TabId) {
        // E-22: a dropdown belongs to the tab it was opened over; any close
        // request dismisses it before a question or a switch can happen.
        self.editor.format_menu = None;
        // OI-105: no save prompt or close while the exporting document's job is
        // still running; the completion clears the pin.
        if self.tab(id).and_then(Tab::document_id).is_some_and(|doc| self.export_pins(doc)) {
            self.export_holds_tab();
            return;
        }
        if self.guard_close_tab(id) {
            return;
        }
        if self.close_tab(id) == CloseOutcome::WindowClose {
            self.request_close_window();
        }
    }

    /// Closes the active tab, or requests the window close for a lone start page.
    pub fn close_active_tab(&mut self) {
        // T-29: the close key ends a tab drag in hand first, and stops there;
        // a second press closes the tab. The drag is the nearer surface.
        if !matches!(self.drag.phase(), rune_kit::shell::TabDragPhase::Idle) {
            self.drag.cancel();
            return;
        }
        if let Some(id) = self.active_tab().map(|tab| tab.id) {
            self.request_close_tab(id);
        }
    }

    /// Asks the next `poll` to close this window, after the dirty guard.
    pub fn request_close_window(&mut self) {
        if self.guard_close_window() {
            return;
        }
        self.shared.set_window_closing(self.id, true);
        self.ui.close_requested = true;
    }

    /// Opens a start-page window cascaded from the previous one created within
    /// three seconds, else from this one (T-31/S-13): 32 points down and right,
    /// restarting at the work area's top-left when that leaves it. A maximized
    /// source makes a maximized child; a minimized one opens the default size,
    /// centred. The compositor places it where positions are not exposed
    /// (Wayland).
    pub fn request_new_window(&mut self) {
        let now = self.ui.last_now;
        let plan = self.shared.new_window_plan(self.id, now);
        let request =
            WindowOpenRequest { launch: Launch::default(), kind: WindowKind::Normal, position: plan.position };
        match Rc::clone(&self.shared).request_new_window_planned(request, plan) {
            Ok(opened) => self.shared.note_window_created(opened.id, now),
            Err(error) => self.notice_kind(lmdv_i18n::t!("msg.openWindowFailed", "err" => error), NoticeKind::Error),
        }
    }

    /// Moves a tab, keeping the active document in front.
    pub fn reorder_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        // OI-105: the exporting document's tab stays at its slot until the job
        // ends. Moving another tab is allowed.
        if self.tabs[from].document_id().is_some_and(|doc| self.export_pins(doc)) {
            self.export_holds_tab();
            return;
        }
        let active_id = self.active_tab().map(|tab| tab.id);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if let Some(id) = active_id
            && let Some(index) = self.tabs.iter().position(|tab| tab.id == id)
        {
            self.active = index;
        }
    }

    /// T-10: moves the front tab one slot left (`direction` < 0) or right,
    /// clamped at the ends. False when it could not move (one tab, or already
    /// at that end), so the caller knows whether to reveal it again.
    pub fn move_active_tab(&mut self, direction: i32) -> bool {
        if direction == 0 || self.tabs.len() < 2 {
            return false;
        }
        // OI-105: an export pins the front document; the move is refused rather
        // than reported as a successful reveal.
        if self.active_tab().and_then(Tab::document_id).is_some_and(|doc| self.export_pins(doc)) {
            self.export_holds_tab();
            return false;
        }
        let to = if direction < 0 {
            match self.active.checked_sub(1) {
                Some(to) => to,
                None => return false,
            }
        } else {
            match self.active.checked_add(1).filter(|to| *to < self.tabs.len()) {
                Some(to) => to,
                None => return false,
            }
        };
        self.reorder_tab(self.active, to);
        true
    }

    /// Releases a tab's document references. The caller has already removed it
    /// from `tabs`.
    fn release_tab(&mut self, tab: Tab) {
        let closing = tab.document_id();
        if let TabSource::Document(doc) = &tab.source {
            self.loads.forget(doc.id);
            self.restored_docs.remove(&doc.id);
            if let Some(lease) = doc.lease_holder()
                && lease.window == self.id
                && lease.tab == tab.id
            {
                doc.release_edit(lease);
            }
            if !self.tabs.iter().any(|other| other.document_id() == Some(doc.id)) {
                doc.subscribers.borrow_mut().remove(&self.id);
            }
            if doc.subscribers.borrow().is_empty() {
                self.shared.forget_parsed(doc.id);
            }
        }
        if let Some(path) = tab.source.document().and_then(|doc| doc.key.borrow().path().map(Path::to_path_buf)) {
            self.shared.unwatch_path(&path);
        }
        if let Some(doc) = closing {
            self.parse_inflight.remove(&doc);
        }
        drop(tab);
    }

    /// Whether any open document has unsaved edits.
    pub fn has_unsaved_documents(&self) -> bool {
        self.tabs.iter().filter_map(|tab| tab.document()).any(|doc| doc.is_dirty())
    }

    /// Whether this window holds the only open view of `document`.
    ///
    /// Subscribers are updated as a tab is attached and released, so they are
    /// the immediate truth while the window registry is only republished after
    /// a `poll`.
    pub fn is_last_view(&self, document: &Document) -> bool {
        !document.subscribers.borrow().iter().any(|window| *window != self.id)
    }

    /// The documents this window's close question has to name (T-24).
    ///
    /// A dirty file is included only while this window holds its last view: a
    /// document another window still shows closes silently here and keeps its
    /// buffer, lease and draft there. Pasted text has no file, so every pasted
    /// tab is named whether changed or not (T-24).
    pub fn close_documents(&self) -> Vec<CloseDoc> {
        let mut docs: Vec<CloseDoc> = Vec::new();
        for tab in &self.tabs {
            let Some(document) = tab.document() else { continue };
            if docs.iter().any(|doc| doc.id == document.id) {
                continue;
            }
            let pasted = matches!(*document.key.borrow(), DocumentKey::Untitled(_));
            let dirty = document.is_dirty();
            if !pasted && !(dirty && self.is_last_view(document)) {
                continue;
            }
            docs.push(CloseDoc { id: document.id, title: close_title(document), pasted, dirty });
        }
        docs
    }

    /// The T-24 close question: changed files and pasted text are named
    /// separately, with the interface's list conjunction.
    pub fn close_window_question(&self) -> String {
        let docs = self.close_documents();
        let changed: Vec<&str> =
            docs.iter().filter(|doc| doc.dirty && !doc.pasted).map(|doc| doc.title.as_str()).collect();
        let pasted: Vec<&str> = docs.iter().filter(|doc| doc.pasted).map(|doc| doc.title.as_str()).collect();
        if pasted.is_empty() {
            if changed.len() == 1 {
                lmdv_i18n::t!("msg.closeWindowOneQ", "name" => changed[0])
            } else {
                lmdv_i18n::t!("msg.closeWindowQ", "names" => lmdv_i18n::list(&changed))
            }
        } else if changed.is_empty() {
            if pasted.len() == 1 {
                lmdv_i18n::t!("msg.closeWindowPastedOneQ", "name" => pasted[0])
            } else {
                lmdv_i18n::t!("msg.closeWindowPastedQ", "names" => lmdv_i18n::list(&pasted))
            }
        } else {
            lmdv_i18n::t!("msg.closeWindowMixedQ", "changed" => lmdv_i18n::list(&changed), "pasted" => lmdv_i18n::list(&pasted))
        }
    }

    /// The window title for the native frame and the desktop (W-09):
    /// `“name” — lmdv`, with `• ` in front while the front tab is dirty, and
    /// exactly `lmdv` on a start page.
    pub fn window_title(&self) -> String {
        match self.active_tab() {
            Some(Tab { source: TabSource::Document(doc), view, .. }) => {
                let title = doc.title.borrow();
                if title.trim().is_empty() {
                    "lmdv".to_owned()
                } else {
                    // The dot follows the shared registry too: a document
                    // dirtied in another window still reads dirty here without
                    // hashing the buffer every frame.
                    let dirty = view.dirty || self.shared.document_is_dirty(doc.id);
                    let name = format!("\u{201c}{}\u{201d}", title.trim());
                    let name = if dirty { format!("\u{2022} {name}") } else { name };
                    lmdv_i18n::t!("toolbar.windowTitle", "name" => name)
                }
            }
            _ => "lmdv".to_owned(),
        }
    }

    /// Shows a status notice for `NOTICE_SECONDS`.
    ///
    /// The kind is [`NoticeKind::Info`] unless the message is one of the
    /// confirmations the older handlers raise through this one entry point (a
    /// save, a copied path, table or image): those are successes (H-06).
    pub fn notice(&mut self, text: impl Into<String>) {
        let text = text.into();
        let kind = if is_confirmation(&text) { NoticeKind::Success } else { NoticeKind::Info };
        self.show_notice(text, kind, None, NOTICE_SECONDS);
    }

    /// [`Self::notice`] with an explicit severity (H-06).
    pub fn notice_kind(&mut self, text: impl Into<String>, kind: NoticeKind) {
        self.show_notice(text.into(), kind, None, NOTICE_SECONDS);
    }

    /// A notice that does not expire on its own (R-29): an empty/error state
    /// stays readable until its document is saved or reloaded, the user
    /// dismisses it, or its tab closes (T-26). Another document coming forward
    /// leaves it on the tab it describes and restores it on the way back. Use
    /// it for a fact about the document on screen, never for a confirmation.
    pub fn sticky_notice(&mut self, text: impl Into<String>, kind: NoticeKind) {
        self.show_notice(text.into(), kind, None, NOTICE_STICKY_SECONDS);
    }

    /// [`Self::notice`] whose words are the strip's, with its own stay.
    ///
    /// Every notice is numbered here (H-07): the bar alternates an invisible
    /// mark on the number, so the same message twice still changes what a
    /// screen reader is told.
    ///
    /// T-26: a sticky notice also becomes the front tab's own, so another tab
    /// coming forward leaves it behind and switching back restores it. A
    /// confirmation or a settings message is transient and belongs to the
    /// window's front tab only.
    pub(crate) fn show_notice(&mut self, text: String, kind: NoticeKind, action: Option<NoticeAction>, seconds: f64) {
        self.ui.notice_seq = self.ui.notice_seq.wrapping_add(1);
        let deadline = if seconds.is_finite() { self.ui.last_now + seconds } else { f64::INFINITY };
        let notice = Notice { text, deadline, action, kind, seq: self.ui.notice_seq };
        if notice.is_sticky()
            && let Some(tab) = self.tabs.get_mut(self.active)
        {
            tab.view.notice = Some(notice.clone());
        }
        self.ui.notice = Some(notice);
    }

    /// A sticky notice about `doc` (T-26): when that document is not the front
    /// tab, the notice waits on its tab and is shown when it comes forward, so
    /// a failed background load never puts its words over another document.
    pub(crate) fn sticky_notice_for_document(&mut self, doc: DocumentId, text: impl Into<String>, kind: NoticeKind) {
        let text = text.into();
        if self.active_document_id() == Some(doc) {
            self.sticky_notice(text, kind);
            return;
        }
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.document_id() == Some(doc)) else { return };
        self.ui.notice_seq = self.ui.notice_seq.wrapping_add(1);
        tab.view.notice = Some(Notice { text, deadline: f64::INFINITY, action: None, kind, seq: self.ui.notice_seq });
    }

    /// Moves the front tab's visible notice onto the tab before another one
    /// comes forward (T-26). Only a sticky notice is kept: a confirmation has
    /// done its work and does not come back on a later switch.
    pub(crate) fn stash_active_notice(&mut self) {
        let Some(notice) = self.ui.notice.take() else { return };
        if !notice.is_sticky() {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.view.notice = Some(notice);
        }
    }

    /// Shows the front tab's own sticky notice, if it has one (T-26).
    pub(crate) fn load_active_notice(&mut self) {
        self.ui.notice = self.tabs.get(self.active).and_then(|tab| tab.view.notice.clone());
    }

    /// Ends the sticky notice a document event makes stale (R-29) on the tabs
    /// of those documents, and on the window when one of them is in front. A
    /// dirty document's save or reload never silences another tab's error.
    fn clear_sticky_notice_for(&mut self, docs: &[DocumentId]) {
        let front_affected = self.active_tab().and_then(Tab::document_id).is_some_and(|doc| docs.contains(&doc));
        if front_affected && self.ui.notice.as_ref().is_some_and(Notice::is_sticky) {
            self.ui.notice = None;
        }
        for tab in &mut self.tabs {
            if tab.document_id().is_some_and(|doc| docs.contains(&doc))
                && tab.view.notice.as_ref().is_some_and(Notice::is_sticky)
            {
                tab.view.notice = None;
            }
        }
    }

    /// Dismisses the current notice, including the front tab's own copy.
    pub fn dismiss_notice(&mut self) {
        self.ui.notice = None;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.view.notice = None;
        }
    }

    /// T-03: keeps the strip's post-save check in step with the editor's saved
    /// flash. A save completion in this poll names the document(s) it wrote by
    /// their changed `last_local_write`; the flash's expiry clears them. The
    /// save's “Saved” reaches a screen reader through the notice bar, the
    /// window's one polite live region (H-07), not a second whisper here.
    fn track_saved_flash(&mut self, before: Option<f64>, writes_before: &[(DocumentId, Option<FileMeta>)]) -> bool {
        let after = self.editor.saved_flash_until;
        if after == before {
            return false;
        }
        if after.is_none() {
            if self.saved_flash_docs.is_empty() {
                return false;
            }
            self.saved_flash_docs.clear();
            return true;
        }
        let mut docs = BTreeSet::new();
        for tab in &self.tabs {
            let Some(document) = tab.document() else { continue };
            let old = writes_before.iter().find(|(id, _)| *id == document.id).and_then(|(_, meta)| *meta);
            if document.last_local_write.get() != old {
                docs.insert(document.id);
            }
        }
        // Saving an unchanged file may write no new identity: the document in
        // front is the operator's own save.
        if docs.is_empty()
            && let Some(doc) = self.active_document_id()
        {
            docs.insert(doc);
        }
        // R-29: a save is a document event: the saved document's sticky
        // empty/error notice gives way to its own transient “Saved” (T-26).
        self.clear_sticky_notice_for(&docs.iter().copied().collect::<Vec<_>>());
        self.saved_flash_docs = docs;
        true
    }

    /// Arms T-15's mark for one tab (an open behind, or an existing document
    /// opened behind) and asks the next poll to reveal it in the strip.
    fn pulse_tab(&mut self, tab: TabId) {
        let Some(index) = self.tabs.iter().position(|candidate| candidate.id == tab) else { return };
        // Nothing to find when the document is already in front.
        if index == self.active {
            return;
        }
        let pulse = TabPulse { seq: self.tab_pulse_seq, until: self.ui.last_now + TAB_PULSE_SECONDS };
        self.tab_pulse_seq = self.tab_pulse_seq.wrapping_add(1).max(1);
        self.tabs[index].view.pulse = Some(pulse);
        self.ui.reveal_tab = Some(tab);
    }

    /// T-15: scrolls the strip to a tab just opened behind, so the ring and dot
    /// are where the eye can see them. The tab stays behind the front one.
    fn apply_pending_tab_reveal(&mut self, tx: &mut Tx) -> bool {
        let Some(tab) = self.ui.reveal_tab else { return false };
        self.ui.reveal_tab = None;
        if self.tab(tab).is_none() {
            return false;
        }
        tx.scroll_into_view_in(crate::tab_list::TAB_STRIP_SCROLL_KEY, format!("t-{}", tab_key(tab)));
        false
    }

    /// T-15: expires the Unread mark of tabs opened behind, waking the frame
    /// when the next one is due. A tab that came forward already cleared its
    /// mark in [`Self::note_active_tab`].
    fn poll_tab_pulses(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        let mut next: Option<f64> = None;
        for tab in &mut self.tabs {
            let Some(pulse) = tab.view.pulse else { continue };
            if now >= pulse.until {
                tab.view.pulse = None;
                changed = true;
            } else {
                next = Some(next.map_or(pulse.until, |due: f64| due.min(pulse.until)));
            }
        }
        if let Some(due) = next {
            tx.wake_at(due);
        }
        changed
    }

    /// Drains worker and UI mailboxes, pumps timers and updates the registry.
    ///
    /// Returns whether the view reads anything that changed. Close and quit
    /// requests are issued here, where the command left them, so command
    /// functions stay free of host-specific `Tx` effects.
    pub fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        self.shared.note_now(now);
        // T-03: what a save completion in this poll may change, for the tab's
        // check: the flash deadline and each document's last own write.
        let saved_flash_before = self.editor.saved_flash_until;
        let writes_before: Vec<(DocumentId, Option<FileMeta>)> = self
            .tabs
            .iter()
            .filter_map(|tab| tab.document().map(|document| (document.id, document.last_local_write.get())))
            .collect();
        // OI-141: Rune answers the focus in a poll; the section line hides while
        // a focus ring the keyboard put somewhere sits under it. A change must
        // re-run the view, or the line would keep the last frame's verdict.
        if tx.focused() != self.ui.focused_key.as_deref() {
            self.ui.focused_key = tx.focused().map(str::to_owned);
            changed = true;
        }
        if tx.focus_visible() != self.ui.focus_visible {
            self.ui.focus_visible = tx.focus_visible();
            changed = true;
        }
        for job in self.worker.drain() {
            changed |= self.handle_job(job, tx);
        }
        changed |= self.apply_pending_editor_tabs(tx);
        for msg in self.ui_inbox.drain() {
            changed |= self.handle_ui(msg);
        }
        changed |= self.clear_stale_tear_hover();
        if self.shared.is_router(self.id) {
            for request in self.shared.launch_inbox().drain() {
                changed |= self.route_launch(request.launch);
            }
        }
        changed |= self.reap_transfers();
        changed |= self.pump_timers(now);
        // H-02: the start page drops recents whose file is gone when it is
        // drawn, not only when `recents.json` is loaded. The sweep is throttled
        // and only runs while the start page is in front; a headless gate never
        // rewrites the user's recents.
        if !self.shared.is_headless() && self.active_tab().is_some_and(Tab::is_start) {
            changed |= self.shared.prune_dead_recents(self.ui.now_unix_ms);
        }
        // R-29: an ordinary notice names its own wake, so it disappears on
        // time even when nothing else changes; a sticky one waits for the
        // document or the user.
        if let Some(deadline) =
            self.ui.notice.as_ref().filter(|notice| !notice.is_sticky()).map(|notice| notice.deadline)
        {
            tx.wake_at(deadline);
        }
        // L-09: a notice's **Save as…** needs the frame's `Tx` for the native
        // dialog, so the press only asks and this poll opens it.
        if let Some(doc) = self.ui.save_as_requested.take() {
            // The notice belonged to a pasted document; a tab switch in the
            // meantime must not turn the press into Save as on a file (L-09).
            let pasted =
                self.tabs.iter().find(|tab| tab.document_id() == Some(doc)).is_some_and(|tab| {
                    matches!(&*tab.document().expect("checked").key.borrow(), DocumentKey::Untitled(_))
                });
            if pasted {
                self.request_save_as(doc, crate::editor::AfterSave::Dismiss, tx);
                changed = true;
            }
        }
        // OI-073: the document watcher's setup error is not a per-frame fact;
        // show it once, like the file browser's watch failure.
        if let Some(error) = self.shared.take_watch_error() {
            self.notice_kind(lmdv_i18n::t!("msg.watchFailed", "err" => error), NoticeKind::Warning);
            changed = true;
        }
        changed |= self.poll_editor(now, tx);
        changed |= self.track_saved_flash(saved_flash_before, &writes_before);
        changed |= self.continue_close_batch();
        changed |= self.poll_share(now, tx);
        changed |= self.drive_parse();
        changed |= self.refresh_reading_stats();
        changed |= self.poll_tab_card(now);
        changed |= self.poll_browser(now, tx);
        changed |= self.images.get().is_some_and(|cache| cache.refresh());
        self.apply_pending_scroll(tx);
        changed |= self.apply_pending_fragment(tx);
        changed |= self.apply_line_target(now, tx);
        changed |= self.poll_line_reveal(tx);
        changed |= self.poll_reader_find(now, tx);
        changed |= self.poll_palette(now, tx);
        changed |= self.poll_tables(now, tx);
        changed |= self.poll_lightbox();
        changed |= self.poll_reading(now, tx);
        changed |= self.poll_reading_keys(now);
        // One context menu at a time: a right-click that reaches a block after a
        // link's menu was open replaces it (OI-195).
        if self.ui.block_menu.is_some() && self.ui.link_menu.take().is_some() {
            changed = true;
        }
        // The link menu is the reader's; leaving reading mode drops it.
        if self.ui.link_menu.is_some() && self.active_tab().is_none_or(|tab| tab.view.mode != Mode::Viewer) {
            self.ui.link_menu = None;
            changed = true;
        }
        changed |= self.poll_tab_pulses(now, tx);
        changed |= self.poll_scroll_pan(now, tx);
        self.apply_pending_tab_reveal(tx);
        changed |= self.poll_landing(now, tx);
        changed |= self.poll_section_line(now, tx);
        changed |= self.expire_line_flash(now);
        changed |= self.sync_outline();
        // O-03: the scroll spy's active row follows the page in the outline
        // pane; the reveal is issued here because the view cannot hold a `Tx`.
        changed |= self.reveal_active_outline_row(tx);

        let title = self.window_title();
        if title != self.ui.last_title {
            // The native placement becomes readable once the window is mapped;
            // cache it for the next cascade without a native call every frame.
            self.shared.refresh_placement(self.id);
            self.ui.last_title = title.clone();
            tx.set_title(title);
            changed = true;
        }
        self.sync_registry();
        // Session fragments are built only when something restorable changed
        // (OI-151); the store write is what debounces. A changed tab, mode,
        // anchor or active index schedules one write 500 ms out
        // (architecture §8.4).
        if self.publish_session() && self.ui.session_due.is_none() {
            let due = now + crate::session::SESSION_DEBOUNCE;
            self.ui.session_due = Some(due);
            tx.wake_at(due);
        }
        if self.ui.session_due.is_some_and(|due| now >= due) {
            self.ui.session_due = None;
            self.shared.queue_session(false);
        }

        if self.ui.quit_requested {
            self.ui.quit_requested = false;
            self.shared.set_quitting();
            tx.quit();
            changed = true;
        } else if self.ui.close_requested {
            self.ui.close_requested = false;
            tx.close_window();
            changed = true;
        }
        changed
    }

    fn handle_job(&mut self, job: JobMsg, tx: &mut Tx) -> bool {
        match job {
            JobMsg::Loaded { doc, generation, result, .. } => {
                if !self.loads.finish(doc, generation) {
                    return false;
                }
                let Some(document) = self.shared.docs().get(doc) else { return false };
                match result {
                    Ok(loaded) => {
                        let path = loaded.path;
                        let meta = loaded.meta;
                        // E-15: a same-file reopen's disk check. It is compared
                        // exactly like the watcher's read, but a difference is
                        // never written over a dirty buffer: the tabs are marked
                        // and the existing file-changed question asks (R-22),
                        // while a clean view reloads with its anchor restored.
                        if self.editor.reopen_checks.remove(&doc) {
                            let same = {
                                let current = document.text();
                                current.text() == loaded.buffer.text()
                                    && current.eol() == loaded.buffer.eol()
                                    && current.had_bom() == loaded.buffer.had_bom()
                            };
                            let dirty = document.is_dirty()
                                || self.tabs.iter().any(|tab| tab.document_id() == Some(doc) && tab.view.dirty);
                            if same {
                                if dirty {
                                    // The buffer already says what the disk says,
                                    // but its saved base is older: only the
                                    // baseline metadata moves, so the text stays
                                    // the user's to save.
                                    document.saved.borrow_mut().meta = meta;
                                } else {
                                    document.set_saved(meta, document.hash(), path);
                                }
                                return false;
                            }
                            if dirty {
                                for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(doc)) {
                                    tab.view.disk_changed = true;
                                }
                                if self.editor.prompt.is_none() {
                                    self.flush_all_drafts();
                                    self.editor.prompt = Some(crate::editor::Prompt::DiskChanged { doc });
                                }
                                self.refresh_titles();
                                return true;
                            }
                            if self.active_document_id() == Some(doc) {
                                let anchor = self
                                    .tabs
                                    .iter()
                                    .find(|tab| tab.document_id() == Some(doc))
                                    .map(|tab| tab.view.anchor.clone())
                                    .unwrap_or_default();
                                self.begin_restore(&anchor);
                            }
                        }
                        // R-21: a watcher reload whose text came back unchanged is
                        // an attribute touch (OneDrive, a backup tool, `touch`):
                        // record the new baseline and leave the buffer, the parse,
                        // the reading anchor and the undo history alone.
                        if self.editor.compare_reloads.remove(&doc) {
                            let same = {
                                let current = document.text();
                                current.text() == loaded.buffer.text()
                                    && current.eol() == loaded.buffer.eol()
                                    && current.had_bom() == loaded.buffer.had_bom()
                            };
                            if same {
                                document.set_saved(meta, document.hash(), path);
                                return false;
                            }
                            // Different text: a real reload, so the reading
                            // place is restored after the swap (R-21).
                            if self.active_document_id() == Some(doc) {
                                let anchor = self
                                    .tabs
                                    .iter()
                                    .find(|tab| tab.document_id() == Some(doc))
                                    .map(|tab| tab.view.anchor.clone())
                                    .unwrap_or_default();
                                self.begin_restore(&anchor);
                            }
                        }
                        *document.title.borrow_mut() = title_for_path(&path);
                        *document.buf.borrow_mut() = loaded.buffer;
                        *document.status.borrow_mut() = DocStatus::Ready;
                        // OI-005: the same file reached through a symlink, a
                        // hard link or another spelling must not stay two
                        // documents. The load's canonical path and its
                        // `(dev, ino)` are the file's identity; a live document
                        // that owns either is the one view, and this tab joins
                        // it instead of inserting a second one.
                        if let Some(survivor) = self.merge_loaded_document(doc, &path, meta) {
                            // The survivor's own base may be the one that won;
                            // its length is what the size warning describes.
                            let len = self
                                .shared
                                .docs()
                                .get(survivor)
                                .map_or(meta.len, |document| document.saved.borrow().meta.len);
                            self.note_large_document(survivor, len);
                            self.sync_editor_titles();
                            self.refresh_titles();
                            self.refresh_find_hits();
                            self.drive_parse();
                            self.sync_outline();
                            self.apply_pending_scroll(tx);
                            // A Back/Forward move that opened this document is
                            // on screen now: it commits (O-13). A merged move
                            // already points at the survivor's tab.
                            self.finish_travel(survivor, true, None, tx);
                            tx.instant();
                            return true;
                        }
                        if matches!(*document.key.borrow(), DocumentKey::File(_)) {
                            self.adopt_load_path(doc, &path);
                        }
                        document.set_saved(meta, document.hash(), path.clone());
                        self.shared.docs_mut().insert_ready(document.clone());
                        // OI-010: above 8 MiB the reader says so once and the
                        // document only ever opens in viewer mode on its own.
                        self.note_large_document(doc, meta.len);
                        if !self.restored_docs.remove(&doc) {
                            self.shared.record_recent(path.clone(), unix_millis());
                        }
                        // A reload may reset the buffer revision to zero; forget
                        // the old model so the parse job cannot look current.
                        self.shared.forget_parsed(doc);
                        self.reset_views_after_load(doc);
                        self.sync_editor_titles();
                        self.request_draft_check(doc);
                        self.refresh_titles();
                        self.refresh_find_hits();
                        self.drive_parse();
                        self.sync_outline();
                        self.apply_pending_scroll(tx);
                        // A Back/Forward move that opened this document is on
                        // screen now: it commits (O-13).
                        self.finish_travel(doc, true, None, tx);
                        tx.instant();
                        true
                    }
                    Err(error) => {
                        self.editor.compare_reloads.remove(&doc);
                        self.editor.reopen_checks.remove(&doc);
                        self.restored_docs.remove(&doc);
                        let travel = self.navigation.pending.as_ref().is_some_and(|pending| pending.doc == Some(doc));
                        *document.status.borrow_mut() = DocStatus::Error(error.to_string());
                        let name = document.title.borrow().clone();
                        if !travel {
                            // R-29: the document's error card stays behind its
                            // sticky notice until another document comes forward.
                            // T-26: the notice belongs to that document's tab,
                            // so a failed background load does not speak over
                            // the document in front.
                            self.sticky_notice_for_document(
                                doc,
                                lmdv_i18n::t!("msg.openFailed", "name" => name, "err" => error),
                                NoticeKind::Error,
                            );
                            // OI-073: a deleted or unreadable file can still
                            // have a crash draft. Offer it here — the buffer is
                            // empty, so Restore keeps the text and Save again
                            // can recreate the file.
                            self.request_draft_check(doc);
                        }
                        self.finish_travel(doc, false, Some(&error), tx);
                        true
                    }
                }
            }
            JobMsg::Parsed { doc, revision, result } => self.finish_parse(doc, revision, result, tx),
            JobMsg::Saved { job, doc, generation, result } => self.handle_saved(job, doc, generation, result, tx),
            JobMsg::DraftLoaded { doc, key, draft, issue } => self.handle_draft_loaded(doc, key, draft, issue),
            JobMsg::Exported { job, doc, format, result } => self.handle_exported(job, doc, format, result),
            JobMsg::RichCopied { job, doc, result } => self.handle_rich_copied(job, doc, result, tx),
            JobMsg::Opened { path, result, .. } => self.handle_opened(path, result),
            JobMsg::TreeListed { generation, root, result, .. } => self.finish_tree_listing(generation, root, result),
            JobMsg::Searched { generation, root, query, result, .. } => {
                self.finish_search(generation, root, query, result)
            }
            JobMsg::TreeChanged { root } => {
                if self.browser.root.as_deref() == Some(root.as_path()) {
                    self.request_tree_refresh_from_watch();
                    true
                } else {
                    false
                }
            }
            JobMsg::TreeWatchFailed { root, error } => {
                if self.browser.root.as_deref() == Some(root.as_path()) {
                    if self.browser.watch_error.as_deref() != Some(error.as_str()) {
                        self.browser.watch_error = Some(error.clone());
                        self.notice_kind(lmdv_i18n::t!("msg.watchFailed", "err" => error), NoticeKind::Warning);
                    }
                    true
                } else {
                    false
                }
            }
            JobMsg::Probed { .. } => false,
            JobMsg::LinkResolved { tab, doc, dest, fragment, placement, result, .. } => {
                self.finish_link_resolve(tab, doc, dest, fragment, placement, result, tx)
            }
            JobMsg::Stored { ok, .. } => {
                if ok {
                    false
                } else {
                    self.notice_kind(lmdv_i18n::t!("msg.stateSaveFailed"), NoticeKind::Error);
                    true
                }
            }
        }
    }

    /// Joins a completed load to another live document for the same file
    /// (OI-005).
    ///
    /// The canonical path and the file's `(dev, ino)` identity are the same
    /// file; a load that reaches a document already registered under either is
    /// not a second document. The already-loaded document wins so its buffer
    /// and any edits survive; a loading duplicate has nothing to keep, so the
    /// completed read wins instead. Returns the surviving document.
    fn merge_loaded_document(&mut self, doc: DocumentId, path: &Path, meta: lmdv_core::FileMeta) -> Option<DocumentId> {
        let other = {
            let docs = self.shared.docs();
            docs.find_identity(&meta).or_else(|| docs.find_path(path))
        }?;
        if other.id == doc {
            return None;
        }
        let other_ready = matches!(*other.status.borrow(), DocStatus::Ready);
        // OI-172: an in-place replacement leaves the ready document holding
        // the older buffer. The completion is the fresher read when its
        // modification time (or, at the same time, its length) moved on.
        let completion_newer = other_ready && completion_supersedes(meta, other.saved.borrow().meta);
        let (survivor, absorbed) = if other_ready { (other.id, doc) } else { (doc, other.id) };
        let mut completion_applied = false;
        let mut disk_newer = false;

        // A survivor that is this completion still needs its canonical key,
        // saved base and identity; the duplicate's checks ran first, above.
        if survivor == doc {
            self.adopt_load_path(doc, path);
            if let Some(document) = self.shared.docs().get(doc) {
                document.set_saved(meta, document.hash(), path.to_path_buf());
                self.shared.docs_mut().insert_ready(document);
            }
        } else if completion_newer {
            let completion = self.shared.docs().get(doc);
            if let Some(completion) = completion {
                if other.is_dirty() {
                    // Keep the unsaved edits and let the disk-changed prompt
                    // decide, exactly like a watcher `Changed` (E-10/E-15).
                    disk_newer = true;
                } else {
                    self.apply_newer_completion(&other, &completion, path, meta);
                    self.adopt_load_path(survivor, path);
                    self.shared.docs_mut().insert_ready(other.clone());
                    // The cached model belongs to the replaced text.
                    self.shared.forget_parsed(survivor);
                    completion_applied = true;
                }
            }
        }

        self.remap_document_tabs(absorbed, survivor);
        // One document per file: the absorbed id leaves the registry, and its
        // model, load bookkeeping and restored-tab marker go with it. A tab
        // that still holds the `Rc` (another window, until its message lands)
        // keeps it alive harmlessly.
        self.parse_inflight.remove(&absorbed);
        self.shared.forget_parsed(absorbed);
        self.shared.docs_mut().remove(absorbed);
        let was_restored = self.restored_docs.remove(&absorbed);
        let survivor_restored = self.restored_docs.remove(&survivor);
        if !was_restored && !survivor_restored {
            self.shared.record_recent(path.to_path_buf(), unix_millis());
        }
        self.broadcast_merge(absorbed, survivor);
        if completion_applied {
            self.refresh_document_views(survivor, false);
            self.broadcast_document_refresh(survivor, false);
        } else if disk_newer {
            self.refresh_document_views(survivor, true);
            self.broadcast_document_refresh(survivor, true);
        }
        Some(survivor)
    }

    /// Points `doc` at the canonical spelling a completed load resolved
    /// (OI-005), moving its watch, place, recent entry and history with it.
    ///
    /// The watcher followed the lexical path the tab was opened with; every
    /// stored key follows the file to its canonical name.
    fn adopt_load_path(&mut self, doc: DocumentId, path: &Path) {
        let Some(document) = self.shared.docs().get(doc) else { return };
        let old_path = document.key.borrow().path().map(Path::to_path_buf);
        if old_path.as_deref() == Some(path) {
            return;
        }
        self.shared.docs_mut().rekey(doc, DocumentKey::File(path.to_path_buf()));
        if let Some(old) = &old_path {
            self.shared.watch_path(path);
            self.shared.unwatch_path(old);
            self.shared.rename_position(old, path);
            self.shared.rename_recent(old, path);
            self.rename_history_paths(old, path);
        }
    }

    /// Resets every view of `doc` to the document's current text after a load
    /// (E-10) or a merge that adopted a newer disk read (OI-172).
    ///
    /// The editor session keeps its layout and caret; only the text and the
    /// revision baseline change. The last good model stays on screen until the
    /// parse for the new revision lands.
    fn reset_views_after_load(&mut self, doc: DocumentId) {
        // R-29: a reload is a document event; the reloaded document's sticky
        // empty/error notice goes from its tab and from the window (T-26).
        self.clear_sticky_notice_for(std::slice::from_ref(&doc));
        let Some(document) = self.shared.docs().get(doc) else { return };
        let text = document.text().text().to_owned();
        let eol = document.text().eol();
        let revision = document.revision();
        for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(doc)) {
            tab.view.reader_revision = None;
            tab.view.reader_error = None;
            tab.view.dirty = false;
            tab.view.disk_changed = false;
            tab.view.edit_base = Some(text.clone());
            if let Some(editor) = tab.view.editor.as_mut() {
                // A reload keeps the session's layout: block mode never flips
                // mid-session, whatever the new size.
                let was_block = editor.block_mode_active();
                let caret = editor.selection().start.min(text.len());
                editor.set_text(&text);
                if was_block {
                    editor.enable_block_mode();
                    editor.set_selection(caret..caret);
                }
                crate::block_ui::prime_editor_focus(editor);
                editor.set_eol(eol);
                editor.set_doc_revision(revision);
                editor.set_base_revision(revision);
            }
        }
    }

    /// Applies a newer read of the same file to this window's views of `doc`
    /// (OI-172), or marks them disk-changed when the buffer keeps its edits.
    fn refresh_document_views(&mut self, doc: DocumentId, disk_changed: bool) -> bool {
        let Some(document) = self.shared.docs().get(doc) else { return false };
        if disk_changed {
            let mut marked = false;
            for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(doc)) {
                marked |= !tab.view.disk_changed;
                tab.view.disk_changed = true;
            }
            if marked && self.editor.prompt.is_none() {
                self.flush_all_drafts();
                self.editor.prompt = Some(crate::editor::Prompt::DiskChanged { doc });
            }
            return marked;
        }
        // A user typed between the merge and this message: the unsaved buffer
        // wins, exactly as it did in the merging window.
        if document.is_dirty() {
            return false;
        }
        drop(document);
        if !self.tabs.iter().any(|tab| tab.document_id() == Some(doc)) {
            return false;
        }
        self.reset_views_after_load(doc);
        self.sync_editor_titles();
        self.refresh_titles();
        self.refresh_find_hits();
        self.drive_parse();
        self.sync_outline();
        true
    }

    /// Makes another window's Save as or a watcher-observed rename reach this
    /// window's Back/Forward places and tab titles (OI-173/OI-180).
    ///
    /// Positions and recents are process-wide and the saving window already
    /// moved them; watches are process-wide too. Only the per-tab places and
    /// the cached title are this window's own state.
    fn apply_document_rename(&mut self, doc: DocumentId, from: &Path, to: &Path) -> bool {
        let old_key = from.to_string_lossy();
        let new_key = to.to_string_lossy();
        let mut changed = false;
        for tab in &mut self.tabs {
            let holds_old =
                tab.view.nav.back.iter().chain(tab.view.nav.forward.iter()).any(|place| place.path.as_str() == old_key);
            if holds_old {
                tab.view.nav.rename_path(&old_key, &new_key);
                changed = true;
            }
        }
        for tab in self.tabs.iter_mut().filter(|tab| tab.document_id() == Some(doc)) {
            let before = tab.title();
            tab.refresh_title();
            changed |= tab.title() != before;
        }
        changed
    }

    /// Tells every other window that `doc` moved from `from` to `to` (OI-173's
    /// Save as and OI-180's watcher rename).
    ///
    /// Positions, recents and watches are process-wide and the moving window
    /// already handled them; what is per window is each tab's Back/Forward
    /// places and the cached title.
    pub(crate) fn broadcast_window_rename(&self, doc: DocumentId, from: &Path, to: &Path) {
        let ids: Vec<WindowId> =
            self.shared.windows().iter().map(|entry| entry.id).filter(|id| *id != self.id).collect();
        for id in ids {
            let _ = self
                .shared
                .try_post_window(id, UiMsg::DocumentRenamed { doc, from: from.to_path_buf(), to: to.to_path_buf() });
        }
    }

    /// Tells every other window that `doc` adopted a newer disk read, or that
    /// its dirty buffer now sits under changed content (OI-172).
    fn broadcast_document_refresh(&self, doc: DocumentId, disk_changed: bool) {
        let ids: Vec<WindowId> =
            self.shared.windows().iter().map(|entry| entry.id).filter(|id| *id != self.id).collect();
        for id in ids {
            let _ = self.shared.try_post_window(id, UiMsg::DocumentRefreshed { doc, disk_changed });
        }
    }

    /// Replaces a ready document's buffer with a newer read of the same file
    /// (OI-172). The survivor keeps its id, tabs, lease and reading position.
    fn apply_newer_completion(&mut self, survivor: &Document, completion: &Document, path: &Path, meta: FileMeta) {
        let (text, eol, bom) = {
            let buffer = completion.text();
            (buffer.text().to_owned(), buffer.eol(), buffer.had_bom())
        };
        if survivor.text().text() != text {
            survivor.replace_all(&text, eol);
            survivor.buf.borrow_mut().set_had_bom(bom);
        }
        *survivor.title.borrow_mut() = title_for_path(path);
        *survivor.status.borrow_mut() = DocStatus::Ready;
        survivor.set_saved(meta, survivor.hash(), path.to_path_buf());
    }

    /// Points this window's views of `from` at `to` (OI-005).
    ///
    /// One tab per document per window (T-11): while the survivor already has
    /// a tab here, the duplicate view closes and the survivor comes forward.
    /// Otherwise the loading tab itself becomes the survivor's view, keeping
    /// its strip place, anchor and history.
    fn remap_document_tabs(&mut self, from: DocumentId, to: DocumentId) -> bool {
        let Some(survivor) = self.shared.docs().get(to) else { return false };
        let from_tabs: Vec<TabId> =
            self.tabs.iter().filter(|tab| tab.document_id() == Some(from)).map(|tab| tab.id).collect();
        if from_tabs.is_empty() {
            return false;
        }
        if let Some(index) = self.index_of_document(to) {
            let survivor_tab = self.tabs[index].id;
            if let Some(pending) = self.navigation.pending.as_mut()
                && pending.tab.is_some_and(|tab| from_tabs.contains(&tab))
            {
                // The travel's tab is this duplicate view: the survivor's tab
                // is the place now, and its completion commits the move.
                pending.tab = Some(survivor_tab);
                pending.doc = Some(to);
            }
            for tab in from_tabs {
                self.release_duplicate_view(tab);
            }
            let _ = self.activate_tab(survivor_tab);
            return true;
        }
        if let Some(pending) = self.navigation.pending.as_mut()
            && pending.doc == Some(from)
        {
            // The same tab now shows the survivor; the move still commits.
            pending.doc = Some(to);
        }
        // A tab that entered edit mode while the load completed elsewhere
        // holds a lease on the absorbed document; leave that session before
        // the tab shows the survivor.
        if self.tabs.iter().any(|tab| tab.document_id() == Some(from) && tab.view.mode == Mode::Editor) {
            self.leave_editor_for(from);
        }
        let new_path = survivor.key.borrow().path().map(Path::to_path_buf);
        for tab_id in from_tabs {
            let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else { continue };
            let absorbed = self.tabs[index].document().cloned();
            let old_path = absorbed.as_ref().and_then(|doc| doc.key.borrow().path().map(Path::to_path_buf));
            if let (Some(old), Some(new)) = (old_path.as_deref(), new_path.as_deref())
                && old != new
            {
                let old_key = old.to_string_lossy().into_owned();
                let new_key = new.to_string_lossy();
                self.tabs[index].view.nav.rename_path(&old_key, &new_key);
            }
            self.tabs[index].source = TabSource::Document(survivor.clone());
            self.tabs[index].view.reader = None;
            self.tabs[index].view.reader_revision = None;
            self.tabs[index].view.reader_error = None;
            self.tabs[index].view.dirty = survivor.is_dirty();
            self.tabs[index].view.disk_changed = false;
            self.tabs[index].refresh_title();
            if let Some(cached) = self.shared.latest_parsed(to)
                && let Ok(model) = cached.result
            {
                self.tabs[index].view.reader = Some(model);
                self.tabs[index].view.reader_revision = Some(cached.revision);
                self.tabs[index].view.reader_error = None;
            }
            if let Some(absorbed) = absorbed {
                let last_view = !self.tabs.iter().any(|tab| tab.document_id() == Some(absorbed.id));
                if last_view {
                    absorbed.subscribers.borrow_mut().remove(&self.id);
                    if absorbed.subscribers.borrow().is_empty() {
                        self.shared.forget_parsed(absorbed.id);
                    }
                }
            }
            survivor.subscribers.borrow_mut().insert(self.id);
            if old_path != new_path {
                if let Some(old_path) = old_path {
                    self.shared.unwatch_path(&old_path);
                }
                if let Some(new_path) = &new_path {
                    self.shared.watch_path(new_path);
                }
            }
        }
        true
    }

    /// Tells every other window to point its views of `from` at `to`.
    fn broadcast_merge(&self, from: DocumentId, to: DocumentId) {
        let ids: Vec<WindowId> =
            self.shared.windows().iter().map(|entry| entry.id).filter(|id| *id != self.id).collect();
        for id in ids {
            let _ = self.shared.try_post_window(id, UiMsg::DocumentMerged { from, to });
        }
    }

    /// Says once that `doc` is a large document and keeps editing explicit
    /// (OI-010).
    ///
    /// A restored edit mode is dropped before the first poll applies it: a
    /// document above the threshold only opens in the reader on its own, and
    /// `Ctrl+E` remains the explicit way in.
    fn note_large_document(&mut self, doc: DocumentId, len: u64) {
        if !lmdv_io::is_large_document(len) {
            return;
        }
        let tabs: Vec<TabId> =
            self.tabs.iter().filter(|tab| tab.document_id() == Some(doc)).map(|tab| tab.id).collect();
        for tab in tabs {
            self.pending_editor_tabs.remove(&tab);
        }
        if !self.shared.mark_large_warned(doc) {
            return;
        }
        let name = self.shared.docs().get(doc).map(|document| document.title.borrow().clone()).unwrap_or_default();
        let mib = lmdv_io::LARGE_DOC_BYTES.div_ceil(1024 * 1024);
        self.notice_kind(lmdv_i18n::t!("msg.largeDocument", "name" => name, "mib" => mib), NoticeKind::Warning);
    }

    /// Routes one forwarded launch on the router window.
    ///
    /// Exactly one window drains the shared launch queue, so a burst of
    /// launches is decided once each. A launch that resolves to this window is
    /// opened in the same frame; another window receives it in its mailbox.
    fn route_launch(&mut self, launch: Launch) -> bool {
        match Rc::clone(&self.shared).route_launch(launch.clone(), Some(self.id)) {
            crate::windows::RouteAction::OpenHere => {
                self.open_launch(&launch);
                true
            }
            crate::windows::RouteAction::Posted(_) | crate::windows::RouteAction::Opened(_) => true,
            crate::windows::RouteAction::Failed(error) => {
                self.notice_kind(lmdv_i18n::t!("msg.openWindowFailed", "err" => error), NoticeKind::Error);
                true
            }
        }
    }

    fn handle_ui(&mut self, msg: UiMsg) -> bool {
        match msg {
            UiMsg::Open(path) => {
                self.open_path(path);
                true
            }
            UiMsg::OpenLaunch(launch) => {
                self.open_launch(&launch);
                true
            }
            UiMsg::ActivateDocument(doc) => match self.index_of_document(doc) {
                Some(index) => self.switch_to_index(index),
                None => false,
            },
            UiMsg::PrefsChanged => {
                // Every window derives its appearance and theme again; the
                // controls read the shared preferences per frame.
                self.invalidate_appearance();
                true
            }
            UiMsg::CloseWindow(target) if target == self.id => {
                self.request_close_window();
                true
            }
            // The router already asked the native window to come forward; the
            // message settles this window's frame and refreshes its placement.
            UiMsg::FocusWindow(target) if target == self.id => {
                self.shared.refresh_placement(self.id);
                true
            }
            UiMsg::FocusWindow(_) => false,
            UiMsg::Transfer(msg) => self.handle_transfer(msg),
            UiMsg::ReleaseDocument { doc } => self.release_clean_views(doc),
            UiMsg::DocumentMerged { from, to } => self.remap_document_tabs(from, to),
            UiMsg::DocumentRefreshed { doc, disk_changed } => self.refresh_document_views(doc, disk_changed),
            UiMsg::DocumentRenamed { doc, from, to } => self.apply_document_rename(doc, &from, &to),
            UiMsg::CloseWindow(_) => false,
        }
    }

    fn pump_timers(&mut self, now: f64) -> bool {
        self.ui.last_now = now;
        self.ui.now_unix_ms = unix_millis();
        if self.ui.notice.as_ref().is_some_and(|notice| now >= notice.deadline) {
            self.ui.notice = None;
            true
        } else {
            false
        }
    }
    fn refresh_titles(&mut self) {
        for tab in &mut self.tabs {
            tab.refresh_title();
        }
    }

    /// Adopts the shared model for a tab when it is already parsed.
    pub(crate) fn adopt_cached_parse(&self, tab: &mut Tab, doc: &Document) {
        let Some(cached) = self.shared.latest_parsed(doc.id) else { return };
        let Ok(model) = cached.result else { return };
        if cached.revision < tab.view.reader_revision.unwrap_or(0) {
            return;
        }
        tab.view.reader = Some(model);
        tab.view.reader_revision = Some(cached.revision);
        tab.view.reader_error = None;
    }

    /// Requests the active tab's model when it is missing or stale.
    ///
    /// One parse per document revision: the shared cache answers when another
    /// window (or a scene) already parsed it, `parse_inflight` prevents a
    /// second job for the same revision, and a newer revision makes the old
    /// completion stale. Oversized documents never reach the parser and keep
    /// the source preview.
    fn drive_parse(&mut self) -> bool {
        let Some(tab) = self.active_tab() else { return false };
        let Some(document) = tab.document().cloned() else { return false };
        if !matches!(*document.status.borrow(), DocStatus::Ready) {
            return false;
        }
        let revision = document.revision();
        if tab.view.reader_revision == Some(revision) {
            return false;
        }
        // Typing parses on a 120 ms debounce (E-05); the last good model stays
        // on screen until the new one lands.
        if tab.view.mode == Mode::Editor && tab.view.preview_due.is_some_and(|due| self.ui.last_now < due) {
            return false;
        }
        if let Some(result) = self.shared.parsed(document.id, revision) {
            let tab_id = tab.id;
            return self.adopt_parsed(tab_id, document.id, revision, result);
        }
        let (bytes, lines) = {
            let buffer = document.text();
            (buffer.text().len(), buffer.line_count())
        };
        if bytes > reader::PARSE_MAX_BYTES || lines > reader::PARSE_MAX_LINES {
            let reason = lmdv_i18n::t!("msg.tooLarge", "bytes" => bytes, "lines" => lines);
            let Some(tab) = self.active_tab_mut() else { return false };
            if tab.view.reader_error.as_deref() == Some(reason.as_str()) {
                return false;
            }
            tab.view.reader_error = Some(reason);
            return true;
        }
        let source = document.snapshot();
        if self.shared.is_headless() {
            // One settled frame, no worker race: `--snap`/`--tree` gates show
            // the article, not “Preparing…”. Interactive windows never run a
            // parse here.
            let tab_id = tab.id;
            let result = Ok(Arc::new(lmdv_md::parse_with_revision(&source, revision)));
            self.shared.store_parsed(document.id, revision, result.clone());
            return self.adopt_parsed(tab_id, document.id, revision, result);
        }
        if self.parse_inflight.get(&document.id) == Some(&revision) {
            return false;
        }
        self.parse_inflight.insert(document.id, revision);
        self.shared.jobs().parse(self.worker.clone(), document.id, revision, source);
        true
    }

    /// Adopts one parse result into a tab that still shows `doc`.
    fn adopt_parsed(
        &mut self,
        tab_id: TabId,
        doc: DocumentId,
        revision: u64,
        result: Result<Arc<MdDoc>, String>,
    ) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else { return false };
        if tab.document_id() != Some(doc) || tab.view.reader_revision.is_some_and(|current| current > revision) {
            return false;
        }
        match result {
            Ok(model) => {
                let changed = tab.view.reader.as_ref().map(|reader| reader.revision) != Some(model.revision)
                    || tab.view.reader_error.is_some();
                tab.view.reader = Some(model);
                tab.view.reader_revision = Some(revision);
                tab.view.reader_error = None;
                crate::reader_find::refresh_tab(tab);
                changed
            }
            Err(message) => {
                if tab.view.reader_error.as_deref() == Some(message.as_str()) {
                    return false;
                }
                tab.view.reader_error = Some(message);
                true
            }
        }
    }

    /// Accepts a [`JobMsg::Parsed`] completion for every tab showing `doc`.
    fn finish_parse(
        &mut self,
        doc: DocumentId,
        revision: u64,
        result: Result<Arc<MdDoc>, String>,
        tx: &mut Tx,
    ) -> bool {
        if self.parse_inflight.get(&doc) == Some(&revision) {
            self.parse_inflight.remove(&doc);
        }
        let Some(document) = self.shared.docs().get(doc) else { return false };
        let current = document.revision();
        drop(document);
        // A completion for a revision the buffer has already left is only
        // useful as the last-good model, never as the current model.
        let store = revision <= current;
        let mut changed = false;
        if store {
            self.shared.store_parsed(doc, revision, result.clone());
        }
        let tabs: Vec<TabId> =
            self.tabs.iter().filter(|tab| tab.document_id() == Some(doc)).map(|tab| tab.id).collect();
        for tab in tabs {
            let adopt = if result.is_ok() { revision <= current } else { revision == current };
            if adopt {
                changed |= self.adopt_parsed(tab, doc, revision, result.clone());
            }
        }
        if changed {
            self.sync_outline();
            self.drive_parse();
            self.apply_pending_scroll(tx);
            self.sync_preview_to_caret(tx);
        }
        changed
    }

    /// Publishes this window's title, active document, document set and dirty
    /// flags after a `poll`. Other windows and the launch router read the
    /// registry instead of hashing every buffer.
    pub(crate) fn sync_registry(&self) {
        let title = self.window_title();
        let active_document = self.active_document_id();
        let documents: BTreeSet<DocumentId> = self.tabs.iter().filter_map(Tab::document_id).collect();
        let dirty: BTreeSet<DocumentId> =
            self.tabs.iter().filter(|tab| tab.view.dirty).filter_map(Tab::document_id).collect();
        let only_start = documents.is_empty();
        let prompt_open = self.editor.prompt.is_some();
        self.shared.publish_window(
            self.id,
            WindowSnapshot { title, active_document, documents, dirty, only_start, prompt_open },
        );
    }

    /// Starts a generation for `doc`; test-only view of the load tracker.
    #[cfg(test)]
    pub(crate) fn tracker_begin(&mut self, doc: DocumentId) -> u64 {
        self.loads.begin(doc)
    }

    /// Finishes a generation; test-only view of the load tracker.
    #[cfg(test)]
    pub(crate) fn tracker_finish(&mut self, doc: DocumentId, generation: u64) -> bool {
        self.loads.finish(doc, generation)
    }
}

impl Drop for WindowState {
    fn drop(&mut self) {
        // A tab in flight is no longer this window's view: release only the
        // subscriber entry and a lease that pointed at the lost tab.
        self.forget_pending_views();
        let last = self.shared.windows().len() <= 1;
        self.flush_all_drafts();
        self.capture_active_position();
        if last {
            self.shared.set_quitting();
        }
        // If no close hook ran (the host was dropped), repeat the session
        // decision here. A window that closed while others stay leaves the
        // session.
        if !self.session_flushed {
            self.shared.refresh_placement(self.id);
            if last || self.shared.is_quitting() {
                let _ = self.publish_session();
                self.shared.queue_session(true);
            } else {
                self.shared.remove_session_window(self.id());
                self.shared.queue_session(false);
            }
            self.session_flushed = true;
        }
        let tabs = std::mem::take(&mut self.tabs);
        for tab in tabs {
            self.release_tab(tab);
        }
        self.loads.clear();
        self.shared.unregister_window(self.id);
        self.shared.docs_mut().collect_unused();
        // Drafts and the coalesced state queued above must reach disk before
        // the process ends; on a quit the store flush cannot wait for the
        // skipped `on_quit`.
        if self.shared.windows().is_empty() {
            self.shared.set_quitting();
            self.shared.flush_store(Duration::from_millis(750));
        }
    }
}

/// The strip key for a tab (`tab-7`).
pub fn tab_key(id: TabId) -> String {
    format!("tab-{id}")
}

/// Parses a strip key back to a tab id.
pub fn parse_tab_key(key: &str) -> Option<TabId> {
    key.strip_prefix("tab-")?.parse().ok()
}

/// The display title for a path: its final component.
pub fn title_for_path(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether an external drop may open `path` (D-02: Markdown only).
///
/// The rule is the file browser's own [`lmdv_io::is_markdown_name`], so a drop
/// and the tree agree on `.md`, `.markdown` and the rest.
pub(crate) fn is_markdown_path(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(lmdv_io::is_markdown_name)
}

/// Remembers the modifiers a window holds so a link click can place its target
/// (T-12/L-11). `WindowSpec::on_modifiers` calls this on every change.
pub(crate) fn modifiers_changed(state: &mut WindowState, mods: rune::Mods, _: &mut Tx) {
    state.set_link_modifiers(mods);
}

/// Whether a completed read describes newer content than a document's saved
/// base (OI-172).
///
/// The modification time decides. When both sides report the same time, a
/// different length still means the disk changed; identical metadata means the
/// two reads are the same content and the ready document keeps its buffer.
fn completion_supersedes(completion: FileMeta, saved: FileMeta) -> bool {
    if completion.mtime_ns != saved.mtime_ns {
        return completion.mtime_ns > saved.mtime_ns;
    }
    completion.len != saved.len
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::ScrollSnapshot;
    use crate::test_support;
    use crate::view::view;
    use lmdv_core::Place;
    use rune::{Env, Runtime};
    use tempfile::TempDir;

    fn window(temp: &TempDir) -> WindowState {
        WindowState::new(test_support::shared(temp))
    }

    fn ready_doc(shared: &AppShared, path: &str, text: &str) -> Rc<Document> {
        let key = DocumentKey::File(PathBuf::from(path));
        let id = shared.docs_mut().allocate_id();
        let doc = Rc::new(Document::ready(id, key, title_for_path(Path::new(path)), SourceBuffer::new(text)));
        shared.docs_mut().insert_ready(doc.clone());
        doc
    }

    #[test]
    fn a_new_window_has_one_start_page() {
        let temp = TempDir::new().unwrap();
        let state = window(&temp);
        assert_eq!(state.tabs().len(), 1);
        assert!(state.active_tab().is_some_and(Tab::is_start));
        assert_eq!(state.active_tab().map(Tab::title).as_deref(), Some("New tab"));
        assert_eq!(state.window_title(), "lmdv");
        assert_eq!(state.shared.windows().len(), 1);
    }

    #[test]
    fn the_window_title_quotes_the_name_and_prefixes_the_dirty_dot() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        ready_doc(&shared, "/tmp/lmdv-app-test/notes.md", "# Notes\n");
        let tab = state.open_path(PathBuf::from("/tmp/lmdv-app-test/notes.md"));

        assert_eq!(state.window_title(), "\u{201c}notes.md\u{201d} \u{2014} lmdv", "W-09: quotes, no dot when clean");
        assert!(!state.window_title().contains('\u{2022}'));

        state.tabs.iter_mut().find(|candidate| candidate.id == tab).expect("the tab").view.dirty = true;
        assert_eq!(
            state.window_title(),
            "\u{2022} \u{201c}notes.md\u{201d} \u{2014} lmdv",
            "W-09: the dirty dot prefixes the quoted name"
        );

        state.tabs.iter_mut().find(|candidate| candidate.id == tab).expect("the tab").view.dirty = false;
        assert_eq!(state.window_title(), "\u{201c}notes.md\u{201d} \u{2014} lmdv", "clean again clears the dot");

        state.new_start_tab();
        assert_eq!(state.window_title(), "lmdv", "W-09: a start page is exactly the app name");
    }

    #[test]
    fn a_document_empty_in_the_registry_keeps_the_plain_title() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let id = shared.docs_mut().allocate_id();
        let doc = Rc::new(Document::ready(
            id,
            DocumentKey::Untitled(lmdv_core::PasteId(0)),
            "",
            SourceBuffer::new("no title"),
        ));
        shared.docs_mut().insert_ready(doc.clone());
        let tab = state.attach_document(doc);

        assert!(state.activate_tab(tab));
        assert_eq!(state.window_title(), "lmdv", "an untitled document does not read as `“” — lmdv`");
    }

    #[test]
    fn new_tab_appends_at_the_end_and_brings_it_forward() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.active_tab().map(|tab| tab.id).expect("start tab");
        let second = state.new_start_tab();

        assert_eq!(state.tabs().len(), 2);
        assert_eq!(state.active_index(), 1);
        assert_ne!(first, second);
        assert!(state.active_tab().is_some_and(Tab::is_start));
    }

    #[test]
    fn the_tab_menus_new_tab_inserts_after_its_tab() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let one = state.open_synthetic("One", "one");
        let two = state.open_synthetic("Two", "two");
        let three = state.open_synthetic("Three", "three");

        // T-04: the menu's New tab inserts after the tab it was opened on.
        let inserted = state.new_start_tab_after(one);
        assert_eq!(state.active_index(), 1);
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(inserted));
        assert_eq!(state.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>(), [one, inserted, two, three]);

        // Ctrl+T still appends at the end.
        let appended = state.new_start_tab();
        assert_eq!(state.tabs().last().map(|tab| tab.id), Some(appended));
        assert_eq!(state.active_index(), 4);
    }

    #[test]
    fn moving_the_front_tab_one_slot_clamps_at_the_ends() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("One", "one");
        state.open_synthetic("Two", "two");
        state.open_synthetic("Three", "three");
        state.activate_index(0);

        assert!(!state.move_active_tab(-1), "the first tab cannot move left");
        assert!(state.move_active_tab(1));
        assert_eq!(state.tabs().iter().map(Tab::title).collect::<Vec<_>>(), ["Two", "One", "Three"]);
        assert_eq!(state.active_index(), 1);
        assert!(state.move_active_tab(1));
        assert_eq!(state.active_index(), 2);
        assert!(!state.move_active_tab(1), "the last tab cannot move right");
        assert_eq!(state.tabs().iter().map(Tab::title).collect::<Vec<_>>(), ["Two", "Three", "One"]);
    }

    #[test]
    fn a_document_replaces_the_front_start_page() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Notes", "# Notes\n");

        assert_eq!(state.tabs().len(), 1, "the start page was replaced");
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(tab));
        assert_eq!(state.active_tab().map(Tab::title).as_deref(), Some("Notes"));
    }

    #[test]
    fn a_background_tab_pulses_until_it_is_activated_or_the_interval_ends() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let front = state.open_synthetic("Front", "# Front\n");
        ready_doc(&shared, "/tmp/lmdv-app-test/behind.md", "# Behind\n");
        let behind = state.open_path_background(PathBuf::from("/tmp/lmdv-app-test/behind.md"));

        // T-15: opening behind rings and marks the tab Unread.
        let pulse = state.tab(behind).and_then(|tab| tab.view.pulse).expect("the background tab pulses");
        assert_eq!(pulse.until, TAB_PULSE_SECONDS);
        assert!(state.tab(front).is_some_and(|tab| tab.view.pulse.is_none()), "the front tab does not");

        // Activation answers the mark at once.
        assert!(state.activate_tab(behind));
        assert!(state.tab(behind).is_some_and(|tab| tab.view.pulse.is_none()), "activation clears the pulse");

        // T-15/T-11: opening the same document behind again pulses its tab
        // rather than silently doing nothing, once another tab is in front.
        assert!(state.activate_tab(front));
        assert_eq!(state.open_path_background(PathBuf::from("/tmp/lmdv-app-test/behind.md")), behind);
        assert!(state.tab(behind).is_some_and(|tab| tab.view.pulse.is_some()), "the existing tab pulses");

        // A second background tab expires on the clock and wakes the frame.
        ready_doc(&shared, "/tmp/lmdv-app-test/later.md", "# Later\n");
        let later = state.open_path_background(PathBuf::from("/tmp/lmdv-app-test/later.md"));
        let until = state.tab(later).and_then(|tab| tab.view.pulse).map(|pulse| pulse.until).expect("a pulse");
        let mut tx = Tx::default();
        state.poll(until + 0.01, &mut tx);
        assert!(state.tab(later).is_some_and(|tab| tab.view.pulse.is_none()), "the interval clears the mark");
    }

    #[test]
    fn a_background_open_does_not_consume_the_front_start_page() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let path = PathBuf::from("/tmp/lmdv-app-test/background.md");
        ready_doc(&shared, "/tmp/lmdv-app-test/background.md", "text");
        let start = state.active_tab().map(|tab| tab.id).expect("a start page is in front");

        let tab = state.open_path_background(path);

        assert_eq!(state.tabs().len(), 2, "the start page stayed and the file joined it");
        assert_eq!(state.tabs()[0].id, start, "the start page keeps its place");
        assert!(state.tabs()[0].is_start());
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(start), "the front tab did not change");
        assert_eq!(state.tab(tab).map(Tab::title).as_deref(), Some("background"), "the file opened behind");
    }

    #[test]
    fn background_tabs_open_after_their_origin_and_queue_until_another_tab_comes_forward() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        for name in ["front", "tail", "a", "b", "c"] {
            ready_doc(&shared, &format!("/tmp/lmdv-app-test/{name}.md"), name);
        }
        let front = state.open_path(PathBuf::from("/tmp/lmdv-app-test/front.md"));
        state.open_path(PathBuf::from("/tmp/lmdv-app-test/tail.md"));
        assert_eq!(state.tabs().iter().map(Tab::title).collect::<Vec<_>>(), ["front", "tail"]);
        assert!(state.activate_tab(front));

        // T-14: each background tab joins after the one it was opened from,
        // and the next one queues after it, not at the end of the strip.
        let a = state.open_path_background(PathBuf::from("/tmp/lmdv-app-test/a.md"));
        let b = state.open_path_background(PathBuf::from("/tmp/lmdv-app-test/b.md"));
        assert_eq!(
            state.tabs().iter().map(Tab::title).collect::<Vec<_>>(),
            ["front", "a", "b", "tail"],
            "the queue sits between the origin and the tab that was after it"
        );
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(front), "a background open never comes forward");
        assert_eq!(state.tabs().get(2).map(|tab| tab.id), Some(b));

        // Opening an already-open document behind the front changes nothing
        // and does not move the queue.
        assert_eq!(state.open_path_background(PathBuf::from("/tmp/lmdv-app-test/a.md")), a);
        assert_eq!(state.tabs().iter().map(Tab::title).collect::<Vec<_>>(), ["front", "a", "b", "tail"]);

        // Another tab coming forward starts a new queue: the next background
        // tab joins right after it.
        assert!(state.activate_tab(b));
        state.open_path_background(PathBuf::from("/tmp/lmdv-app-test/c.md"));
        assert_eq!(state.tabs().iter().map(Tab::title).collect::<Vec<_>>(), ["front", "a", "b", "c", "tail"]);
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(b));
    }

    #[test]
    fn files_opened_from_outside_still_go_at_the_end_after_a_background_queue() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        for name in ["front", "a", "outside"] {
            ready_doc(&shared, &format!("/tmp/lmdv-app-test/{name}.md"), name);
        }
        let front = state.open_path(PathBuf::from("/tmp/lmdv-app-test/front.md"));
        state.open_path_background(PathBuf::from("/tmp/lmdv-app-test/a.md"));
        assert!(state.activate_tab(front));
        state.open_path(PathBuf::from("/tmp/lmdv-app-test/outside.md"));

        assert_eq!(
            state.tabs().iter().map(Tab::title).collect::<Vec<_>>(),
            ["front", "a", "outside"],
            "T-14: a foreground file open still appends"
        );
    }

    #[test]
    fn a_document_never_replaces_a_pasted_tab_in_front() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let pasted = state.open_synthetic("Pasted notes", "# Budget\n");
        let path = PathBuf::from("/tmp/lmdv-app-test/after-paste.md");
        ready_doc(&shared, "/tmp/lmdv-app-test/after-paste.md", "text");

        let opened = state.open_path(path);

        assert_eq!(state.tabs().len(), 2, "the pasted tab stayed and the file joined it");
        assert_eq!(state.tabs()[0].id, pasted, "pasted text is never replaced (T-13)");
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(opened), "the file comes forward");
    }

    #[test]
    fn the_saved_check_follows_the_flash_and_typing_clears_it() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Notes", "# Notes\n");
        let doc = state.active_document_id().expect("a document");
        assert!(!state.tab_shows_saved_check(state.tab(tab).expect("the tab")), "nothing saved yet");

        state.editor.saved_flash_until = Some(10.0);
        state.saved_flash_docs.insert(doc);
        assert!(state.tab_shows_saved_check(state.tab(tab).expect("the tab")), "the check follows the flash");

        state.active_tab_mut().expect("the tab").view.dirty = true;
        assert!(!state.tab_shows_saved_check(state.tab(tab).expect("the tab")), "typing takes the check away at once");

        state.active_tab_mut().expect("the tab").view.dirty = false;
        let mut tx = Tx::default();
        assert!(state.poll(11.0, &mut tx), "the flash expiry changes the strip");
        assert!(state.saved_flash_docs.is_empty(), "the expiry clears the check");
        assert!(!state.tab_shows_saved_check(state.tab(tab).expect("the tab")));
    }

    #[test]
    fn a_save_completion_names_the_written_document_for_the_check() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.open_synthetic("One", "one\n");
        let second = state.open_synthetic("Two", "two\n");
        let written = state.tab(first).and_then(|tab| tab.document()).cloned().expect("the first document");
        let writes_before: Vec<(DocumentId, Option<FileMeta>)> = state
            .tabs()
            .iter()
            .filter_map(|tab| tab.document().map(|doc| (doc.id, doc.last_local_write.get())))
            .collect();

        // A background close-batch save writes the first document, not the front one.
        written.last_local_write.set(Some(FileMeta { len: 4, ..FileMeta::default() }));
        state.editor.saved_flash_until = Some(5.0);
        assert!(state.track_saved_flash(None, &writes_before));

        assert!(state.tab_shows_saved_check(state.tab(first).expect("the first tab")), "the written tab checks");
        assert!(!state.tab_shows_saved_check(state.tab(second).expect("the second tab")), "an untouched tab does not");
        assert_eq!(state.saved_flash_docs, BTreeSet::from([written.id]));
    }

    #[test]
    fn opening_a_file_twice_activates_its_existing_tab() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let path = PathBuf::from("/tmp/lmdv-app-test/note.md");
        ready_doc(&shared, "/tmp/lmdv-app-test/note.md", "text");

        let first = state.open_path(path.clone());
        let second = state.open_path(path);
        assert_eq!(first, second);
        assert_eq!(state.tabs().len(), 1, "one tab per document per window");
        assert!(state.active_document_id().is_some());
    }

    #[test]
    fn a_document_appends_when_the_front_tab_is_not_a_start_page() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.open_synthetic("One", "one");
        let second = state.open_synthetic("Two", "two");
        assert_ne!(first, second);
        assert_eq!(state.tabs().len(), 2);
        assert_eq!(state.active_index(), 1);
    }

    fn ready_files(shared: &AppShared, temp: &TempDir, names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| {
                let path = temp.path().join(name);
                ready_doc(shared, path.to_str().expect("a UTF-8 test path"), "text");
                path
            })
            .collect()
    }

    #[test]
    fn a_markdown_drop_opens_the_first_in_front_and_the_rest_behind() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let paths = ready_files(&shared, &temp, &["one.md", "two.markdown", "three.MD"]);
        state.open_synthetic("Existing", "existing");
        let existing = state.active_tab().map(|tab| tab.id).expect("a document");

        state.open_dropped_at(&paths, None);

        assert_eq!(state.tabs().len(), 4, "one tab per dropped file");
        let titles: Vec<String> = state.tabs().iter().map(Tab::title).collect();
        assert_eq!(titles, ["Existing", "one", "two", "three"]);
        assert_eq!(state.active_tab().map(Tab::title).as_deref(), Some("one"), "the first file is in front");
        assert_eq!(state.tabs()[0].id, existing, "the old front tab keeps its place");
        assert_eq!(state.tabs()[2].title(), "two", "the remaining files joined behind");
        assert_eq!(state.tabs()[3].title(), "three");
    }

    #[test]
    fn a_drop_of_a_file_already_shown_brings_that_tab_forward_and_never_duplicates_it() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let paths = ready_files(&shared, &temp, &["one.md", "two.md"]);
        state.open_synthetic("Existing", "existing");
        let first = state.open_path(paths[0].clone());
        state.open_synthetic("Front", "front");
        assert_ne!(state.active_tab().map(|tab| tab.id), Some(first));

        state.open_dropped_at(&paths, None);

        assert_eq!(state.tabs().iter().filter(|tab| tab.id == first).count(), 1, "no duplicate");
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(first), "the dropped document comes forward");
        assert_eq!(state.tabs().last().map(Tab::title).as_deref(), Some("two"), "the rest still joins behind");
    }

    #[test]
    fn a_strip_drop_inserts_at_the_index_and_successive_files_follow() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let paths = ready_files(&shared, &temp, &["a.md", "b.md", "c.md"]);
        state.open_synthetic("One", "one");
        state.open_synthetic("Two", "two");
        state.open_synthetic("Three", "three");
        state.activate_index(2);

        state.open_dropped_at(&paths, Some(1));

        let titles: Vec<String> = state.tabs().iter().map(Tab::title).collect();
        assert_eq!(titles, ["One", "a", "b", "c", "Two", "Three"]);
        assert_eq!(state.active_index(), 1, "the first dropped file takes the drop index");
        assert_eq!(state.active_tab().map(Tab::title).as_deref(), Some("a"));
    }

    #[test]
    fn a_strip_drop_past_the_last_tab_appends_after_it() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let paths = ready_files(&shared, &temp, &["a.md"]);
        state.open_synthetic("One", "one");
        state.open_synthetic("Two", "two");

        state.open_dropped_at(&paths, Some(2));

        let titles: Vec<String> = state.tabs().iter().map(Tab::title).collect();
        assert_eq!(titles, ["One", "Two", "a"]);
    }

    #[test]
    fn the_strip_drop_index_reads_the_measured_tab_rects() {
        let temp = TempDir::new().unwrap();
        let state = window(&temp);
        // A strip from y 40 to 68; tabs 80 points wide, the last one longer.
        state.drop.record_geometry(
            Some([0.0, 40.0, 1000.0, 28.0]),
            &[(0, [8.0, 40.0, 80.0, 28.0]), (1, [96.0, 40.0, 80.0, 28.0]), (2, [184.0, 40.0, 200.0, 28.0])],
        );

        assert_eq!(state.drop.strip_index(0.0, 50.0), Some(0), "left of the first tab");
        assert_eq!(state.drop.strip_index(40.0, 50.0), Some(0), "left of its middle");
        assert_eq!(state.drop.strip_index(60.0, 50.0), Some(1), "past its middle");
        assert_eq!(state.drop.strip_index(120.0, 50.0), Some(1));
        assert_eq!(state.drop.strip_index(200.0, 50.0), Some(2));
        assert_eq!(state.drop.strip_index(1000.0, 50.0), Some(3), "past the last tab");
        assert_eq!(state.drop.strip_index(50.0, 100.0), None, "below the tab band is a window drop");
        assert_eq!(state.drop.strip_index(1200.0, 50.0), None, "outside the band's right edge");
        assert_eq!(state.drop.strip_index(-10.0, 50.0), None, "outside its left edge");
    }

    #[test]
    fn only_markdown_paths_are_dropped_in() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let paths = [temp.path().join("a.md"), temp.path().join("b.MARKDOWN"), temp.path().join("c.txt")];
        let start = state.tabs().len();
        assert!(
            matches!(state.active_tab().map(|tab| &tab.source), Some(TabSource::Start)),
            "a start page is in front"
        );

        state.open_dropped_at(&paths, None);

        assert_eq!(state.tabs().len(), start + 1, "the first file replaces the start page and the second joins it");
        let titles: Vec<String> = state.tabs().iter().map(Tab::title).collect();
        assert_eq!(titles, ["a", "b"], "the text file is not opened");
        assert!(is_markdown_path(&temp.path().join("a.md")));
        assert!(is_markdown_path(&temp.path().join("b.MARKDOWN")), "the browser's rule ignores case");
        assert!(!is_markdown_path(&temp.path().join("c.txt")));
        assert!(!is_markdown_path(&temp.path().join("no-extension")), "a stem alone is not Markdown");
    }

    #[test]
    fn a_refused_drop_says_which_file_is_not_markdown() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let paths = [temp.path().join("picture.png")];

        state.dropped_files_rejected(&paths);

        let notice = state.ui.notice.as_ref().expect("a notice explains the refusal");
        assert!(notice.text.contains("picture.png"), "the notice names the file: {}", notice.text);
        assert!(notice.text.to_lowercase().contains("markdown"), "and says what it would take");
    }

    #[test]
    fn closing_the_middle_tab_brings_the_right_neighbour_forward() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.open_synthetic("One", "one");
        let middle = state.open_synthetic("Two", "two");
        let last = state.open_synthetic("Three", "three");
        assert!(state.activate_index(1));
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(middle));

        assert_eq!(state.close_tab(middle), CloseOutcome::Closed);
        assert_eq!(state.tabs().len(), 2);
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(last), "the right neighbour comes forward");
        assert_eq!(state.tabs()[0].id, first);
    }

    #[test]
    fn closing_the_last_document_leaves_a_start_page() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Only", "text");
        assert_eq!(state.close_tab(tab), CloseOutcome::Closed);
        assert_eq!(state.tabs().len(), 1);
        assert!(state.active_tab().is_some_and(Tab::is_start));
        assert!(state.shared.docs().is_empty(), "the document was collected");
    }

    #[test]
    fn closing_a_lone_start_page_asks_the_window_to_close() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.active_tab().map(|tab| tab.id).expect("start tab");
        state.request_close_tab(tab);
        assert!(state.ui.close_requested);
        assert_eq!(state.tabs().len(), 1, "the start page stays until the close happens");
    }

    #[test]
    fn closing_a_loading_tab_invalidates_its_generation() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Note", "text");
        let doc = state.tab(tab).and_then(Tab::document).map(|doc| doc.id).expect("doc");
        let generation = state.tracker_begin(doc);
        assert_eq!(state.close_tab(tab), CloseOutcome::Closed);
        assert!(!state.tracker_finish(doc, generation));
    }

    #[test]
    fn a_stale_load_message_does_not_touch_the_document() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Note", "original text");
        let doc = state.tab(tab).and_then(Tab::document).map(|doc| doc.id).expect("doc");
        let stale = state.tracker_begin(doc);
        let _current = state.tracker_begin(doc);

        state.worker().post(JobMsg::Loaded {
            job: 9,
            doc,
            generation: stale,
            result: Ok(lmdv_io::LoadedFile {
                path: PathBuf::from("/tmp/stale.md"),
                buffer: SourceBuffer::new("stale text"),
                meta: lmdv_core::FileMeta::default(),
            }),
        });
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);

        let doc = state.tab(tab).and_then(Tab::document).expect("the tab survived");
        assert_eq!(doc.text().text(), "original text", "the stale completion was dropped");
    }

    #[test]
    fn switching_tabs_keeps_each_scroll_anchor() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.open_synthetic("One", "# One\n\nBody text.\n");
        state.reading.scroll.set(ScrollSnapshot { offset: 320.0, max: 1200.0, viewport: 700.0 });
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);
        state.poll(0.31, &mut tx);
        assert_eq!(state.tab(first).map(|tab| tab.view.anchor.offset), Some(320.0));

        let second = state.open_synthetic("Two", "# Two\n\nOther text.\n");
        assert_eq!(state.tab(first).map(|tab| tab.view.anchor.offset), Some(320.0), "the old tab kept its place");
        assert_eq!(state.tab(second).map(|tab| tab.view.anchor.offset), Some(0.0));

        state.reading.scroll.set(ScrollSnapshot { offset: 48.0, ..ScrollSnapshot::default() });
        state.poll(1.0, &mut tx);
        state.poll(1.31, &mut tx);
        assert_eq!(state.tab(second).map(|tab| tab.view.anchor.offset), Some(48.0));

        assert!(state.activate_tab(first));
        assert_eq!(state.tab(first).map(|tab| tab.view.anchor.offset), Some(320.0));
        state.poll(2.0, &mut tx);
        assert!(state.reading.pending.is_none(), "the restore was issued");
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(first));
    }

    #[test]
    fn reordering_keeps_the_active_document_in_front() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.open_synthetic("One", "one");
        let second = state.open_synthetic("Two", "two");
        let third = state.open_synthetic("Three", "three");
        assert!(state.activate_tab(second));

        state.reorder_tab(2, 0);
        assert_eq!(state.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>(), vec![third, first, second]);
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(second));
    }

    #[test]
    fn tab_keys_round_trip() {
        for id in [0, 1, 42, TabId::MAX] {
            assert_eq!(parse_tab_key(&tab_key(id)), Some(id));
        }
        assert_eq!(parse_tab_key("tab-"), None);
        assert_eq!(parse_tab_key("tab-x"), None);
        assert_eq!(parse_tab_key("7"), None);
    }

    #[test]
    fn strip_titles_drop_the_extension() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let doc = ready_doc(&shared, "/tmp/lmdv-app-test/notes.md", "text");
        let id = state.attach_document(doc);
        assert_eq!(state.tab(id).map(Tab::title).as_deref(), Some("notes"));
        assert_eq!(state.tab(id).map(Tab::full_title).as_deref(), Some("notes.md"));
        assert_eq!(state.tab(id).map(Tab::tooltip).as_deref(), Some("/tmp/lmdv-app-test/notes.md"));
    }

    #[test]
    fn a_notice_expires_on_the_virtual_clock() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.notice("hello");
        let mut tx = Tx::default();
        assert!(state.poll(0.0, &mut tx));
        assert!(state.ui.notice.is_some());
        state.poll(NOTICE_SECONDS + 0.1, &mut tx);
        assert!(state.ui.notice.is_none());
    }

    #[test]
    fn a_sticky_notice_lasts_until_the_document_changes_or_is_saved() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        ready_doc(&shared, "/tmp/lmdv-app-test/one.md", "one");
        let mut tx = Tx::default();

        // The start page lets the poll run without a load, watch or draft job:
        // this test is the notice's own clock, not a worker race.
        state.sticky_notice("Not found: gone.md", NoticeKind::Error);
        assert!(state.ui.notice.as_ref().is_some_and(Notice::is_sticky));
        state.poll(30.0, &mut tx);
        assert!(state.ui.notice.is_some(), "R-29: a sticky notice does not expire on its own");
        assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.deadline.is_infinite()));

        // An ordinary notice still lives exactly its four seconds.
        state.notice("hello");
        assert!(state.ui.notice.as_ref().is_some_and(|notice| !notice.is_sticky()));
        state.poll(30.0 + NOTICE_SECONDS - 0.1, &mut tx);
        assert!(state.ui.notice.is_some(), "the ordinary notice is still up before its deadline");
        state.poll(30.0 + NOTICE_SECONDS + 0.1, &mut tx);
        assert!(state.ui.notice.is_none(), "the ordinary notice expired on its deadline");

        state.sticky_notice("Not found: gone.md", NoticeKind::Error);
        state.open_path(PathBuf::from("/tmp/lmdv-app-test/one.md"));
        assert!(state.ui.notice.is_none(), "R-29: another document removes the previous notice");

        state.sticky_notice("Preview error", NoticeKind::Error);
        state.editor.saved_flash_until = Some(9.0);
        assert!(state.track_saved_flash(None, &[]));
        assert!(state.ui.notice.is_none(), "R-29: a save force-hides a sticky notice");
    }

    #[test]
    fn ui_open_messages_are_applied_by_poll() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.ui_inbox().post(UiMsg::Open(PathBuf::from("/tmp/lmdv-app-test/ui.md")));
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);
        assert_eq!(state.active_tab().map(Tab::title).as_deref(), Some("ui"));
    }

    #[test]
    fn a_forwarded_launch_opens_a_tab_with_its_line() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("launch.md");
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        let mut state = window(&temp);
        let launch = Launch { path: Some(path.clone()), line: Some(2), ..Launch::default() };
        state.shared().launch_inbox().push(lmdv_io::instance::LaunchRequest::new(launch));

        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);
        let tab = state.active_tab().expect("the launch opened a tab");
        assert_eq!(tab.full_title(), "launch.md");
        assert_eq!(tab.view.line, Some(2), "the forwarded --line survives the handoff");
    }

    #[test]
    fn a_close_request_is_issued_from_poll() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.request_close_window();
        let mut tx = Tx::default();
        assert!(state.poll(0.0, &mut tx));
        assert!(!state.ui.close_requested);
    }

    #[test]
    fn a_dirty_document_is_reported_for_the_close_veto() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let doc = ready_doc(&shared, "/tmp/lmdv-app-test/dirty.md", "text");
        state.attach_document(doc.clone());
        assert!(!state.has_unsaved_documents());
        doc.apply(0..4, "changed");
        assert!(state.has_unsaved_documents());
    }

    #[test]
    fn closing_the_last_window_flushes_queued_state_and_marks_the_exit_clean() {
        let temp = TempDir::new().unwrap();
        let dirs = test_support::dirs(&temp);
        let shared = crate::shared::AppShared::start(dirs.clone());
        let state = WindowState::new(shared.clone());
        let path = PathBuf::from("/tmp/lmdv-last-window.md");
        shared.record_recent(path.clone(), 7);
        assert!(!shared.is_quitting());

        drop(state);
        assert!(shared.windows().is_empty());
        assert!(shared.is_quitting(), "the last close is a clean exit");
        let recents = lmdv_io::store::load_recents(&dirs);
        assert_eq!(recents.first().map(|recent| recent.path.clone()), Some(path));
    }

    #[test]
    fn two_windows_share_one_document_one_lease_and_one_registry() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut first = WindowState::new(shared.clone());
        let mut second = WindowState::new(shared.clone());

        let first_tab = first.open_synthetic("Shared", "text");
        let doc = first.tab(first_tab).and_then(Tab::document).cloned().expect("document");
        let second_tab = second.attach_document(doc.clone());
        assert!(
            second.tab(second_tab).and_then(Tab::document).is_some_and(|view| Rc::ptr_eq(view, &doc)),
            "both windows draw the same canonical document"
        );
        assert_eq!(doc.subscribers.borrow().len(), 2);

        let lease = doc.acquire_edit(first.id(), first_tab).expect("the first window edits");
        let refused = doc.acquire_edit(second.id(), second_tab).expect_err("the second window is a viewer");
        assert_eq!(refused.holder(), lease);

        shared.note_window_focused(second.id());
        assert_eq!(shared.windows().focus_order().first(), Some(&second.id()));

        let mut tx = Tx::default();
        first.poll(0.0, &mut tx);
        second.poll(0.0, &mut tx);
        let registry = shared.windows();
        let entry = registry.get(first.id()).expect("first registered");
        assert_eq!(entry.title, "\u{201c}Shared\u{201d} \u{2014} lmdv");
        assert_eq!(entry.active_document, Some(doc.id));
        assert_eq!(entry.documents, BTreeSet::from([doc.id]));
        assert_eq!(registry.window_showing(doc.id), Some(second.id()), "focus order picks the window");
        drop(registry);

        assert_eq!(second.close_tab(second_tab), CloseOutcome::Closed);
        assert!(!doc.subscribers.borrow().contains(&second.id()), "the closed view unsubscribed");
        assert!(doc.subscribers.borrow().contains(&first.id()));

        drop(second);
        assert_eq!(shared.windows().len(), 1);
        drop(first);
        assert_eq!(doc.lease_holder(), None, "closing the editing window releases the lease");
        assert!(shared.windows().is_empty());
        drop(doc);
        shared.docs_mut().collect_unused();
        assert!(shared.docs().is_empty(), "the last view collected the document");
    }

    /// Polls until `done`, like the editor tests: real loads run on workers.
    fn poll_until(state: &mut WindowState, what: &str, mut done: impl FnMut(&WindowState) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut tx = Tx::default();
        while !done(state) {
            assert!(std::time::Instant::now() < deadline, "{what} never happened");
            state.poll(0.0, &mut tx);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn loaded_active(state: &WindowState) -> Option<Rc<Document>> {
        let doc = state.active_tab().and_then(Tab::document).cloned()?;
        let ready = matches!(*doc.status.borrow(), DocStatus::Ready);
        ready.then_some(doc)
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_open_merges_into_the_one_document_and_keeps_its_edits() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real.md");
        std::fs::write(&real, "# Real\n").unwrap();
        let alias = temp.path().join("alias.md");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let mut state = window(&temp);

        state.open_path(real.clone());
        poll_until(&mut state, "the real file", |state| loaded_active(state).is_some());
        let survivor = loaded_active(&state).expect("the real document");
        // Unsaved text is never replaced by the other spelling's disk read.
        survivor.apply(0..1, "X");
        assert!(survivor.is_dirty());

        state.open_path(alias.clone());
        poll_until(&mut state, "the alias merge", |state| {
            state.tabs().len() == 1 && state.active_document_id() == Some(survivor.id)
        });

        assert_eq!(state.shared().docs().len(), 1, "one document for the file");
        let shown = state.active_tab().and_then(Tab::document).cloned().expect("the survivor tab");
        assert!(Rc::ptr_eq(&shown, &survivor));
        assert!(shown.text().text().starts_with('X'), "the edited buffer survived the merge");
        assert!(shown.is_dirty());
        let canonical = std::fs::canonicalize(&alias).unwrap();
        assert_eq!(state.shared().docs().find_path(&canonical).map(|doc| doc.id), Some(survivor.id));
        assert!(state.shared().docs().find_path(&alias).is_none(), "the alias spelling is not a document");
        assert_eq!(state.shared().watch_refcount(&canonical), 1, "the merged view keeps one watch");
    }

    /// Replaces `path` with new content through an atomic rename, the way a
    /// save in another editor does: a new inode and a fresh modification time.
    fn replace_file(path: &Path, text: &str) {
        let staged = path.with_extension("staged");
        std::fs::write(&staged, text).unwrap();
        std::fs::rename(&staged, path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_in_place_replacement_reaches_the_ready_document_through_the_link() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real.md");
        std::fs::write(&real, "old\n").unwrap();
        let alias = temp.path().join("alias.md");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let mut state = window(&temp);

        state.open_path(real.clone());
        poll_until(&mut state, "the real file", |state| loaded_active(state).is_some());
        let survivor = loaded_active(&state).expect("the real document");
        replace_file(&real, "new text\n");

        // OI-172: the alias load reads the new file; the ready document held
        // the older buffer, but the completion is newer and the buffer clean.
        state.open_path(alias);
        poll_until(&mut state, "the replacement merge", |state| {
            state.tabs().len() == 1 && state.active_document_id() == Some(survivor.id)
        });
        assert_eq!(survivor.text().text(), "new text\n", "the newer read replaced the stale buffer");
        assert!(!survivor.is_dirty(), "the adopted text is the saved base");
        assert_eq!(survivor.saved.borrow().meta.len, "new text\n".len() as u64);
        assert_eq!(state.shared().docs().len(), 1);
        assert!(
            state.active_tab().and_then(|tab| tab.view.reader_revision).is_none(),
            "the stale model is dropped so the new text is parsed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_dirty_buffer_keeps_its_edits_and_asks_about_the_newer_disk_copy() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real.md");
        std::fs::write(&real, "old\n").unwrap();
        let alias = temp.path().join("alias.md");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let mut state = window(&temp);

        state.open_path(real.clone());
        poll_until(&mut state, "the real file", |state| loaded_active(state).is_some());
        let survivor = loaded_active(&state).expect("the real document");
        survivor.apply(0..1, "X");
        assert!(survivor.is_dirty());
        replace_file(&real, "new text\n");

        state.open_path(alias);
        poll_until(&mut state, "the dirty merge's disk question", |state| {
            matches!(state.prompt(), Some(crate::editor::Prompt::DiskChanged { .. }))
        });
        assert_eq!(survivor.text().text(), "Xld\n", "the unsaved edit survived the merge");
        assert!(survivor.is_dirty());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new text\n", "nothing was written");
        assert!(state.active_tab().is_some_and(|tab| tab.view.disk_changed), "the tab says the disk moved under it");
    }

    #[cfg(unix)]
    #[test]
    fn two_loads_in_flight_still_end_with_one_document() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real.md");
        std::fs::write(&real, "# Real\n").unwrap();
        let alias = temp.path().join("alias.md");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let shared = test_support::shared(&temp);
        let mut first = WindowState::new(shared.clone());
        let mut second = WindowState::new(shared.clone());

        // Neither load has completed when the second open starts: whichever
        // completion lands second must still leave one document.
        first.open_path(real);
        second.open_path(alias);
        poll_until(&mut first, "the first load", |state| loaded_active(state).is_some());
        let first_doc = loaded_active(&first).expect("the first document");
        poll_until(&mut second, "the second load", |state| {
            state.active_tab().and_then(Tab::document).is_some_and(|doc| Rc::ptr_eq(doc, &first_doc))
        });

        assert_eq!(shared.docs().len(), 1, "both spellings are one document");
        let second_doc = second.active_tab().and_then(Tab::document).cloned().expect("the second view");
        assert!(Rc::ptr_eq(&first_doc, &second_doc));
    }

    #[cfg(unix)]
    #[test]
    fn a_third_window_showing_the_same_spelling_joins_the_survivor() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real.md");
        std::fs::write(&real, "# Real\n").unwrap();
        let alias = temp.path().join("alias.md");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let shared = test_support::shared(&temp);
        let mut first = WindowState::new(shared.clone());
        let mut second = WindowState::new(shared.clone());
        let mut third = WindowState::new(shared.clone());

        first.open_path(real.clone());
        poll_until(&mut first, "the first window's load", |state| loaded_active(state).is_some());
        let survivor = loaded_active(&first).expect("the real document");

        // Two windows load the alias spelling: one document, two loads in
        // flight. The first completion merges it away; the other window learns
        // from the merge message instead of keeping a second document.
        second.open_path(alias.clone());
        third.open_path(alias.clone());
        poll_until(&mut second, "the second window's merge", |state| state.active_document_id() == Some(survivor.id));
        poll_until(&mut third, "the third window's merge", |state| state.active_document_id() == Some(survivor.id));
        assert_eq!(shared.docs().len(), 1);
        assert!(Rc::ptr_eq(third.active_tab().and_then(Tab::document).unwrap(), &survivor));
        assert_eq!(survivor.subscribers.borrow().len(), 3, "all three windows show the survivor");
    }

    #[test]
    fn a_large_document_warns_once_and_keeps_editing_explicit() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let path = PathBuf::from("/tmp/lmdv-app-test/large.md");
        let doc = shared.docs_mut().insert_loading(DocumentKey::File(path.clone()), "large.md".into());
        let tab = state.attach_document(doc.clone());
        let generation = state.tracker_begin(doc.id);
        // A restored edit mode waits for this first poll; the size drops it.
        state.pending_editor_tabs.insert(tab);
        state.worker().post(JobMsg::Loaded {
            job: 1,
            doc: doc.id,
            generation,
            result: Ok(lmdv_io::LoadedFile {
                path: path.clone(),
                buffer: SourceBuffer::new("# Large\n"),
                meta: lmdv_core::FileMeta { len: lmdv_io::LARGE_DOC_BYTES + 1, ..lmdv_core::FileMeta::default() },
            }),
        });
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);

        assert!(!state.pending_editor_tabs.contains(&tab), "a large document does not restore edit mode");
        assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Viewer));
        let notice = state.ui.notice.as_ref().map(|notice| notice.text.clone()).unwrap_or_default();
        assert!(notice.contains("MiB"), "the warning names the size: {notice}");
        assert!(notice.contains("large.md"), "the warning names the file: {notice}");

        state.poll(NOTICE_SECONDS + 1.0, &mut tx);
        assert!(state.ui.notice.is_none(), "the size is warned about once");
    }

    #[test]
    fn a_watcher_setup_error_is_a_single_notice() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        shared.record_watch_error("inotify watches exhausted");

        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);
        let notice = state.ui.notice.as_ref().map(|notice| notice.text.clone()).unwrap_or_default();
        assert!(notice.contains("inotify watches exhausted"), "{notice}");

        state.dismiss_notice();
        state.poll(0.1, &mut tx);
        assert!(state.ui.notice.is_none(), "OI-073: the watcher failure is not repeated");
    }

    // ---- T-26: per-tab state ---------------------------------------------

    #[test]
    fn a_sticky_notice_stays_with_its_tab_and_comes_back_with_it() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.open_synthetic("First", "# First\n");
        state.sticky_notice("Not found: deleted.md", NoticeKind::Error);
        assert!(state.tab(first).expect("the first tab").view.notice.is_some(), "the tab keeps its own copy");

        let second = state.open_synthetic("Second", "# Second\n");
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(second));
        assert!(state.ui.notice.is_none(), "T-26: the second tab does not inherit the first one's notice");
        assert!(state.tab(second).expect("the second tab").view.notice.is_none());
        assert!(state.tab(first).expect("the first tab").view.notice.is_some(), "the notice waits on its own tab");

        assert!(state.activate_tab(first));
        let notice = state.ui.notice.as_ref().expect("T-26: the first tab's notice is restored");
        assert!(notice.text.contains("deleted.md"), "{}", notice.text);
        assert_eq!(notice.kind, NoticeKind::Error);
        assert!(notice.is_sticky(), "it is the same sticky notice, not a new one");

        // Ctrl+T puts a fresh start page in front: it wears no notice, and the
        // document keeps its own for the way back.
        state.new_start_tab();
        assert!(state.ui.notice.is_none(), "a new start page does not inherit the document's notice");
        assert!(state.tab(first).expect("the first tab").view.notice.is_some());
        assert!(state.activate_tab(first));
        assert!(state.ui.notice.is_some(), "the document's notice returns with its tab");

        // Dismissing it ends the tab's copy too: a later switch does not undo it.
        state.dismiss_notice();
        assert!(state.tab(first).expect("the first tab").view.notice.is_none());
        state.activate_tab(second);
        state.activate_tab(first);
        assert!(state.ui.notice.is_none(), "a dismissed notice never comes back");
    }

    #[test]
    fn an_ordinary_notice_does_not_follow_its_tab_back() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.open_synthetic("First", "# First\n");
        state.notice("Path copied to the clipboard.");
        assert_eq!(state.ui.notice.as_ref().map(|notice| notice.kind), Some(NoticeKind::Success));

        state.open_synthetic("Second", "# Second\n");
        assert!(state.ui.notice.is_none(), "another tab does not show the first one's confirmation");
        state.activate_tab(first);
        assert!(state.ui.notice.is_none(), "T-26 keeps sticky state only: a confirmation is spent");
        assert!(state.tab(first).expect("the first tab").view.notice.is_none());
    }

    #[test]
    fn a_save_clears_the_tabs_sticky_notice_for_good() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Notes", "# Notes\n");
        state.sticky_notice("Not found: deleted.md", NoticeKind::Error);

        // A save completion arms the flash in a later poll; the sticky goes and
        // must not return when the tab is switched away and back.
        let before = state.editor.saved_flash_until;
        state.editor.saved_flash_until = Some(10.0);
        state.track_saved_flash(before, &[]);
        assert!(state.ui.notice.is_none(), "R-29: a save clears the sticky notice");
        assert!(state.tab(tab).expect("the tab").view.notice.is_none(), "and its per-tab copy");

        let other = state.open_synthetic("Other", "# Other\n");
        state.activate_tab(tab);
        assert!(state.ui.notice.is_none(), "the saved document's tab no longer carries it");
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(tab));
        assert!(state.tab(other).is_some_and(|tab| tab.view.notice.is_none()));
    }

    #[test]
    fn a_failed_background_load_keeps_its_notice_on_that_documents_tab() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let prior = state.open_synthetic("Prior", "# Prior\n");

        // A background open of a missing file: its load fails while the first
        // tab stays in front, so the notice must not be spoken over it.
        let missing = temp.path().join("deleted.md");
        let failed = state.open_path_background(missing);
        poll_until(&mut state, "the background failure", |state| {
            state
                .tab(failed)
                .and_then(Tab::document)
                .is_some_and(|doc| matches!(*doc.status.borrow(), DocStatus::Error(_)))
        });

        assert_eq!(state.active_tab().map(|tab| tab.id), Some(prior), "the first tab stayed in front");
        assert!(state.ui.notice.is_none(), "T-26: the failed background tab's notice is not the front tab's");
        let stored = state.tab(failed).and_then(|tab| tab.view.notice.as_ref()).expect("the notice waits on its tab");
        assert!(stored.text.contains("deleted.md"), "{}", stored.text);
        assert!(stored.is_sticky());

        assert!(state.activate_tab(failed));
        assert!(
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("deleted.md")),
            "bringing the failed tab forward shows its error notice with its card"
        );
    }

    #[test]
    fn each_tab_keeps_the_scheme_its_last_frame_was_rendered_with() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let first = state.open_synthetic("First", "# First\n");

        state.current_appearance(false);
        let light = AppearanceKey::for_prefs(&shared.prefs(), false);
        assert_eq!(state.tab(first).expect("the first tab").view.scheme.get(), Some(light));

        let second = state.open_synthetic("Second", "# Second\n");
        assert_eq!(
            state.tab(second).expect("the second tab").view.scheme.get(),
            None,
            "a tab that has not drawn yet has no scheme of its own"
        );
        state.current_appearance(true);
        let dark = AppearanceKey::for_prefs(&shared.prefs(), true);
        assert_eq!(state.tab(second).expect("the second tab").view.scheme.get(), Some(dark));
        assert_eq!(
            state.tab(first).expect("the first tab").view.scheme.get(),
            Some(light),
            "T-26: a hidden tab keeps the scheme it last drew with"
        );

        // Switching back renders the scheme it had and records it again.
        state.activate_tab(first);
        state.current_appearance(false);
        assert_eq!(state.tab(first).expect("the first tab").view.scheme.get(), Some(light));

        // A theme change is live: the next render records the new scheme on the
        // tab that is shown, even though the hidden one still has the old key.
        state.activate_tab(second);
        let mut prefs = shared.prefs().clone();
        prefs.theme = lmdv_core::ThemePref::Dark;
        assert!(shared.set_prefs(prefs), "the theme changed");
        state.current_appearance(false);
        let dark_theme = AppearanceKey::for_prefs(&shared.prefs(), false);
        assert_eq!(state.tab(second).expect("the second tab").view.scheme.get(), Some(dark_theme));
        assert_eq!(
            state.tab(first).expect("the first tab").view.scheme.get(),
            Some(light),
            "the hidden tab keeps its old record until it is drawn again"
        );
        state.activate_tab(first);
        state.current_appearance(false);
        assert_eq!(
            state.tab(first).expect("the first tab").view.scheme.get(),
            Some(dark_theme),
            "J-01: showing the tab draws the live theme and records it"
        );
    }

    // ---- H-02: dead recents at display time ------------------------------

    #[test]
    fn dead_recents_disappear_when_the_start_page_is_drawn() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let alive = temp.path().join("alive.md");
        std::fs::write(&alive, "# Alive\n").unwrap();
        let dead = temp.path().join("deleted.md");
        shared.preview_recents([(alive.clone(), 1_000), (dead.clone(), 2_000)]);
        let mut state = WindowState::new(shared.clone());
        assert!(state.active_tab().is_some_and(Tab::is_start));

        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);

        let recents = shared.recents();
        assert_eq!(recents.len(), 1, "H-02: the vanished file is removed when the start page is drawn");
        assert_eq!(recents[0].path, alive, "the living recent stays");
        drop(recents);

        let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
        runtime.frame(0.0);
        let elements = runtime.elements();
        assert!(elements.iter().any(|element| element.key.as_deref() == Some("recent-0")), "the living row draws");
        assert!(
            !elements.iter().any(|element| element.text.as_deref().is_some_and(|text| text.contains("deleted.md"))),
            "the dead row is not drawn"
        );
    }

    // ---- H-08: the tab's save confirmation and its announcement -----------

    #[test]
    fn a_save_marks_the_tab_and_announces_it_as_a_polite_status() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("saved.md");
        std::fs::write(&path, "# Title\n\nBody\n").unwrap();
        let mut state = window(&temp);
        state.open_path(path.clone());
        poll_until(&mut state, "the document and its parse", |state| {
            loaded_active(state).is_some() && state.active_tab().is_some_and(|tab| tab.view.reader.is_some())
        });
        let tab = state.active_tab().map(|tab| tab.id).expect("a document tab");

        let mut tx = Tx::default();
        assert!(state.enter_editor(None, &mut tx), "edit mode");
        state.active_editor_edit().set_caret(0);
        state.active_editor_edit().insert("New ");
        state.editor_changed();
        state.request_save_active(&mut tx);
        poll_until(&mut state, "the save", |state| state.saved_flash() && state.ui.notice.is_some());

        // H-08/T-03: the check is the tab's, and the notice bar is the
        // announcement: a polite status carrying the save's own words.
        assert!(state.tab(tab).is_some_and(|tab| state.tab_shows_saved_check(tab)), "the tab shows the saved check");
        let notice = state.ui.notice.clone().expect("the saved announcement");
        assert_eq!(notice.text, lmdv_i18n::t!("msg.saved"));
        assert_eq!(notice.kind, NoticeKind::Success, "a save is a success, not an error alert");

        let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
        runtime.frame(0.0);
        let elements = runtime.elements();
        assert!(
            elements.iter().any(|element| element.key.as_deref() == Some(&format!("tab-saved-{}", tab_key(tab)))),
            "H-08: the check draws in the tab's leading slot, not only in the toolbar"
        );
        let tree = runtime.accessibility_json();
        let status = a11y_node(&tree, "notice-text").expect("the notice's accessible text");
        assert_eq!(status.get("role").and_then(|value| value.as_str()), Some("status"));
        assert!(
            a11y_states(status).contains(&"live-polite"),
            "H-08: “Saved” is announced by the window's polite live region: {:?}",
            a11y_states(status)
        );
        let saved = lmdv_i18n::t!("msg.saved");
        assert_eq!(status.get("name").and_then(|value| value.as_str()), Some(saved));
    }

    // ---- H-09: the error card, its reading time and the prior document ----

    #[test]
    fn an_unreadable_document_shows_the_error_card_and_no_reading_time() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let missing = temp.path().join("unreadable.md");
        let tab = state.open_path(missing.clone());
        poll_until(&mut state, "the failed load", |state| {
            state
                .tab(tab)
                .and_then(Tab::document)
                .is_some_and(|doc| matches!(*doc.status.borrow(), DocStatus::Error(_)))
        });

        let shown = state.tab(tab).expect("the error tab");
        assert!(shown.view.reading.is_none(), "H-09: an unreadable document has no reading time");
        let title = shown.title();

        let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
        runtime.frame(0.0);
        let elements = runtime.elements();
        let card = by_key(&elements, "document-error");
        assert_eq!(card.role, "group", "H-09: the article shows a centered error card");
        let [cx0, cy0, cw, ch] = card.rect;
        let in_card = |element: &&rune::ElementInfo| {
            let [x, y, w, h] = element.rect;
            x >= cx0 - 0.5 && y >= cy0 - 0.5 && x + w <= cx0 + cw + 0.5 && y + h <= cy0 + ch + 0.5
        };
        let text: Vec<&str> = elements.iter().filter(in_card).filter_map(|element| element.text.as_deref()).collect();
        assert!(text.iter().any(|line| line.contains("unreadable.md")), "the card names the document: {text:?}");
        assert!(
            text.iter().any(|line| *line == lmdv_i18n::t!("msg.openFailedCard")),
            "the card says the document could not be opened: {text:?}"
        );
        assert!(by_key(&elements, "retry-open").label.is_some(), "the card offers Try again");
        assert!(by_key(&elements, "close-error-tab").label.is_some(), "and Close tab");
        assert!(
            !elements.iter().any(|element| element.key.as_deref() == Some("start-page-scroll")),
            "H-09: a failed open never falls back to the home page"
        );
        let title_path = format!("t-{}/pill/title/label", tab_key(tab));
        let tab_title = elements
            .iter()
            .find(|element| element.path.ends_with(&title_path))
            .unwrap_or_else(|| panic!("no title element at {title_path}"));
        assert_eq!(tab_title.text.as_deref(), Some(title.as_str()), "the tab shows its name only, no time");
    }

    #[test]
    fn a_failed_back_move_keeps_the_previous_document_and_says_not_found() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let path = temp.path().join("prior.md");
        std::fs::write(&path, "# Prior\n\nBody\n").unwrap();
        let mut state = WindowState::new(shared);
        state.open_path(path.clone());
        poll_until(&mut state, "the prior document", |state| loaded_active(state).is_some());
        let prior = state.active_tab().map(|tab| tab.id).expect("the prior tab");

        let missing = temp.path().join("deleted.md");
        if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == prior) {
            tab.view.nav.record(Place::new(missing.to_string_lossy(), ScrollAnchor::top()));
        }
        let mut tx = Tx::default();
        state.nav_back(&mut tx);
        poll_until(&mut state, "the not-found notice", |state| {
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("Not found"))
        });

        // H-09: with a prior document, the dead target is a notice over that
        // document, never home and never a leftover error tab.
        assert_eq!(state.active_tab().map(|tab| tab.id), Some(prior), "the previous document stays in front");
        assert_eq!(state.tabs().len(), 1, "the failed move's error tab is closed again");
        let notice = state.ui.notice.clone().expect("the not-found notice");
        assert!(notice.text.contains("deleted.md"), "{}", notice.text);
        assert!(!notice.is_sticky(), "a failed move is a transient notice, not the tab's error state");

        let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
        runtime.frame(0.0);
        let elements = runtime.elements();
        assert!(
            !elements.iter().any(|element| element.key.as_deref() == Some("document-error")),
            "the prior document's article is still the subject"
        );
        assert!(
            !elements.iter().any(|element| element.key.as_deref() == Some("start-page-scroll")),
            "H-09: no home page replaces the prior document"
        );
        assert!(
            elements.iter().any(|element| element.key.as_deref() == Some("notice-text")
                && element.text.as_deref().is_some_and(|text| text.contains("deleted.md"))),
            "the notice says what was not found"
        );
    }

    // ---- H-13: one version on the start page and in Help ------------------

    #[test]
    fn the_start_page_and_help_show_the_same_version() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_help(HelpView::Full);

        let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(true, 2.0));
        runtime.frame(0.0);
        let elements = runtime.elements();

        let expected = lmdv_i18n::t!("start.version", "version" => env!("CARGO_PKG_VERSION"));
        assert!(expected.contains(env!("CARGO_PKG_VERSION")), "the label carries the package version");
        let start = by_key(&elements, "start-version");
        let header = by_key(&elements, crate::help::HELP_VERSION);
        let about = by_key(&elements, crate::help::HELP_ABOUT_VERSION);
        assert_eq!(start.text.as_deref(), Some(expected.as_str()), "H-13: the start-page footer");
        assert_eq!(header.text.as_deref(), Some(expected.as_str()), "H-13: the Help header");
        assert_eq!(about.text.as_deref(), Some(expected.as_str()), "H-13: the Help About row");
        assert_eq!(start.text, header.text);
        assert_eq!(header.text, about.text);
    }

    // ---- D-08: every cancel settles the kit's drag to Idle -----------------

    fn drag_in_hand(runtime: &mut Runtime<WindowState>, tab: lmdv_core::TabId, dx: f32) {
        let (x, y) = runtime.find(&format!("t-{}", tab_key(tab))).expect("the drag's tab");
        runtime.pointer_move(x, y);
        runtime.frame(0.0);
        runtime.pointer_down(x, y);
        runtime.frame(0.0);
        runtime.pointer_move(x + dx, y);
        runtime.frame(0.0);
        assert!(
            matches!(runtime.state.drag.phase(), rune_kit::shell::TabDragPhase::Dragging(_)),
            "the reorder is in hand before the cancel: {:?}",
            runtime.state.drag.phase()
        );
    }

    fn three_tabs(state: &mut WindowState) -> Vec<lmdv_core::TabId> {
        vec![
            state.open_synthetic("One", "# One\n"),
            state.open_synthetic("Two", "# Two\n"),
            state.open_synthetic("Three", "# Three\n"),
        ]
    }

    #[test]
    fn escape_settles_an_in_hand_reorder_to_idle() {
        use rune::{Key, KeyEvent, Mods};
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tabs = three_tabs(&mut state);
        let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        drag_in_hand(&mut runtime, tabs[0], 140.0);

        runtime.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        runtime.frame(1.0 / 60.0);
        runtime.pointer_up(0.0, 0.0);
        runtime.frame(2.0 / 60.0);

        assert!(
            matches!(runtime.state.drag.phase(), rune_kit::shell::TabDragPhase::Idle),
            "Escape ends the gesture: {:?}",
            runtime.state.drag.phase()
        );
        let titles: Vec<_> = runtime.state.tabs().iter().map(Tab::full_title).collect();
        assert_eq!(titles, ["One", "Two", "Three"], "the tab returned to its original slot");
    }

    #[test]
    fn pointer_cancel_settles_an_in_hand_reorder_to_idle() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tabs = three_tabs(&mut state);
        let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        drag_in_hand(&mut runtime, tabs[0], 140.0);

        runtime.cancel_pointer();
        runtime.frame(1.0 / 60.0);

        assert!(
            matches!(runtime.state.drag.phase(), rune_kit::shell::TabDragPhase::Idle),
            "a cancelled pointer ends the gesture: {:?}",
            runtime.state.drag.phase()
        );
        let titles: Vec<_> = runtime.state.tabs().iter().map(Tab::full_title).collect();
        assert_eq!(titles, ["One", "Two", "Three"], "no reorder landed");
    }

    #[test]
    fn opening_a_modal_settles_an_in_hand_reorder_to_idle() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tabs = three_tabs(&mut state);
        let mut runtime = Runtime::new(state, view, (1100.0, 760.0), Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        drag_in_hand(&mut runtime, tabs[0], 140.0);

        runtime.update(|state, _| state.open_help(HelpView::Full));
        runtime.frame(1.0 / 60.0);
        runtime.frame(2.0 / 60.0);

        assert!(
            matches!(runtime.state.drag.phase(), rune_kit::shell::TabDragPhase::Idle),
            "a modal taking keys ends the gesture: {:?}",
            runtime.state.drag.phase()
        );
        let titles: Vec<_> = runtime.state.tabs().iter().map(Tab::full_title).collect();
        assert_eq!(titles, ["One", "Two", "Three"], "no reorder landed");
    }

    /// The accessibility node keyed `key`, depth first.
    fn a11y_node<'a>(node: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        if node.get("key").and_then(|value| value.as_str()) == Some(key) {
            return Some(node);
        }
        node.get("children")?.as_array()?.iter().find_map(|child| a11y_node(child, key))
    }

    /// The state names on an accessibility node (`live-polite`, `focused`, …).
    fn a11y_states(node: &serde_json::Value) -> Vec<&str> {
        node.get("states")
            .and_then(|value| value.as_array())
            .map(|states| states.iter().filter_map(|state| state.as_str()).collect())
            .unwrap_or_default()
    }

    fn by_key<'a>(elements: &'a [rune::ElementInfo], key: &str) -> &'a rune::ElementInfo {
        elements
            .iter()
            .find(|element| element.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("no element keyed {key}"))
    }
}
