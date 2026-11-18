//! The editor's UX layer: find/replace bar, format bar, `/` block menu and
//! go-to-line (E-20–E-31).
//!
//! Every text change still goes through the shared document: the widgets edit
//! the same [`EditorSession`] the source field binds, and each command calls
//! [`WindowState::editor_changed`] so the preview, draft timer and dirty flag
//! see it. The pure Markdown operations live in [`lmdv_edit::format`]; this
//! module only maps keys and buttons onto them, keeps the per-tab bar state and
//! computes the format bar's pressed state.
//!
//! The renderer has no paint-only highlight API yet (OI-043), so find shows the
//! current match through the source field's own selection instead of fake
//! highlights.

use std::ops::Range;

use lmdv_edit::{
    BlockEditKind, BlockKind, EditorSession, LinePrefix, delete_line, duplicate_line, goto_line, insert_link,
    move_line_down, move_line_up, select_line, set_heading, toggle_comment, toggle_line_prefix, toggle_wrap,
};
use rune::{Key, KeyEvent, TextEdit, TextStyle, Tx};

use crate::window::{Mode, Tab, WindowState};

/// E-37: the editor's font size is 87.5% of the reading size (14 pt at the
/// 16 pt default, 19.25 pt at 22).
pub const EDITOR_FONT_SCALE: f32 = 0.875;
/// The editor's line spacing, as the original CodeMirror scroller sets it.
pub const EDITOR_LINE_RATIO: f32 = 1.6;

/// The source field's two metrics for one reading size (E-37).
///
/// The field's monospaced family stays the kit's; only the size and the
/// proportional line height follow the reading size. Changing the Aa size
/// rebuilds the view, so the field re-measures at the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorMetrics {
    /// The field's font size in points.
    pub size: f32,
    /// The field's line height in points.
    pub line_height: f32,
}

impl EditorMetrics {
    /// The metrics of a reading size in points (13–22 in the settings).
    pub fn for_reader(size: u16) -> Self {
        let size = f32::from(size.max(1)) * EDITOR_FONT_SCALE;
        Self { size, line_height: size * EDITOR_LINE_RATIO }
    }

    /// The field style: `base`'s family and weight at the editor's metrics.
    pub fn field(self, base: TextStyle) -> TextStyle {
        TextStyle { size: self.size, line_height: self.line_height, ..base }
    }
}

/// The key of the source field's scroll viewport.
pub const SOURCE_SCROLL_KEY: &str = "editor-scroll";
/// The key of the source text field.
pub const SOURCE_FIELD_KEY: &str = "editor-source";
/// The find bar's query field.
pub const FIND_QUERY_KEY: &str = "find-query";
/// The find bar's replace field.
pub const FIND_REPLACE_KEY: &str = "find-replace";
/// The go-to-line field.
pub const GOTO_FIELD_KEY: &str = "goto-line";
/// The toolbar's live-preview toggle, the control Esc then Tab hands the
/// keyboard to when it leaves the editor (E-33).
pub const PREVIEW_TOGGLE_KEY: &str = "toggle-preview";

/// The find bar's two text fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindField {
    /// The query.
    Query,
    /// The replacement.
    Replace,
}

/// Per-tab find bar widgets. The session owns the query/options and hits; these
/// two fields are what `text_input` binds.
#[derive(Clone, Debug, Default)]
pub struct FindBar {
    /// The bar is visible.
    pub open: bool,
    /// The query field.
    pub query: TextEdit,
    /// The replacement field.
    pub replace: TextEdit,
    /// The selection the bar opened on, so typing picks the first match after it.
    pub anchor: usize,
}

/// The `/` block menu's state for one tab.
#[derive(Clone, Debug, Default)]
pub struct SlashMenu {
    /// The menu is showing.
    pub open: bool,
    /// Byte offset just after the line's indentation, where `/` starts.
    pub start: usize,
    /// The text after `/`, used as the filter.
    pub query: String,
    /// The highlighted row.
    pub selected: usize,
}

/// The go-to-line prompt.
#[derive(Clone, Debug, Default)]
pub struct GotoLine {
    /// The line number field.
    pub input: TextEdit,
    /// Why the last attempt was refused.
    pub error: Option<String>,
}

/// One row in the `/` menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashItem {
    /// The command id the row runs.
    pub command: &'static str,
    /// The localized label the row shows.
    pub label: String,
    /// Words the filter may match.
    pub keywords: String,
}

/// A formatting, line or block command. The format bar, the editor keymap and
/// the `/` menu all map onto one of these and then onto a command id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatAction {
    /// Toggle an inline marker.
    Wrap(&'static str),
    /// Set an ATX heading level (0 removes it).
    Heading(u8),
    /// Toggle a list or quote prefix.
    Prefix(LinePrefix),
    /// Wrap the selection in a link.
    Link,
    /// Insert a block template.
    Block(BlockKind),
    /// Toggle the HTML/fence comment.
    Comment,
    /// Move the selected lines up.
    MoveLineUp,
    /// Move the selected lines down.
    MoveLineDown,
    /// Duplicate the selected lines.
    DuplicateLine,
    /// Delete the selected lines.
    DeleteLine,
    /// Select the touched lines.
    SelectLine,
}

impl FormatAction {
    /// The command id shared by keyboard, buttons and the `/` menu.
    pub fn command_id(self) -> &'static str {
        match self {
            Self::Wrap("**") => "edit.bold",
            Self::Wrap("*") => "edit.italic",
            Self::Wrap("~~") => "edit.strike",
            Self::Wrap("`") => "edit.code",
            Self::Wrap(_) => "edit.bold",
            Self::Heading(1) => "edit.heading-1",
            Self::Heading(2) => "edit.heading-2",
            Self::Heading(3) => "edit.heading-3",
            Self::Heading(_) => "edit.paragraph",
            Self::Prefix(LinePrefix::Bullet) => "edit.list-bullet",
            Self::Prefix(LinePrefix::Ordered) => "edit.list-ordered",
            Self::Prefix(LinePrefix::Task) => "edit.list-task",
            Self::Prefix(LinePrefix::Quote) => "edit.quote",
            Self::Link => "edit.link",
            Self::Block(BlockKind::Table) => "edit.insert-table",
            Self::Block(BlockKind::Mermaid) => "edit.insert-mermaid",
            Self::Block(BlockKind::Math) => "edit.insert-math",
            Self::Block(BlockKind::Code) => "edit.insert-code-block",
            Self::Block(BlockKind::Note) => "edit.insert-note",
            Self::Block(BlockKind::Tip) => "edit.insert-tip",
            Self::Block(BlockKind::Warning) => "edit.insert-warning",
            Self::Block(BlockKind::Divider) => "edit.insert-divider",
            Self::Comment => "edit.comment",
            Self::MoveLineUp => "edit.move-line-up",
            Self::MoveLineDown => "edit.move-line-down",
            Self::DuplicateLine => "edit.duplicate-line",
            Self::DeleteLine => "edit.delete-line",
            Self::SelectLine => "edit.select-line",
        }
    }

    /// The localized label the format bar and `/` menu show.
    pub fn label(self) -> &'static str {
        match self {
            Self::Wrap("**") => lmdv_i18n::t!("cmd.bold"),
            Self::Wrap("*") => lmdv_i18n::t!("cmd.italic"),
            Self::Wrap("~~") => lmdv_i18n::t!("cmd.strike"),
            Self::Wrap("`") => lmdv_i18n::t!("cmd.code"),
            Self::Wrap(_) => lmdv_i18n::t!("cmd.bold"),
            Self::Heading(1) => lmdv_i18n::t!("cmd.h1"),
            Self::Heading(2) => lmdv_i18n::t!("cmd.h2"),
            Self::Heading(3) => lmdv_i18n::t!("cmd.h3"),
            Self::Heading(_) => lmdv_i18n::t!("cmd.paragraph"),
            Self::Prefix(LinePrefix::Bullet) => lmdv_i18n::t!("cmd.bullet"),
            Self::Prefix(LinePrefix::Ordered) => lmdv_i18n::t!("cmd.ordered"),
            Self::Prefix(LinePrefix::Task) => lmdv_i18n::t!("cmd.task"),
            Self::Prefix(LinePrefix::Quote) => lmdv_i18n::t!("cmd.quote"),
            Self::Link => lmdv_i18n::t!("cmd.link"),
            Self::Block(BlockKind::Table) => lmdv_i18n::t!("cmd.table"),
            Self::Block(BlockKind::Mermaid) => lmdv_i18n::t!("cmd.diagram"),
            Self::Block(BlockKind::Math) => lmdv_i18n::t!("cmd.math"),
            Self::Block(BlockKind::Code) => lmdv_i18n::t!("cmd.codeblock"),
            Self::Block(BlockKind::Note) => lmdv_i18n::t!("cmd.note"),
            Self::Block(BlockKind::Tip) => lmdv_i18n::t!("cmd.tip"),
            Self::Block(BlockKind::Warning) => lmdv_i18n::t!("cmd.warning"),
            Self::Block(BlockKind::Divider) => lmdv_i18n::t!("cmd.divider"),
            Self::Comment => lmdv_i18n::t!("cmd.comment"),
            Self::MoveLineUp => lmdv_i18n::t!("cmd.moveLineUp"),
            Self::MoveLineDown => lmdv_i18n::t!("cmd.moveLineDown"),
            Self::DuplicateLine => lmdv_i18n::t!("cmd.copyLineDown"),
            Self::DeleteLine => lmdv_i18n::t!("cmd.deleteLine"),
            Self::SelectLine => lmdv_i18n::t!("cmd.selectLine"),
        }
    }

    /// Extra filter words for the `/` menu.
    pub fn keywords(self) -> String {
        match self {
            Self::Heading(1) => format!("h1 title {}", lmdv_i18n::t!("kw.h1")),
            Self::Heading(2) => format!("h2 subtitle {}", lmdv_i18n::t!("kw.h2")),
            Self::Heading(3) => "h3".to_owned(),
            Self::Prefix(LinePrefix::Bullet) => format!("ul {}", lmdv_i18n::t!("kw.bullet")),
            Self::Prefix(LinePrefix::Ordered) => format!("ol {}", lmdv_i18n::t!("kw.ordered")),
            Self::Prefix(LinePrefix::Task) => lmdv_i18n::t!("kw.task").to_owned(),
            Self::Prefix(LinePrefix::Quote) => lmdv_i18n::t!("kw.quote").to_owned(),
            Self::Block(BlockKind::Table) => lmdv_i18n::t!("kw.table").to_owned(),
            Self::Block(BlockKind::Mermaid) => lmdv_i18n::t!("kw.diagram").to_owned(),
            Self::Block(BlockKind::Math) => lmdv_i18n::t!("kw.math").to_owned(),
            Self::Block(BlockKind::Code) => lmdv_i18n::t!("kw.codeblock").to_owned(),
            Self::Block(BlockKind::Note) => lmdv_i18n::t!("kw.note").to_owned(),
            Self::Block(BlockKind::Tip) => lmdv_i18n::t!("kw.tip").to_owned(),
            Self::Block(BlockKind::Warning) => lmdv_i18n::t!("kw.warning").to_owned(),
            Self::Block(BlockKind::Divider) => lmdv_i18n::t!("kw.divider").to_owned(),
            _ => String::new(),
        }
    }
}

/// The format bar's pressed state, read from the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FormatState {
    /// The selection/caret is bold.
    pub bold: bool,
    /// It is italic.
    pub italic: bool,
    /// It is struck through.
    pub strike: bool,
    /// It is inline code.
    pub code: bool,
    /// The ATX heading level of the first selected line (0: none).
    pub heading: u8,
    /// The first selected line is a bullet list.
    pub bullet: bool,
    /// It is an ordered list.
    pub ordered: bool,
    /// It is a task list.
    pub task: bool,
    /// It is a quote.
    pub quote: bool,
}

/// The block inserts, in the `/` menu's order (E-28). Headings and lists are
/// blocks there too, as in the original.
const SLASH_ACTIONS: [FormatAction; 15] = [
    FormatAction::Heading(1),
    FormatAction::Heading(2),
    FormatAction::Heading(3),
    FormatAction::Prefix(LinePrefix::Bullet),
    FormatAction::Prefix(LinePrefix::Ordered),
    FormatAction::Prefix(LinePrefix::Task),
    FormatAction::Prefix(LinePrefix::Quote),
    FormatAction::Block(BlockKind::Table),
    FormatAction::Block(BlockKind::Mermaid),
    FormatAction::Block(BlockKind::Math),
    FormatAction::Block(BlockKind::Code),
    FormatAction::Block(BlockKind::Note),
    FormatAction::Block(BlockKind::Tip),
    FormatAction::Block(BlockKind::Warning),
    FormatAction::Block(BlockKind::Divider),
];

/// The `/` menu rows matching `query` at the start of any label or keyword
/// word. An empty query shows every row.
pub fn slash_items(query: &str) -> Vec<SlashItem> {
    let query = query.trim().to_ascii_lowercase();
    SLASH_ACTIONS
        .iter()
        .filter_map(|&action| {
            let label = action.label().to_owned();
            let keywords = action.keywords();
            if !query.is_empty() {
                let haystack = format!("{label} {keywords}").to_ascii_lowercase();
                let matches_word = haystack.split_whitespace().any(|word| word.starts_with(&query));
                if !matches_word {
                    return None;
                }
            }
            Some(SlashItem { command: action.command_id(), label, keywords })
        })
        .collect()
}

/// The format bar's state for a tab.
pub fn format_state(tab: &Tab) -> FormatState {
    let Some(editor) = tab.view.editor.as_ref() else { return FormatState::default() };
    let (text, selection) = match format_context(editor) {
        Some(context) => context,
        None => return FormatState::default(),
    };
    let (from, to) = (selection.start.min(text.len()), selection.end.min(text.len()));
    let (from, to) = if from <= to { (from, to) } else { (to, from) };
    let line_start = text[..from].rfind('\n').map_or(0, |offset| offset + 1);
    let line_end = text[to..].find('\n').map_or(text.len(), |offset| to + offset);
    let line = &text[line_start..line_end];

    let mut state = FormatState {
        bold: marker_active(text, from, to, "**"),
        italic: marker_active(text, from, to, "*"),
        strike: marker_active(text, from, to, "~~"),
        code: marker_active(text, from, to, "`"),
        heading: heading_level(line),
        ..FormatState::default()
    };
    if let Some((_, kind)) = list_prefix(line) {
        match kind {
            ListKind::Bullet => state.bullet = true,
            ListKind::Ordered => state.ordered = true,
            ListKind::Task => state.task = true,
        }
    }
    state.quote = line.trim_start().starts_with('>');
    state
}

/// The block's text and selection for the format bar: the block field in block
/// mode, the single field otherwise.
fn format_context(editor: &EditorSession) -> Option<(&str, Range<usize>)> {
    if let Some(block) = editor.block_editor() {
        let index = editor.active_block().unwrap_or(0);
        let block = block.block(index)?;
        let (from, to) = block.edit.range();
        return Some((&block.edit.text, from..to));
    }
    let (from, to) = editor.edit.range();
    Some((&editor.edit.text, from..to))
}

/// Runs one format action on a field.
pub fn run_format(action: FormatAction, edit: &mut TextEdit) -> bool {
    match action {
        FormatAction::Wrap(marker) => toggle_wrap(edit, marker),
        FormatAction::Heading(level) => set_heading(edit, level),
        FormatAction::Prefix(kind) => toggle_line_prefix(edit, kind),
        FormatAction::Link => insert_link(edit),
        FormatAction::Block(kind) => kind.insert(edit),
        FormatAction::Comment => toggle_comment(edit),
        FormatAction::MoveLineUp => move_line_up(edit),
        FormatAction::MoveLineDown => move_line_down(edit),
        FormatAction::DuplicateLine => duplicate_line(edit),
        FormatAction::DeleteLine => delete_line(edit),
        FormatAction::SelectLine => select_line(edit),
    }
}

impl WindowState {
    /// The `on_edit` accessor for the find bar's fields.
    pub(crate) fn active_find_edit(&mut self, field: FindField) -> &mut TextEdit {
        let active = self.active;
        if let Some(tab) = self.tabs.get_mut(active) {
            return match field {
                FindField::Query => &mut tab.view.find_bar.query,
                FindField::Replace => &mut tab.view.find_bar.replace,
            };
        }
        &mut self.editor.scratch
    }

    /// The `on_edit` accessor for the go-to-line field.
    pub(crate) fn active_goto_edit(&mut self) -> &mut TextEdit {
        if let Some(goto) = self.editor.goto_line.as_mut() {
            return &mut goto.input;
        }
        &mut self.editor.scratch
    }

    /// Opens (or refocuses) the find bar. `Ctrl+F` shows the find row only;
    /// `Ctrl+H` adds the replace row (E-20).
    pub(crate) fn open_find(&mut self, with_replace: bool, tx: &mut Tx) {
        if self.active_tab().is_none_or(|tab| tab.view.mode != Mode::Editor) {
            return;
        }
        self.hide_narrow_preview();
        let mut seed: Option<String> = None;
        if let Some(tab) = self.active_tab_mut()
            && let Some(editor) = tab.view.editor.as_mut()
        {
            editor.find.replace_open = with_replace;
            if !tab.view.find_bar.open {
                let selection = editor.selection();
                let selected = editor.text().get(selection.clone()).unwrap_or("").to_owned();
                if !selected.is_empty() && !selected.contains('\n') {
                    seed = Some(selected);
                }
                tab.view.find_bar.anchor = selection.start;
                if tab.view.find_bar.query.text != editor.find.query {
                    tab.view.find_bar.query.replace_text(editor.find.query.clone());
                }
                if tab.view.find_bar.replace.text != editor.find.replace {
                    tab.view.find_bar.replace.replace_text(editor.find.replace.clone());
                }
                tab.view.find_bar.open = true;
                tab.view.find_bar.query.set_selection(0, tab.view.find_bar.query.text.len());
            }
            if let Some(seed) = seed {
                tab.view.find_bar.query.replace_text(seed);
                tab.view.find_bar.query.set_selection(0, tab.view.find_bar.query.text.len());
                editor.find.query = tab.view.find_bar.query.text.clone();
            }
        }
        self.refresh_find_from_anchor();
        self.focus_find_field(with_replace, tx);
    }

    /// `Ctrl+F`/`Ctrl+H` focus: the replacement first iff it is visible and the
    /// query already has text, as the original does.
    pub(crate) fn focus_find_field(&mut self, with_replace: bool, tx: &mut Tx) {
        let replace_first = with_replace
            && self.active_tab().is_some_and(|tab| tab.view.find_bar.open && !tab.view.find_bar.query.text.is_empty());
        if replace_first {
            tx.focus(FIND_REPLACE_KEY, false);
        } else {
            tx.focus(FIND_QUERY_KEY, false);
        }
    }

    /// Closes the bar and returns the keyboard to the source field.
    pub(crate) fn close_find(&mut self, tx: &mut Tx) {
        if let Some(tab) = self.active_tab_mut() {
            tab.view.find_bar.open = false;
        }
        self.focus_editor_field(tx);
    }

    /// The find bar's query changed: mirror it into the session and pick the
    /// first match at or after where the bar opened.
    pub(crate) fn find_query_changed(&mut self) {
        let Some(tab) = self.active_tab_mut() else { return };
        let Some(editor) = tab.view.editor.as_mut() else { return };
        editor.find.query = tab.view.find_bar.query.text.clone();
        if editor.find.query.is_empty() {
            editor.find.matches.clear();
            editor.find.current = 0;
            return;
        }
        self.refresh_find_from_anchor();
    }

    /// The replace field changed.
    pub(crate) fn find_replace_changed(&mut self) {
        let Some(tab) = self.active_tab_mut() else { return };
        let Some(editor) = tab.view.editor.as_mut() else { return };
        editor.find.replace = tab.view.find_bar.replace.text.clone();
    }

    /// Recomputes the hits and selects the first one from the bar's anchor.
    pub(crate) fn refresh_find_from_anchor(&mut self) {
        let Some(tab) = self.active_tab_mut() else { return };
        let Some(editor) = tab.view.editor.as_mut() else { return };
        if editor.find.query.is_empty() {
            editor.find.matches.clear();
            editor.find.current = 0;
            return;
        }
        editor.refresh_find();
        if editor.find.matches.is_empty() {
            return;
        }
        let anchor = tab.view.find_bar.anchor;
        let index = editor.find.matches.iter().position(|hit| hit.source.start >= anchor).unwrap_or(0);
        editor.find.current = index;
        let range = editor.find.matches[index].source.clone();
        editor.set_selection(range);
    }

    /// Recomputes hits after a source edit without moving the user's caret.
    pub(crate) fn refresh_find_hits(&mut self) {
        let Some(tab) = self.active_tab_mut() else { return };
        let Some(editor) = tab.view.editor.as_mut() else { return };
        if editor.find.query.is_empty() {
            editor.find.matches.clear();
            editor.find.current = 0;
            return;
        }
        editor.refresh_find();
    }

    /// Sets the query from the app (scene, test, palette).
    pub(crate) fn set_find_query(&mut self, query: &str) {
        if let Some(tab) = self.active_tab_mut() {
            tab.view.find_bar.query.replace_text(query);
            if let Some(editor) = tab.view.editor.as_mut() {
                editor.find.query = query.to_owned();
            }
        }
        self.refresh_find_from_anchor();
    }

    /// Sets the replacement from the app.
    pub(crate) fn set_find_replace(&mut self, replace: &str) {
        if let Some(tab) = self.active_tab_mut() {
            tab.view.find_bar.replace.replace_text(replace);
        }
        self.find_replace_changed();
    }

    /// `Enter`/F3 in the find bar and the source: next match, wrapping.
    pub(crate) fn find_next_cmd(&mut self, tx: &mut Tx) {
        if !self.find_bar_visible() {
            self.show_find_bar(false);
        }
        let stepped = {
            let Some(tab) = self.active_tab_mut() else { return };
            let Some(editor) = tab.view.editor.as_mut() else { return };
            if editor.find.query.is_empty() { false } else { editor.find_next() }
        };
        if stepped {
            self.focus_editor_field(tx);
            self.scroll_to_editor_selection(tx);
        }
    }

    /// `Shift+Enter`/Shift+F3: previous match, wrapping.
    pub(crate) fn find_prev_cmd(&mut self, tx: &mut Tx) {
        if !self.find_bar_visible() {
            self.show_find_bar(false);
        }
        let stepped = {
            let Some(tab) = self.active_tab_mut() else { return };
            let Some(editor) = tab.view.editor.as_mut() else { return };
            if editor.find.query.is_empty() { false } else { editor.find_prev() }
        };
        if stepped {
            self.focus_editor_field(tx);
            self.scroll_to_editor_selection(tx);
        }
    }

    /// Shows the bar, keeping the session's query.
    fn show_find_bar(&mut self, with_replace: bool) {
        self.hide_narrow_preview();
        let Some(tab) = self.active_tab_mut() else { return };
        let Some(editor) = tab.view.editor.as_mut() else { return };
        tab.view.find_bar.open = true;
        tab.view.find_bar.anchor = editor.selection().start;
        if tab.view.find_bar.query.text != editor.find.query {
            tab.view.find_bar.query.replace_text(editor.find.query.clone());
        }
        if tab.view.find_bar.replace.text != editor.find.replace {
            tab.view.find_bar.replace.replace_text(editor.find.replace.clone());
        }
        editor.find.replace_open = with_replace;
    }

    /// The bar is open on the active tab.
    pub fn find_bar_visible(&self) -> bool {
        self.active_tab().is_some_and(|tab| tab.view.find_bar.open)
    }

    /// Replaces the current match and advances; one undo step.
    pub(crate) fn replace_current_cmd(&mut self, tx: &mut Tx) {
        let replaced = {
            let Some(tab) = self.active_tab_mut() else { return };
            let Some(editor) = tab.view.editor.as_mut() else { return };
            editor.replace_current()
        };
        if replaced {
            self.focus_editor_field(tx);
            self.editor_changed();
            self.scroll_to_editor_selection(tx);
        }
    }

    /// Replaces every hit in one undo group.
    pub(crate) fn replace_all_cmd(&mut self) {
        let count = {
            let Some(tab) = self.active_tab_mut() else { return };
            let Some(editor) = tab.view.editor.as_mut() else { return };
            editor.replace_all_matches()
        };
        if count > 0 {
            self.editor_changed();
            let message = lmdv_i18n::t!("cm.replacedMatches").replace('$', &count.to_string());
            self.notice(message);
        } else {
            self.notice(lmdv_i18n::t!("pane.noMatches"));
        }
    }

    /// Toggles one of the find bar's three options and refreshes the hits.
    pub(crate) fn toggle_find_case(&mut self) {
        if let Some(tab) = self.active_tab_mut()
            && let Some(editor) = tab.view.editor.as_mut()
        {
            editor.find.case_sensitive = !editor.find.case_sensitive;
        }
        self.refresh_find_from_anchor();
    }

    pub(crate) fn toggle_find_whole_word(&mut self) {
        if let Some(tab) = self.active_tab_mut()
            && let Some(editor) = tab.view.editor.as_mut()
        {
            editor.find.whole_word = !editor.find.whole_word;
        }
        self.refresh_find_from_anchor();
    }

    pub(crate) fn toggle_find_regex(&mut self) {
        if let Some(tab) = self.active_tab_mut()
            && let Some(editor) = tab.view.editor.as_mut()
        {
            editor.find.regex = !editor.find.regex;
        }
        self.refresh_find_from_anchor();
    }

    /// The regex compile error for the bar, if any.
    pub fn find_error(&self) -> Option<String> {
        self.active_tab().and_then(|tab| tab.view.editor.as_ref()).and_then(EditorSession::find_error)
    }

    /// The current match count and index for the bar.
    pub fn find_count(&self) -> (usize, usize) {
        self.active_tab()
            .and_then(|tab| tab.view.editor.as_ref())
            .map_or((0, 0), |editor| (editor.find.matches.len(), editor.find.current))
    }

    /// Scrolls the source viewport so the selection's line is visible. Rune
    /// fields only follow the caret while focused, so the find bar scrolls the
    /// viewport itself. In block mode it reveals the active block's row, which
    /// may not be mounted yet.
    pub(crate) fn scroll_to_editor_selection(&mut self, tx: &mut Tx) {
        let Some(tab) = self.active_tab() else { return };
        let Some(editor) = tab.view.editor.as_ref() else { return };
        if editor.block_mode_active() {
            if let Some(index) = editor.active_block() {
                tx.scroll_into_view(crate::block_ui::block_field_key(index));
            }
            return;
        }
        let byte = editor.selection().start;
        let text = editor.text();
        let line = text[..byte.min(text.len())].bytes().filter(|&byte| byte == b'\n').count();
        let y = (line as f32 * self.editor_metrics().line_height - 80.0).max(0.0);
        tx.scroll_to_y(SOURCE_SCROLL_KEY, y);
    }

    /// The source field's metrics at the current reading size (E-37).
    pub(crate) fn editor_metrics(&self) -> EditorMetrics {
        EditorMetrics::for_reader(self.shared().prefs().reader.size)
    }

    /// Applies one format action and syncs the document.
    pub(crate) fn apply_format(&mut self, action: FormatAction) -> bool {
        let Some(tab) = self.active_tab_mut() else { return false };
        let Some(editor) = tab.view.editor.as_mut() else { return false };
        let changed = if editor.block_mode_active() {
            // Only the block the user is in: never silently reformat block 0.
            let Some(index) = editor.active_block() else { return false };
            editor.apply_block(index, BlockEditKind::Discrete, |edit| run_format(action, edit)).unwrap_or(false)
        } else {
            run_format(action, &mut editor.edit)
        };
        if changed {
            self.editor_changed();
        }
        changed
    }

    /// Recomputes the `/` menu from the caret and text; called after edits.
    pub(crate) fn refresh_slash_menu(&mut self) {
        let Some(tab) = self.active_tab() else { return };
        let Some(editor) = tab.view.editor.as_ref() else { return };
        let selection = editor.selection();
        let found = if selection.is_empty() { slash_query_at(editor.text(), selection.end) } else { None };
        let Some((start, query)) = found else {
            if let Some(tab) = self.active_tab_mut() {
                tab.view.slash_menu = SlashMenu::default();
            }
            return;
        };
        let items = slash_items(&query);
        let Some(tab) = self.active_tab_mut() else { return };
        if !tab.view.slash_menu.open || tab.view.slash_menu.start != start {
            tab.view.slash_menu = SlashMenu { open: true, start, query, selected: 0 };
        } else {
            tab.view.slash_menu.query = query;
            tab.view.slash_menu.selected = tab.view.slash_menu.selected.min(items.len().saturating_sub(1));
            if items.is_empty() {
                tab.view.slash_menu.open = false;
            }
        }
    }

    /// The `/` menu is open on the active tab.
    pub fn slash_menu(&self) -> Option<&SlashMenu> {
        self.active_tab().map(|tab| &tab.view.slash_menu).filter(|menu| menu.open)
    }

    /// The filtered rows the menu draws.
    pub fn slash_items(&self) -> Vec<SlashItem> {
        self.slash_menu().map(|menu| slash_items(&menu.query)).unwrap_or_default()
    }

    /// Moves the menu's highlight.
    pub(crate) fn move_slash_selection(&mut self, delta: isize) {
        let count = self.slash_items().len();
        if count == 0 {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            let selected = tab.view.slash_menu.selected as isize + delta;
            let selected = selected.rem_euclid(count as isize) as usize;
            tab.view.slash_menu.selected = selected;
        }
    }

    /// Closes the `/` menu.
    pub(crate) fn close_slash_menu(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.view.slash_menu = SlashMenu::default();
        }
    }

    /// Runs the highlighted `/` row: replaces `/query` and inserts the block.
    /// The caret lands after the inserted text.
    pub(crate) fn submit_slash_menu(&mut self, tx: &mut Tx) {
        let Some((start, selected)) = self.slash_menu().map(|menu| (menu.start, menu.selected)) else { return };
        let items = self.slash_items();
        let Some(item) = items.get(selected).cloned() else { return };
        let range = start..start + 1 + self.slash_menu().map_or(0, |menu| menu.query.len());
        let before = {
            let Some(tab) = self.active_tab_mut() else { return };
            let Some(editor) = tab.view.editor.as_mut() else { return };
            if editor.replace_range(range.clone(), "") {
                editor.set_selection(range.start..range.start);
                editor.text().to_owned()
            } else {
                return;
            }
        };
        self.close_slash_menu();
        self.editor_changed();
        if !self.run_command(item.command, tx) {
            return;
        }
        let after = self
            .active_tab()
            .and_then(|tab| tab.view.editor.as_ref())
            .map(|editor| editor.text().to_owned())
            .unwrap_or_default();
        let (_, _, new_end) = crate::editor::change_range(&before, &after);
        if let Some(tab) = self.active_tab_mut()
            && let Some(editor) = tab.view.editor.as_mut()
        {
            let end = new_end.min(editor.text().len());
            editor.set_selection(end..end);
        }
        self.focus_editor_field(tx);
    }

    /// The `/` menu row clicked with the pointer.
    pub(crate) fn pick_slash_item(&mut self, index: usize, tx: &mut Tx) {
        if let Some(tab) = self.active_tab_mut() {
            tab.view.slash_menu.selected = index;
        }
        self.submit_slash_menu(tx);
    }

    /// Opens the go-to-line prompt with the current line filled in.
    pub(crate) fn open_goto_line(&mut self, tx: &mut Tx) {
        if self.active_tab().is_none_or(|tab| tab.view.mode != Mode::Editor) {
            return;
        }
        let line = self.current_editor_line() + 1;
        self.hide_narrow_preview();
        self.editor.goto_line = Some(GotoLine { input: TextEdit::new(line.to_string()), error: None });
        tx.focus(GOTO_FIELD_KEY, false);
    }

    fn current_editor_line(&self) -> usize {
        let Some(editor) = self.active_tab().and_then(|tab| tab.view.editor.as_ref()) else { return 0 };
        let byte = editor.selection().start;
        let text = editor.text();
        text[..byte.min(text.len())].bytes().filter(|&byte| byte == b'\n').count()
    }

    /// Closes the prompt without moving the caret.
    pub(crate) fn cancel_goto_line(&mut self) {
        self.editor.goto_line = None;
    }

    /// Applies the go-to-line field (E-31). Plain numbers are checked against
    /// the session's line count; `+N`, `-N`, percentages and `line:col` reach
    /// [`lmdv_edit::format::goto_line`]'s own grammar.
    pub(crate) fn submit_goto_line(&mut self, tx: &mut Tx) {
        let Some(prompt) = self.editor.goto_line.as_ref() else { return };
        let spec = prompt.input.text.trim().to_owned();
        if spec.is_empty() {
            self.editor.goto_line = None;
            self.focus_editor_field(tx);
            return;
        }
        let line_count =
            self.active_tab().and_then(|tab| tab.view.editor.as_ref()).map_or(1, |editor| editor.line_count());
        if let Some(number) = plain_line_number(&spec)
            && (number == 0 || number > line_count)
        {
            if let Some(prompt) = self.editor.goto_line.as_mut() {
                prompt.error = Some(lmdv_i18n::t!("goto.pastEnd", "n" => number, "total" => line_count));
            }
            return;
        }
        let reached = {
            let Some(tab) = self.active_tab_mut() else { return };
            let Some(editor) = tab.view.editor.as_mut() else { return };
            if editor.block_mode_active() { goto_block_line(editor, &spec) } else { goto_line(&mut editor.edit, &spec) }
        };
        match reached {
            Some(_) => {
                self.editor.goto_line = None;
                self.scroll_to_editor_selection(tx);
                self.focus_editor_field(tx);
            }
            None => {
                if let Some(prompt) = self.editor.goto_line.as_mut() {
                    prompt.error = Some(lmdv_i18n::t!("goto.invalid").to_owned());
                }
            }
        }
    }

    /// Handles the find bar's keys before the field editor. `Esc` closes and
    /// returns focus to the source; `Enter`/`F3` step; `Ctrl+F`/`Ctrl+H` switch
    /// face.
    pub(crate) fn find_key(&mut self, field: FindField, event: &KeyEvent, tx: &mut Tx) -> bool {
        match event.key {
            Key::Escape => {
                self.close_find(tx);
                return true;
            }
            Key::Enter => {
                if field == FindField::Replace && !event.mods.shift {
                    self.replace_current_cmd(tx);
                } else if event.mods.shift {
                    self.find_prev_cmd(tx);
                } else {
                    self.find_next_cmd(tx);
                }
                return true;
            }
            Key::Function(3) => {
                if event.mods.shift {
                    self.find_prev_cmd(tx);
                } else {
                    self.find_next_cmd(tx);
                }
                return true;
            }
            _ => {}
        }
        if event.is_command('f') && !event.mods.alt && !event.mods.shift {
            self.open_find(false, tx);
            return true;
        }
        if event.is_command('h') && !event.mods.alt && !event.mods.shift {
            self.open_find(true, tx);
            return true;
        }
        if event.is_command('g') && !event.mods.alt && !event.mods.shift {
            self.open_goto_line(tx);
            return true;
        }
        // The editor's format chords are not text-field commands: do not let
        // them reformat the document from inside the find bar.
        if is_format_chord(event) {
            return true;
        }
        false
    }

    /// Handles the go-to-line field's keys.
    pub(crate) fn goto_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        match event.key {
            Key::Escape => {
                self.cancel_goto_line();
                self.focus_editor_field(tx);
                true
            }
            Key::Enter => {
                self.submit_goto_line(tx);
                true
            }
            _ => false,
        }
    }

    /// The editor keymap's transient UI and formatting keys. Called from the
    /// source field first, and from the global `on_key` when the focus is in
    /// the preview or a toolbar, so `Ctrl+F` always finds in edit mode (E-21).
    pub(crate) fn editor_ui_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if self.slash_menu().is_some() {
            match event.key {
                Key::Escape => {
                    self.close_slash_menu();
                    return true;
                }
                Key::Enter => {
                    self.submit_slash_menu(tx);
                    return true;
                }
                Key::Up => {
                    self.move_slash_selection(-1);
                    return true;
                }
                Key::Down => {
                    self.move_slash_selection(1);
                    return true;
                }
                _ => {}
            }
        }
        if event.key == Key::Escape {
            if self.editor.goto_line.is_some() {
                self.cancel_goto_line();
                return true;
            }
            if self.find_bar_visible() {
                self.close_find(tx);
                return true;
            }
            return false;
        }
        if event.is_command('f') && !event.mods.alt && !event.mods.shift {
            self.open_find(false, tx);
            return true;
        }
        if event.is_command('h') && !event.mods.alt && !event.mods.shift {
            self.open_find(true, tx);
            return true;
        }
        if event.is_command('g') && !event.mods.shift {
            self.open_goto_line(tx);
            return true;
        }
        if event.key == Key::Function(3) {
            if event.mods.shift {
                self.find_prev_cmd(tx);
            } else {
                self.find_next_cmd(tx);
            }
            return true;
        }
        if let Some(id) = editor_chord(event) {
            return self.run_command(id, tx);
        }
        false
    }

    /// [`Self::editor_ui_key`] for the global router, which only reaches here
    /// when no focused field consumed the key.
    pub(crate) fn editor_context_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if self.active_tab().is_none_or(|tab| tab.view.mode != Mode::Editor) {
            return false;
        }
        // E-33: after Escape left the source, Tab within two seconds leaves the
        // editor for the toolbar instead of doing anything to the text.
        if crate::indent::plain_tab(event) && self.escape_then_tab(tx) {
            return true;
        }
        // With the preview or a toolbar focused, undo/redo still walks the
        // session's journal in block mode instead of doing nothing.
        if self.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| {
            editor.block_mode_active()
                && !event.mods.alt
                && (event.is_command('z') || (event.is_command('y') && !event.mods.shift))
        }) {
            return self.block_history(event, tx);
        }
        if event.key == Key::Escape {
            if self.slash_menu().is_some() {
                self.close_slash_menu();
                return true;
            }
            if self.editor.goto_line.is_some() {
                self.cancel_goto_line();
                return true;
            }
            if self.find_bar_visible() {
                self.close_find(tx);
                return true;
            }
            return false;
        }
        if event.is_command('f') && !event.mods.alt && !event.mods.shift {
            self.open_find(false, tx);
            return true;
        }
        if event.is_command('h') && !event.mods.alt && !event.mods.shift {
            self.open_find(true, tx);
            return true;
        }
        if event.is_command('g') && !event.mods.shift {
            self.open_goto_line(tx);
            return true;
        }
        if event.key == Key::Function(3) {
            if event.mods.shift {
                self.find_prev_cmd(tx);
            } else {
                self.find_next_cmd(tx);
            }
            return true;
        }
        if let Some(id) = editor_chord(event) {
            return self.run_command(id, tx);
        }
        false
    }
}

/// The editor's own shortcuts, shown in the Help dialog beside the global
/// command table (several are context-dependent and not table entries).
pub fn editor_keys() -> Vec<(String, String)> {
    vec![
        (lmdv_i18n::t!("keys.bik").to_owned(), "Ctrl+B · Ctrl+I · Ctrl+K".to_owned()),
        (lmdv_i18n::t!("keys.findReplace").to_owned(), "Ctrl+F · Ctrl+H".to_owned()),
        (lmdv_i18n::t!("keys.findMatch").to_owned(), "F3 · Shift+F3".to_owned()),
        (lmdv_i18n::t!("keys.moveLine").to_owned(), "Alt+↑ · Alt+↓".to_owned()),
        (lmdv_i18n::t!("keys.dupLine").to_owned(), "Shift+Alt+↓".to_owned()),
        (lmdv_i18n::t!("cmd.deleteLine").to_owned(), "Ctrl+Shift+K".to_owned()),
        (lmdv_i18n::t!("cmd.comment").to_owned(), "Ctrl+/".to_owned()),
        (lmdv_i18n::t!("cm.gotoLine").to_owned(), "Ctrl+G · Ctrl+Alt+G".to_owned()),
        (lmdv_i18n::t!("keys.slash").to_owned(), "/".to_owned()),
    ]
}

/// The editor-only chords that change the document (E-23–E-31). They win over
/// the reader's `Ctrl+B`/`Ctrl+I` while editing, as in the original.
fn editor_chord(event: &KeyEvent) -> Option<&'static str> {
    let mods = event.mods;
    if event.is_command('b') && !mods.alt && !mods.shift {
        return Some("edit.bold");
    }
    if event.is_command('i') && !mods.alt && !mods.shift {
        return Some("edit.italic");
    }
    if event.is_command('x') && mods.shift && !mods.alt {
        return Some("edit.strike");
    }
    if event.is_command('m') && mods.shift && !mods.alt {
        return Some("edit.code");
    }
    if event.is_command('k') && !mods.alt && !mods.shift {
        return Some("edit.link");
    }
    if event.is_command('1') && !mods.alt && !mods.shift {
        return Some("edit.heading-1");
    }
    if event.is_command('2') && !mods.alt && !mods.shift {
        return Some("edit.heading-2");
    }
    if event.is_command('3') && !mods.alt && !mods.shift {
        return Some("edit.heading-3");
    }
    if (event.is_command('/') || event.is_command('7')) && mods.shift {
        return Some("edit.comment");
    }
    if event.is_command('/') && !mods.alt {
        return Some("edit.comment");
    }
    if event.is_command('k') && mods.shift && !mods.alt {
        return Some("edit.delete-line");
    }
    if mods.alt && !mods.ctrl && !mods.shift && event.key == Key::Up {
        return Some("edit.move-line-up");
    }
    if mods.alt && !mods.ctrl && !mods.shift && event.key == Key::Down {
        return Some("edit.move-line-down");
    }
    if mods.alt && mods.shift && !mods.ctrl && event.key == Key::Down {
        return Some("edit.duplicate-line");
    }
    if alt_letter(event, 'l') {
        return Some("edit.select-line");
    }
    None
}

fn alt_letter(event: &KeyEvent, letter: char) -> bool {
    let mods = event.mods;
    if !mods.alt || mods.ctrl || mods.cmd || mods.shift {
        return false;
    }
    match &event.key {
        Key::Command(found) => *found == letter.to_ascii_lowercase(),
        Key::Text(text) => text.eq_ignore_ascii_case(&letter.to_string()),
        _ => false,
    }
}

/// Whether a key is one of the editor's format chords, for the find fields to
/// swallow instead of reformatting the document.
fn is_format_chord(event: &KeyEvent) -> bool {
    editor_chord(event).is_some()
}

/// Matches `/query` at the start of the caret's line, outside fenced and
/// indented code and HTML blocks (E-28).
fn slash_query_at(text: &str, caret: usize) -> Option<(usize, String)> {
    let caret = caret.min(text.len());
    let line_start = text[..caret].rfind('\n').map_or(0, |offset| offset + 1);
    let before = &text[line_start..caret];
    let indent = before.len() - before.trim_start_matches([' ', '\t']).len();
    if indent >= 4 {
        return None;
    }
    let rest = &before[indent..];
    let query = rest.strip_prefix('/')?;
    if !query.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    let line = text[line_start..].split('\n').next().unwrap_or("");
    if line.trim_start().starts_with('<') {
        return None;
    }
    let mut fence: Option<(char, usize)> = None;
    for previous in text[..line_start].split('\n') {
        let trimmed = previous.trim_start();
        if let Some((marker, run)) = fence {
            let closing = trimmed.chars().take_while(|&ch| ch == marker).count();
            if closing >= run && trimmed[closing..].trim().is_empty() {
                fence = None;
            }
        } else if let Some(open) = fence_open(trimmed) {
            fence = Some(open);
        }
    }
    if fence.is_some() {
        return None;
    }
    Some((line_start + indent, query.to_owned()))
}

fn fence_open(trimmed: &str) -> Option<(char, usize)> {
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|&ch| ch == marker).count();
    if run < 3 {
        return None;
    }
    Some((marker, run))
}

fn plain_line_number(spec: &str) -> Option<usize> {
    if spec.is_empty() || !spec.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    spec.parse().ok()
}

/// Go-to-line for block mode: only plain numbers, mapped to the source.
fn goto_block_line(editor: &mut EditorSession, spec: &str) -> Option<usize> {
    let number = plain_line_number(spec)?;
    let target = number.saturating_sub(1);
    let text = editor.text();
    let start = text.split('\n').take(target).map(|line| line.len() + 1).sum::<usize>().min(text.len());
    editor.set_selection(start..start);
    Some(target)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    Bullet,
    Ordered,
    Task,
}

fn heading_level(line: &str) -> u8 {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|&byte| byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return 0;
    }
    if hashes == trimmed.len() || trimmed.as_bytes().get(hashes).is_some_and(|&byte| byte == b' ' || byte == b'\t') {
        hashes as u8
    } else {
        0
    }
}

fn list_prefix(line: &str) -> Option<(usize, ListKind)> {
    let trimmed = line.trim_start();
    let offset = line.len() - trimmed.len();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 6
        && matches!(bytes[0], b'-' | b'*' | b'+')
        && bytes[1] == b' '
        && bytes[2] == b'['
        && matches!(bytes[3], b' ' | b'x' | b'X')
        && bytes[4] == b']'
        && bytes[5] == b' '
    {
        return Some((offset + 6, ListKind::Task));
    }
    let mut digits = 0;
    while digits < bytes.len() && bytes[digits].is_ascii_digit() {
        digits += 1;
    }
    if digits > 0 && matches!(bytes.get(digits), Some(b'.') | Some(b')')) && bytes.get(digits + 1) == Some(&b' ') {
        return Some((offset + digits + 2, ListKind::Ordered));
    }
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*' | b'+') && bytes[1] == b' ' {
        return Some((offset + 2, ListKind::Bullet));
    }
    None
}

fn marker_active(text: &str, from: usize, to: usize, marker: &str) -> bool {
    let (mut from, mut to) = (from.min(text.len()), to.min(text.len()));
    if from > to {
        std::mem::swap(&mut from, &mut to);
    }
    if from == to {
        match word_at(text, from) {
            Some((start, end)) => {
                from = start;
                to = end;
            }
            None => return false,
        }
    } else {
        let selected = &text[from..to];
        let leading = selected.len() - selected.trim_start().len();
        let trailing = selected.len() - selected.trim_end().len();
        from += leading;
        to = to.saturating_sub(trailing);
        if from > to {
            to = from;
        }
    }
    let inner = &text[from..to];
    let width = marker.len();
    if inner.len() >= 2 * width
        && inner.starts_with(marker)
        && inner.ends_with(marker)
        && (marker != "*" || !inner.starts_with("**") || inner.starts_with("***"))
    {
        return true;
    }
    wrapped_by(text, from, to, marker)
}

fn wrapped_by(text: &str, from: usize, to: usize, marker: &str) -> bool {
    let width = marker.len();
    if from < width || to + width > text.len() {
        return false;
    }
    if marker == "*" || marker == "**" {
        let before = run_len_before(text, from, '*');
        let after = run_len_after(text, to, '*');
        if marker == "*" { before % 2 == 1 && after % 2 == 1 } else { before >= 2 && after >= 2 }
    } else {
        text[..from].ends_with(marker) && text[to..].starts_with(marker)
    }
}

fn run_len_before(text: &str, at: usize, ch: char) -> usize {
    let mut len = 0;
    for found in text[..at].chars().rev() {
        if found != ch {
            break;
        }
        len += 1;
    }
    len
}

fn run_len_after(text: &str, at: usize, ch: char) -> usize {
    let mut len = 0;
    for found in text[at..].chars() {
        if found != ch {
            break;
        }
        len += 1;
    }
    len
}

fn word_at(text: &str, at: usize) -> Option<(usize, usize)> {
    let at = at.min(text.len());
    let mut start = at;
    while let Some((offset, ch)) = text[..start].char_indices().next_back() {
        if is_word(ch) {
            start = offset;
        } else {
            break;
        }
    }
    let mut end = at;
    for (offset, ch) in text[at..].char_indices() {
        if is_word(ch) {
            end = at + offset + ch.len_utf8();
        } else {
            break;
        }
    }
    (start < end).then_some((start, end))
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_query_only_opens_at_a_line_start_outside_fences() {
        assert_eq!(slash_query_at("text\n/ta", 8), Some((5, "ta".to_owned())));
        assert_eq!(slash_query_at("  /ta", 5), Some((2, "ta".to_owned())));
        assert_eq!(slash_query_at("text /ta", 8), None);
        assert_eq!(slash_query_at("```\n/ta", 7), None);
        assert_eq!(slash_query_at("text\n```js\n/ta", 12), None);
        assert_eq!(slash_query_at("text\n```js\ncode\n```\n/ta", 23), Some((20, "ta".to_owned())));
        assert_eq!(slash_query_at("    /ta", 7), None, "indented code is suppressed");
    }

    #[test]
    fn slash_items_match_the_start_of_any_word() {
        let all = slash_items("");
        assert_eq!(all.len(), SLASH_ACTIONS.len());
        let table: Vec<&str> = slash_items("ta").iter().map(|item| item.command).collect();
        assert!(table.contains(&"edit.insert-table"), "{table:?}");
        assert!(table.contains(&"edit.list-task"), "{table:?}");
        assert!(!table.contains(&"edit.insert-tip"), "`ta` does not match Tip: {table:?}");
        assert!(slash_items("ti").iter().any(|item| item.command == "edit.insert-tip"));
        assert!(!table.contains(&"edit.quote"), "{table:?}");
        assert_eq!(slash_items("table").len(), 1);
        assert!(slash_items("zzz").is_empty());
    }

    #[test]
    fn headings_and_lists_are_detected_for_the_format_bar() {
        assert_eq!(heading_level("## Two"), 2);
        assert_eq!(heading_level("  ### Three"), 3);
        assert_eq!(heading_level("#tag"), 0);
        assert_eq!(heading_level("text"), 0);
        assert_eq!(list_prefix("- [x] task").map(|(_, kind)| kind), Some(ListKind::Task));
        assert_eq!(list_prefix("12. ordered").map(|(_, kind)| kind), Some(ListKind::Ordered));
        assert_eq!(list_prefix("  - bullet").map(|(_, kind)| kind), Some(ListKind::Bullet));
    }

    #[test]
    fn inline_active_state_follows_the_markers() {
        let text = "**bold** and *italic* and `code`";
        assert!(marker_active(text, 2, 2, "**"));
        assert!(!marker_active(text, 2, 2, "*") || text.starts_with("**"));
        assert!(marker_active(text, 14, 14, "*"));
        assert!(marker_active("`code`", 2, 2, "`"));
        assert!(!marker_active("`code`", 0, 0, "~~"));
        assert!(!marker_active("plain", 2, 2, "~~"));

        let selected = "**bold**";
        assert!(marker_active(selected, 0, selected.len(), "**"));
        assert!(!marker_active(selected, 0, selected.len(), "~~"));
    }

    #[test]
    fn the_editor_font_is_the_reading_size_at_875() {
        let default = EditorMetrics::for_reader(16);
        assert_eq!(default.size, 14.0, "16 pt reads, 14 pt edits (E-37)");
        assert!((default.line_height - 22.4).abs() < 1e-4, "1.6 lines at 14 pt");
        let largest = EditorMetrics::for_reader(22);
        assert_eq!(largest.size, 19.25, "22 pt reads, 19.25 pt edits");
        assert!((largest.line_height - 30.8).abs() < 1e-4, "the line height stays proportional");
        assert_eq!(EditorMetrics::for_reader(13).size, 11.375, "the smallest step scales too");
    }

    #[test]
    fn the_editor_field_keeps_the_kit_mono_family() {
        let kit = rune_kit::Kit::neutral(false);
        let style = EditorMetrics::for_reader(16).field(kit.ty.mono);
        assert_eq!(style.size, 14.0);
        assert_eq!(style.family, kit.ty.mono.family, "the source stays monospaced");
        assert_eq!(style.weight, kit.ty.mono.weight);
        assert_eq!(style.line_height, 22.4);
        assert_ne!(style.size, kit.ty.mono.size, "the kit's 12 pt control size is not the editor's");
    }
}
