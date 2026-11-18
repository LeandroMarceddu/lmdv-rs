//! The one command table behind menus, toolbar buttons, shortcuts and the
//! palette.
//!
//! Every entry has a stable English id, a display title, a category, keywords,
//! default shortcut strings, an enabled predicate and an optional checked
//! predicate. P1 ships File, View and Help commands; later slices append to the
//! same table.

use crate::window::WindowState;
use rune::{FileDialogOptions, Key, KeyEvent, Tx};

/// The menu/palette group a command belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandCategory {
    File,
    Edit,
    View,
    Tab,
    Window,
    Export,
    Settings,
    Help,
}

impl CommandCategory {
    /// The category's display name.
    pub const fn title(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
            Self::Tab => "Tab",
            Self::Window => "Window",
            Self::Export => "Export",
            Self::Settings => "Settings",
            Self::Help => "Help",
        }
    }
}

/// One command. `run` never does disk I/O; it mutates window state or asks the
/// host for a dialog or quit through `Tx`.
pub struct CommandSpec {
    /// Stable English id, never localized.
    pub id: &'static str,
    /// Display title (an i18n key in later slices).
    pub title: &'static str,
    /// Menu group.
    pub category: CommandCategory,
    /// Extra search words.
    pub keywords: &'static [&'static str],
    /// Default shortcut spellings, e.g. `Ctrl+O`, shown in Help and matched by
    /// [`key_matches`].
    pub default_keys: &'static [&'static str],
    /// What the command does.
    pub run: fn(&mut WindowState, &mut Tx),
    /// Whether it can run now. Cheap: never probes disk.
    pub enabled: fn(&WindowState) -> bool,
    /// Whether it is currently on (a toggle), for menu check marks.
    pub checked: Option<fn(&WindowState) -> bool>,
}

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSpec")
            .field("id", &self.id)
            .field("category", &self.category)
            .field("default_keys", &self.default_keys)
            .finish_non_exhaustive()
    }
}

/// The Markdown extensions the original file dialog offers.
pub const MARKDOWN_EXTENSIONS: [&str; 8] = ["md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "markdn", "mdtxt"];

fn open_file(_: &mut WindowState, tx: &mut Tx) {
    let options =
        FileDialogOptions { title: lmdv_i18n::t!("dialog.openFile").to_owned(), ..FileDialogOptions::default() }
            .filter(lmdv_i18n::t!("filter.markdown"), &MARKDOWN_EXTENSIONS);
    tx.open_file(options, |state: &mut WindowState, result, tx| match result {
        Ok(Some(path)) => {
            state.open_path(path);
            state.apply_pending_scroll(tx);
        }
        Ok(None) => {}
        Err(error) => state.notice(lmdv_i18n::t!("msg.fileDialogFailed", "err" => error)),
    });
}

fn paste_document(state: &mut WindowState, tx: &mut Tx) {
    state.paste_as_document(tx);
}

fn copy_path(state: &mut WindowState, tx: &mut Tx) {
    state.copy_path_to_clipboard(tx);
}

fn reveal(state: &mut WindowState, _: &mut Tx) {
    state.reveal_path();
}

fn new_tab(state: &mut WindowState, _: &mut Tx) {
    state.new_start_tab();
}

fn open_folder(state: &mut WindowState, tx: &mut Tx) {
    state.open_folder_dialog(tx);
}

fn folder_search(state: &mut WindowState, tx: &mut Tx) {
    state.open_folder_search(tx);
}

fn new_window(state: &mut WindowState, _: &mut Tx) {
    state.request_new_window();
}

fn move_tab_new_window(state: &mut WindowState, _: &mut Tx) {
    if let Some(tab) = state.active_tab().map(|tab| tab.id) {
        state.move_tab_to_new_window(tab);
    }
}

fn move_tab_to_window_menu(state: &mut WindowState, _: &mut Tx) {
    if let Some(tab) = state.active_tab().map(|tab| tab.id) {
        state.show_tab_window_menu(tab, None);
    }
}

fn close_tab(state: &mut WindowState, tx: &mut Tx) {
    state.close_active_tab();
    state.apply_pending_scroll(tx);
}

fn close_window(state: &mut WindowState, _: &mut Tx) {
    state.request_close_window();
}

fn toggle_edit(state: &mut WindowState, tx: &mut Tx) {
    state.toggle_editor_mode(tx);
}

fn save(state: &mut WindowState, tx: &mut Tx) {
    state.request_save_active(tx);
}

fn save_as(state: &mut WindowState, tx: &mut Tx) {
    state.request_save_as_active(tx);
}

fn reload(state: &mut WindowState, tx: &mut Tx) {
    state.request_reload(tx);
}

fn toggle_preview(state: &mut WindowState, _: &mut Tx) {
    state.toggle_preview();
}

fn find(state: &mut WindowState, tx: &mut Tx) {
    state.open_find(false, tx);
}

fn replace(state: &mut WindowState, tx: &mut Tx) {
    state.open_find(true, tx);
}

fn find_next(state: &mut WindowState, tx: &mut Tx) {
    state.find_next_cmd(tx);
}

fn find_previous(state: &mut WindowState, tx: &mut Tx) {
    state.find_prev_cmd(tx);
}

fn reader_find_open(state: &mut WindowState, tx: &mut Tx) {
    state.open_reader_find(tx);
}

fn reader_find_next(state: &mut WindowState, tx: &mut Tx) {
    state.reader_find_next(tx);
}

fn reader_find_previous(state: &mut WindowState, tx: &mut Tx) {
    state.reader_find_prev(tx);
}

fn reader_find_close(state: &mut WindowState, tx: &mut Tx) {
    state.close_reader_find(tx);
}

fn replace_current(state: &mut WindowState, tx: &mut Tx) {
    state.replace_current_cmd(tx);
}

fn replace_all(state: &mut WindowState, _: &mut Tx) {
    state.replace_all_cmd();
}

fn goto_line_command(state: &mut WindowState, tx: &mut Tx) {
    state.open_goto_line(tx);
}

fn toggle_find_case(state: &mut WindowState, _: &mut Tx) {
    state.toggle_find_case();
}

fn toggle_find_whole_word(state: &mut WindowState, _: &mut Tx) {
    state.toggle_find_whole_word();
}

fn toggle_find_regex(state: &mut WindowState, _: &mut Tx) {
    state.toggle_find_regex();
}

macro_rules! format_command {
    ($name:ident, $action:expr) => {
        fn $name(state: &mut WindowState, _: &mut Tx) {
            state.apply_format($action);
        }
    };
}

format_command!(bold, crate::edit_ui::FormatAction::Wrap("**"));
format_command!(italic, crate::edit_ui::FormatAction::Wrap("*"));
format_command!(strike, crate::edit_ui::FormatAction::Wrap("~~"));
format_command!(inline_code, crate::edit_ui::FormatAction::Wrap("`"));
format_command!(link, crate::edit_ui::FormatAction::Link);
format_command!(heading_1, crate::edit_ui::FormatAction::Heading(1));
format_command!(heading_2, crate::edit_ui::FormatAction::Heading(2));
format_command!(heading_3, crate::edit_ui::FormatAction::Heading(3));
format_command!(paragraph, crate::edit_ui::FormatAction::Heading(0));
format_command!(list_bullet, crate::edit_ui::FormatAction::Prefix(lmdv_edit::LinePrefix::Bullet));
format_command!(list_ordered, crate::edit_ui::FormatAction::Prefix(lmdv_edit::LinePrefix::Ordered));
format_command!(list_task, crate::edit_ui::FormatAction::Prefix(lmdv_edit::LinePrefix::Task));
format_command!(quote, crate::edit_ui::FormatAction::Prefix(lmdv_edit::LinePrefix::Quote));
format_command!(insert_table, crate::edit_ui::FormatAction::Block(lmdv_edit::BlockKind::Table));
format_command!(insert_mermaid, crate::edit_ui::FormatAction::Block(lmdv_edit::BlockKind::Mermaid));
format_command!(insert_math, crate::edit_ui::FormatAction::Block(lmdv_edit::BlockKind::Math));
format_command!(insert_code_block, crate::edit_ui::FormatAction::Block(lmdv_edit::BlockKind::Code));
format_command!(insert_note, crate::edit_ui::FormatAction::Block(lmdv_edit::BlockKind::Note));
format_command!(insert_tip, crate::edit_ui::FormatAction::Block(lmdv_edit::BlockKind::Tip));
format_command!(insert_warning, crate::edit_ui::FormatAction::Block(lmdv_edit::BlockKind::Warning));
format_command!(insert_divider, crate::edit_ui::FormatAction::Block(lmdv_edit::BlockKind::Divider));
format_command!(comment, crate::edit_ui::FormatAction::Comment);
format_command!(move_line_up, crate::edit_ui::FormatAction::MoveLineUp);
format_command!(move_line_down, crate::edit_ui::FormatAction::MoveLineDown);
format_command!(duplicate_line, crate::edit_ui::FormatAction::DuplicateLine);
format_command!(delete_line, crate::edit_ui::FormatAction::DeleteLine);
format_command!(select_line, crate::edit_ui::FormatAction::SelectLine);

fn quit(state: &mut WindowState, _: &mut Tx) {
    state.request_quit();
}

fn toggle_browser(state: &mut WindowState, _: &mut Tx) {
    state.left.toggle();
}

fn toggle_outline(state: &mut WindowState, tx: &mut Tx) {
    state.toggle_outline();
    // Closing the outline lets the section line appear once the page settles
    // (O-07); opening it takes the line away at once.
    tx.wake_at(state.ui.last_now + crate::section::SECTION_SETTLE_SECONDS + 0.02);
}

fn help_keys(state: &mut WindowState, _: &mut Tx) {
    state.open_help(crate::window::HelpView::Shortcuts);
}

fn open_settings(state: &mut WindowState, tx: &mut Tx) {
    state.open_settings(tx);
}

fn larger_text(state: &mut WindowState, _: &mut Tx) {
    state.bump_reader_size(1);
}

fn smaller_text(state: &mut WindowState, _: &mut Tx) {
    state.bump_reader_size(-1);
}

fn reset_text_size(state: &mut WindowState, _: &mut Tx) {
    state.reset_reader_size();
}

fn cycle_theme(state: &mut WindowState, _: &mut Tx) {
    state.cycle_theme();
}

fn share_menu(state: &mut WindowState, _: &mut Tx) {
    if state.share.menu_open {
        state.close_share_menu();
        return;
    }
    state.open_share_menu();
}

fn share_copy_rich(state: &mut WindowState, _: &mut Tx) {
    state.start_rich_copy();
}

fn share_print(state: &mut WindowState, _: &mut Tx) {
    state.print_document();
}

fn share_pdf(state: &mut WindowState, tx: &mut Tx) {
    state.request_export(crate::share::ExportFormat::Pdf, tx);
}

fn share_html(state: &mut WindowState, tx: &mut Tx) {
    state.request_export(crate::share::ExportFormat::Html, tx);
}

fn share_docx(state: &mut WindowState, tx: &mut Tx) {
    state.request_export(crate::share::ExportFormat::Docx, tx);
}

fn share_break_h1(state: &mut WindowState, _: &mut Tx) {
    state.toggle_print_break_h1();
}

fn table_copy(state: &mut WindowState, tx: &mut Tx) {
    state.copy_table(tx);
}

fn table_preview(state: &mut WindowState, tx: &mut Tx) {
    state.open_table_preview(tx);
}

fn table_filter_reset(state: &mut WindowState, _: &mut Tx) {
    state.clear_table_filter();
}

fn table_sort_reset(state: &mut WindowState, _: &mut Tx) {
    state.reset_table_sort();
}

fn nav_back(state: &mut WindowState, tx: &mut Tx) {
    state.nav_back(tx);
}

fn nav_forward(state: &mut WindowState, tx: &mut Tx) {
    state.nav_forward(tx);
}

fn next_tab(state: &mut WindowState, tx: &mut Tx) {
    if state.step_tab(1) {
        state.reveal_active_tab(tx);
        state.apply_pending_scroll(tx);
    }
}

fn previous_tab(state: &mut WindowState, tx: &mut Tx) {
    if state.step_tab(-1) {
        state.reveal_active_tab(tx);
        state.apply_pending_scroll(tx);
    }
}

fn reopen_tab(state: &mut WindowState, tx: &mut Tx) {
    if state.reopen_tab() {
        state.apply_pending_scroll(tx);
    }
}

fn toggle_pin(state: &mut WindowState, _: &mut Tx) {
    state.toggle_pin();
}

fn browser_up(state: &mut WindowState, _: &mut Tx) {
    state.browser_up();
}

fn reading_settings(state: &mut WindowState, tx: &mut Tx) {
    state.open_reading_settings(tx);
}

fn open_help(state: &mut WindowState, _: &mut Tx) {
    state.open_help(crate::window::HelpView::Full);
}

fn theme_system(state: &mut WindowState, _: &mut Tx) {
    state.set_theme(lmdv_core::ThemePref::System);
}

fn theme_light(state: &mut WindowState, _: &mut Tx) {
    state.set_theme(lmdv_core::ThemePref::Light);
}

fn theme_sepia(state: &mut WindowState, _: &mut Tx) {
    state.set_theme(lmdv_core::ThemePref::Sepia);
}

fn theme_dark(state: &mut WindowState, _: &mut Tx) {
    state.set_theme(lmdv_core::ThemePref::Dark);
}

fn theme_black(state: &mut WindowState, _: &mut Tx) {
    state.set_theme(lmdv_core::ThemePref::Black);
}

fn always(_: &WindowState) -> bool {
    true
}

/// Back has a place in this tab's history (O-13).
fn nav_can_back(state: &WindowState) -> bool {
    state.nav_can_back()
}

/// Forward has a place in this tab's history (O-13).
fn nav_can_forward(state: &WindowState) -> bool {
    state.nav_can_forward()
}

fn browser_open(state: &WindowState) -> bool {
    state.left.is_open()
}

fn outline_open(state: &WindowState) -> bool {
    state.right.is_open()
}

fn document_open(state: &WindowState) -> bool {
    state.active_document_id().is_some()
}

fn editor_open(state: &WindowState) -> bool {
    state.active_tab().is_some_and(|tab| tab.view.mode == crate::window::Mode::Editor)
}

/// Whether the reading view has a parsed document (OI-098).
fn reading_document(state: &WindowState) -> bool {
    state.active_tab().is_some_and(|tab| tab.view.mode == crate::window::Mode::Viewer && tab.view.reader.is_some())
}

fn edit_from_reading(state: &mut WindowState, tx: &mut Tx) {
    state.edit_from_reading(tx);
}

/// Not editing: Ctrl+V pastes into the editor's text there (C-11).
fn viewer_mode(state: &WindowState) -> bool {
    !editor_open(state)
}

/// The command has a file path to copy or reveal.
fn has_path_target(state: &WindowState) -> bool {
    state.copy_path_target().is_some()
}

/// More than one tab: stepping wraps among them (C-11).
fn several_tabs(state: &WindowState) -> bool {
    state.tabs().len() > 1
}

/// A closed tab is waiting for Ctrl+Shift+T (T-07).
fn can_reopen(state: &WindowState) -> bool {
    state.can_reopen_tab()
}

fn theme_is_system(state: &WindowState) -> bool {
    state.settings_theme() == lmdv_core::ThemePref::System
}

fn theme_is_light(state: &WindowState) -> bool {
    state.settings_theme() == lmdv_core::ThemePref::Light
}

fn theme_is_sepia(state: &WindowState) -> bool {
    state.settings_theme() == lmdv_core::ThemePref::Sepia
}

fn theme_is_dark(state: &WindowState) -> bool {
    state.settings_theme() == lmdv_core::ThemePref::Dark
}

fn theme_is_black(state: &WindowState) -> bool {
    state.settings_theme() == lmdv_core::ThemePref::Black
}

/// The reading find bar can open: a viewer tab with a parsed article.
fn reader_ready(state: &WindowState) -> bool {
    state.active_tab().is_some_and(|tab| {
        tab.view.mode == crate::window::Mode::Viewer && tab.document().is_some() && tab.view.reader.is_some()
    })
}

/// The reading find bar is open.
fn reader_bar_open(state: &WindowState) -> bool {
    state.active_tab().is_some_and(|tab| tab.view.mode == crate::window::Mode::Viewer && tab.view.reader_find.open)
}

fn tab_movable(state: &WindowState) -> bool {
    state.active_tab().is_some_and(|tab| state.can_move_tab(tab.id))
}

fn tab_has_other_window(state: &WindowState) -> bool {
    tab_movable(state) && !state.window_targets().is_empty()
}

/// A table has been pointed at by its action bar and its model still matches.
fn table_selected(state: &WindowState) -> bool {
    state.table_target().is_some()
}

/// The table can open its preview: it has columns and at least one body row
/// (V-01).
fn table_can_preview(state: &WindowState) -> bool {
    state.table_target().is_some_and(|target| target.table.columns > 0 && !target.table.rows.is_empty())
}

/// A table preview is open.
fn table_preview_open(state: &WindowState) -> bool {
    state.table_preview().is_some()
}

/// The open preview shows a filter, applied or still in its debounce.
fn table_filter_active(state: &WindowState) -> bool {
    state
        .table_preview()
        .is_some_and(|preview| !preview.applied.trim().is_empty() || !preview.query.text.trim().is_empty())
}

/// The open preview sorts its rows.
fn table_sorted(state: &WindowState) -> bool {
    state.table_preview().is_some_and(|preview| preview.sort.is_some())
}

/// Every command in menu order.
pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "nav.back",
        title: "Back",
        category: CommandCategory::View,
        keywords: &["back", "history", "previous", "alt+left", "browser"],
        default_keys: &["Alt+Left"],
        run: nav_back,
        enabled: nav_can_back,
        checked: None,
    },
    CommandSpec {
        id: "nav.forward",
        title: "Forward",
        category: CommandCategory::View,
        keywords: &["forward", "history", "next", "alt+right", "browser"],
        default_keys: &["Alt+Right"],
        run: nav_forward,
        enabled: nav_can_forward,
        checked: None,
    },
    CommandSpec {
        id: "file.open",
        title: "Open file…",
        category: CommandCategory::File,
        keywords: &["open", "file", "markdown", "browse"],
        default_keys: &["Ctrl+O"],
        run: open_file,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "file.paste",
        title: "Paste as a new document",
        category: CommandCategory::File,
        keywords: &["paste", "clipboard", "new", "document", "markdown"],
        default_keys: &["Ctrl+V"],
        run: paste_document,
        enabled: viewer_mode,
        checked: None,
    },
    CommandSpec {
        id: "file.open-folder",
        title: "Open folder…",
        category: CommandCategory::File,
        keywords: &["open", "folder", "browse", "files", "pane"],
        default_keys: &["Ctrl+Shift+O"],
        run: open_folder,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "file.new-tab",
        title: "New tab",
        category: CommandCategory::Tab,
        keywords: &["new", "tab", "start page"],
        default_keys: &["Ctrl+T"],
        run: new_tab,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "file.new-window",
        title: "New window",
        category: CommandCategory::File,
        keywords: &["new", "window", "start page", "cascade"],
        default_keys: &["Ctrl+Shift+N"],
        run: new_window,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "tab.move-new-window",
        title: "Move to a new window",
        category: CommandCategory::Tab,
        keywords: &["move", "tab", "window", "tear", "new"],
        default_keys: &[],
        run: move_tab_new_window,
        enabled: tab_movable,
        checked: None,
    },
    CommandSpec {
        id: "tab.move-to-window",
        title: "Move tab to window…",
        category: CommandCategory::Tab,
        keywords: &["move", "tab", "window", "list", "another"],
        default_keys: &[],
        run: move_tab_to_window_menu,
        enabled: tab_has_other_window,
        checked: None,
    },
    CommandSpec {
        id: "file.close-tab",
        title: "Close tab",
        category: CommandCategory::Tab,
        keywords: &["close", "tab"],
        default_keys: &["Ctrl+W", "Ctrl+F4"],
        run: close_tab,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "tab.reopen",
        title: "Reopen closed tab",
        category: CommandCategory::Tab,
        keywords: &["reopen", "restore", "closed", "tab", "undo close"],
        default_keys: &["Ctrl+Shift+T"],
        run: reopen_tab,
        enabled: can_reopen,
        checked: None,
    },
    CommandSpec {
        id: "tab.next",
        title: "Next tab",
        category: CommandCategory::Tab,
        keywords: &["next", "tab", "cycle", "switch", "ctrl+tab"],
        default_keys: &["Ctrl+Tab", "Ctrl+PgDn"],
        run: next_tab,
        enabled: several_tabs,
        checked: None,
    },
    CommandSpec {
        id: "tab.previous",
        title: "Previous tab",
        category: CommandCategory::Tab,
        keywords: &["previous", "tab", "cycle", "switch", "back"],
        default_keys: &["Ctrl+Shift+Tab", "Ctrl+PgUp"],
        run: previous_tab,
        enabled: several_tabs,
        checked: None,
    },
    CommandSpec {
        id: "file.save",
        title: "Save",
        category: CommandCategory::File,
        keywords: &["save", "write", "disk", "ctrl+s"],
        default_keys: &["Ctrl+S"],
        run: save,
        enabled: document_open,
        checked: None,
    },
    CommandSpec {
        id: "file.save-as",
        title: "Save as…",
        category: CommandCategory::File,
        keywords: &["save", "as", "new name", "untitled", "pasted"],
        default_keys: &["Ctrl+Shift+S"],
        run: save_as,
        enabled: document_open,
        checked: None,
    },
    CommandSpec {
        id: "file.reload",
        title: "Reload from disk",
        category: CommandCategory::File,
        keywords: &["reload", "refresh", "disk", "f5"],
        default_keys: &["Ctrl+R", "F5"],
        run: reload,
        enabled: document_open,
        checked: None,
    },
    CommandSpec {
        id: "file.copy-path",
        title: "Copy path",
        category: CommandCategory::File,
        keywords: &["copy", "path", "clipboard", "file", "folder"],
        default_keys: &[],
        run: copy_path,
        enabled: has_path_target,
        checked: None,
    },
    CommandSpec {
        id: "file.reveal",
        title: "Show in folder",
        category: CommandCategory::File,
        keywords: &["reveal", "show", "folder", "file manager", "explorer", "open"],
        default_keys: &[],
        run: reveal,
        enabled: has_path_target,
        checked: None,
    },
    CommandSpec {
        id: "file.close-window",
        title: "Close window",
        category: CommandCategory::Window,
        keywords: &["close", "window", "quit"],
        default_keys: &["Ctrl+Shift+W"],
        run: close_window,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "file.quit",
        title: "Quit",
        category: CommandCategory::File,
        keywords: &["quit", "exit", "close app"],
        default_keys: &["Ctrl+Q"],
        run: quit,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "edit.toggle",
        title: "Edit document",
        category: CommandCategory::Edit,
        keywords: &["edit", "source", "write", "reading", "ctrl+e"],
        default_keys: &["Ctrl+E"],
        run: toggle_edit,
        enabled: document_open,
        checked: Some(editor_open),
    },
    CommandSpec {
        id: "edit.from-here",
        title: "Edit from here",
        category: CommandCategory::Edit,
        keywords: &["edit", "here", "reading", "anchor", "block", "source", "caret", "f2"],
        default_keys: &["F2"],
        run: edit_from_reading,
        enabled: reading_document,
        checked: None,
    },
    CommandSpec {
        id: "view.toggle-preview",
        title: "Toggle live preview",
        category: CommandCategory::View,
        keywords: &["preview", "split", "editor"],
        default_keys: &[],
        run: toggle_preview,
        enabled: editor_open,
        checked: Some(|state: &WindowState| state.editor.preview_visible),
    },
    CommandSpec {
        id: "edit.find",
        title: "Find",
        category: CommandCategory::Edit,
        keywords: &["find", "search", "query", "ctrl+f"],
        default_keys: &["Ctrl+F"],
        run: find,
        enabled: editor_open,
        checked: Some(|state: &WindowState| state.find_bar_visible()),
    },
    CommandSpec {
        id: "edit.replace",
        title: "Find and replace",
        category: CommandCategory::Edit,
        keywords: &["replace", "find", "search", "ctrl+h"],
        default_keys: &["Ctrl+H"],
        run: replace,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| {
                tab.view.find_bar.open && tab.view.editor.as_ref().is_some_and(|e| e.find.replace_open)
            })
        }),
    },
    CommandSpec {
        id: "edit.find-next",
        title: "Next match",
        category: CommandCategory::Edit,
        keywords: &["find", "next", "match", "f3"],
        default_keys: &["F3"],
        run: find_next,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.find-previous",
        title: "Previous match",
        category: CommandCategory::Edit,
        keywords: &["find", "previous", "match", "shift+f3"],
        default_keys: &["Shift+F3"],
        run: find_previous,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.goto-line",
        title: "Go to line",
        category: CommandCategory::Edit,
        keywords: &["go", "line", "jump", "ctrl+g"],
        default_keys: &["Ctrl+G", "Ctrl+Alt+G"],
        run: goto_line_command,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.replace-current",
        title: "Replace",
        category: CommandCategory::Edit,
        keywords: &["replace", "one", "current", "match"],
        default_keys: &[],
        run: replace_current,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.replace-all",
        title: "Replace all",
        category: CommandCategory::Edit,
        keywords: &["replace", "all", "matches"],
        default_keys: &[],
        run: replace_all,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.find-case",
        title: "Match case",
        category: CommandCategory::Edit,
        keywords: &["find", "case", "sensitive"],
        default_keys: &[],
        run: toggle_find_case,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.find.case_sensitive)
        }),
    },
    CommandSpec {
        id: "edit.find-whole-word",
        title: "Whole word",
        category: CommandCategory::Edit,
        keywords: &["find", "whole", "word"],
        default_keys: &[],
        run: toggle_find_whole_word,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.find.whole_word)
        }),
    },
    CommandSpec {
        id: "edit.find-regex",
        title: "Regexp",
        category: CommandCategory::Edit,
        keywords: &["find", "regex", "regular", "expression"],
        default_keys: &[],
        run: toggle_find_regex,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.find.regex)
        }),
    },
    CommandSpec {
        id: "find.open",
        title: "Find in document",
        category: CommandCategory::Edit,
        keywords: &["find", "search", "reading", "viewer", "ctrl+f"],
        // Ctrl+F is shared with `edit.find`: the two are context-exclusive, so
        // the key router picks the enabled one (viewer here, editor there).
        default_keys: &["Ctrl+F"],
        run: reader_find_open,
        enabled: reader_ready,
        checked: Some(reader_bar_open),
    },
    CommandSpec {
        id: "find.next",
        title: "Next match in document",
        category: CommandCategory::Edit,
        keywords: &["find", "next", "match", "reading", "viewer"],
        default_keys: &[],
        run: reader_find_next,
        enabled: reader_bar_open,
        checked: None,
    },
    CommandSpec {
        id: "find.prev",
        title: "Previous match in document",
        category: CommandCategory::Edit,
        keywords: &["find", "previous", "match", "reading", "viewer"],
        default_keys: &[],
        run: reader_find_previous,
        enabled: reader_bar_open,
        checked: None,
    },
    CommandSpec {
        id: "find.close",
        title: "Close find",
        category: CommandCategory::Edit,
        keywords: &["find", "close", "reading", "viewer"],
        default_keys: &[],
        run: reader_find_close,
        enabled: reader_bar_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.bold",
        title: "Bold",
        category: CommandCategory::Edit,
        keywords: &["bold", "strong", "format", "ctrl+b"],
        default_keys: &[],
        run: bold,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).bold)
        }),
    },
    CommandSpec {
        id: "edit.italic",
        title: "Italic",
        category: CommandCategory::Edit,
        keywords: &["italic", "emphasis", "format", "ctrl+i"],
        default_keys: &[],
        run: italic,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).italic)
        }),
    },
    CommandSpec {
        id: "edit.strike",
        title: "Strikethrough",
        category: CommandCategory::Edit,
        keywords: &["strike", "strikethrough", "format"],
        default_keys: &["Ctrl+Shift+X"],
        run: strike,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).strike)
        }),
    },
    CommandSpec {
        id: "edit.code",
        title: "Inline code",
        category: CommandCategory::Edit,
        keywords: &["code", "inline", "monospace", "format"],
        default_keys: &["Ctrl+Shift+M"],
        run: inline_code,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).code)
        }),
    },
    CommandSpec {
        id: "edit.link",
        title: "Insert link",
        category: CommandCategory::Edit,
        keywords: &["link", "url", "format", "ctrl+k"],
        default_keys: &["Ctrl+K"],
        run: link,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.heading-1",
        title: "Heading 1",
        category: CommandCategory::Edit,
        keywords: &["heading", "h1", "title", "format"],
        default_keys: &["Ctrl+1"],
        run: heading_1,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).heading == 1)
        }),
    },
    CommandSpec {
        id: "edit.heading-2",
        title: "Heading 2",
        category: CommandCategory::Edit,
        keywords: &["heading", "h2", "subtitle", "format"],
        default_keys: &["Ctrl+2"],
        run: heading_2,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).heading == 2)
        }),
    },
    CommandSpec {
        id: "edit.heading-3",
        title: "Heading 3",
        category: CommandCategory::Edit,
        keywords: &["heading", "h3", "format"],
        default_keys: &["Ctrl+3"],
        run: heading_3,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).heading == 3)
        }),
    },
    CommandSpec {
        id: "edit.paragraph",
        title: "Normal text",
        category: CommandCategory::Edit,
        keywords: &["paragraph", "normal", "text", "format"],
        default_keys: &[],
        run: paragraph,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).heading == 0)
        }),
    },
    CommandSpec {
        id: "edit.list-bullet",
        title: "Bulleted list",
        category: CommandCategory::Edit,
        keywords: &["bullet", "unordered", "list", "format"],
        default_keys: &[],
        run: list_bullet,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).bullet)
        }),
    },
    CommandSpec {
        id: "edit.list-ordered",
        title: "Numbered list",
        category: CommandCategory::Edit,
        keywords: &["ordered", "numbered", "list", "format"],
        default_keys: &[],
        run: list_ordered,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).ordered)
        }),
    },
    CommandSpec {
        id: "edit.list-task",
        title: "Task list",
        category: CommandCategory::Edit,
        keywords: &["task", "todo", "checkbox", "list", "format"],
        default_keys: &[],
        run: list_task,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).task)
        }),
    },
    CommandSpec {
        id: "edit.quote",
        title: "Quote",
        category: CommandCategory::Edit,
        keywords: &["quote", "blockquote", "citation", "format"],
        default_keys: &[],
        run: quote,
        enabled: editor_open,
        checked: Some(|state: &WindowState| {
            state.active_tab().is_some_and(|tab| crate::edit_ui::format_state(tab).quote)
        }),
    },
    CommandSpec {
        id: "edit.insert-table",
        title: "Table",
        category: CommandCategory::Edit,
        keywords: &["table", "grid", "columns", "rows", "insert"],
        default_keys: &[],
        run: insert_table,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.insert-mermaid",
        title: "Mermaid diagram",
        category: CommandCategory::Edit,
        keywords: &["mermaid", "diagram", "flowchart", "insert"],
        default_keys: &[],
        run: insert_mermaid,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.insert-math",
        title: "Formula block",
        category: CommandCategory::Edit,
        keywords: &["math", "formula", "latex", "insert"],
        default_keys: &[],
        run: insert_math,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.insert-code-block",
        title: "Code block",
        category: CommandCategory::Edit,
        keywords: &["code", "fence", "snippet", "insert"],
        default_keys: &[],
        run: insert_code_block,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.insert-note",
        title: "Note callout",
        category: CommandCategory::Edit,
        keywords: &["note", "callout", "alert", "insert"],
        default_keys: &[],
        run: insert_note,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.insert-tip",
        title: "Tip callout",
        category: CommandCategory::Edit,
        keywords: &["tip", "callout", "alert", "insert"],
        default_keys: &[],
        run: insert_tip,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.insert-warning",
        title: "Warning callout",
        category: CommandCategory::Edit,
        keywords: &["warning", "callout", "alert", "insert"],
        default_keys: &[],
        run: insert_warning,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.insert-divider",
        title: "Divider",
        category: CommandCategory::Edit,
        keywords: &["divider", "rule", "separator", "insert"],
        default_keys: &[],
        run: insert_divider,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.comment",
        title: "Comment line",
        category: CommandCategory::Edit,
        keywords: &["comment", "html", "format"],
        default_keys: &["Ctrl+/"],
        run: comment,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.move-line-up",
        title: "Move line up",
        category: CommandCategory::Edit,
        keywords: &["move", "line", "up"],
        default_keys: &[],
        run: move_line_up,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.move-line-down",
        title: "Move line down",
        category: CommandCategory::Edit,
        keywords: &["move", "line", "down"],
        default_keys: &[],
        run: move_line_down,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.duplicate-line",
        title: "Duplicate line",
        category: CommandCategory::Edit,
        keywords: &["duplicate", "copy", "line"],
        default_keys: &[],
        run: duplicate_line,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.delete-line",
        title: "Delete line",
        category: CommandCategory::Edit,
        keywords: &["delete", "line"],
        default_keys: &[],
        run: delete_line,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "edit.select-line",
        title: "Select line",
        category: CommandCategory::Edit,
        keywords: &["select", "line"],
        default_keys: &[],
        run: select_line,
        enabled: editor_open,
        checked: None,
    },
    CommandSpec {
        id: "view.toggle-browser",
        title: "Toggle file browser",
        category: CommandCategory::View,
        keywords: &["sidebar", "files", "browser", "tree", "pane"],
        default_keys: &["Ctrl+B"],
        run: toggle_browser,
        enabled: always,
        checked: Some(browser_open),
    },
    CommandSpec {
        id: "view.toggle-outline",
        title: "Toggle outline",
        category: CommandCategory::View,
        keywords: &["outline", "toc", "headings", "inspector", "pane"],
        default_keys: &["Ctrl+I"],
        run: toggle_outline,
        enabled: always,
        checked: Some(outline_open),
    },
    CommandSpec {
        id: "view.pin",
        title: "Keep on top",
        category: CommandCategory::View,
        keywords: &["pin", "always on top", "window", "float", "above"],
        default_keys: &[],
        run: toggle_pin,
        enabled: always,
        checked: Some(|state: &WindowState| state.pinned),
    },
    CommandSpec {
        id: "browser.up",
        title: "Go to parent folder",
        category: CommandCategory::View,
        keywords: &["up", "parent", "folder", "browser", "tree", "back"],
        default_keys: &[],
        run: browser_up,
        enabled: |state: &WindowState| state.browser_can_go_up(),
        checked: None,
    },
    CommandSpec {
        id: "browser.search",
        title: "Search the folder…",
        category: CommandCategory::View,
        keywords: &["search", "find", "folder", "files", "tree"],
        default_keys: &["Ctrl+Shift+F"],
        run: folder_search,
        enabled: always,
        checked: Some(browser_open),
    },
    CommandSpec {
        id: "help.keys",
        title: "Keyboard shortcuts",
        category: CommandCategory::Help,
        keywords: &["help", "keys", "shortcuts", "keyboard"],
        default_keys: &["?"],
        run: help_keys,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "app.help",
        title: "Help",
        category: CommandCategory::Help,
        keywords: &["help", "guide", "documentation", "keys"],
        default_keys: &["F1"],
        run: open_help,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "app.settings",
        title: "Settings…",
        category: CommandCategory::Settings,
        keywords: &["settings", "preferences", "options", "theme", "font", "size", "language", "appearance", "editor"],
        default_keys: &["Ctrl+,"],
        run: open_settings,
        enabled: always,
        checked: Some(|state: &WindowState| state.settings_is_open()),
    },
    CommandSpec {
        id: "app.reader-larger",
        title: "Larger text",
        category: CommandCategory::Settings,
        keywords: &["larger", "bigger", "text", "size", "zoom", "reader"],
        default_keys: &["Ctrl+="],
        run: larger_text,
        enabled: |state: &WindowState| state.can_grow_reader(),
        checked: None,
    },
    CommandSpec {
        id: "app.reader-smaller",
        title: "Smaller text",
        category: CommandCategory::Settings,
        keywords: &["smaller", "text", "size", "zoom", "reader"],
        default_keys: &["Ctrl+-"],
        run: smaller_text,
        enabled: |state: &WindowState| state.can_shrink_reader(),
        checked: None,
    },
    CommandSpec {
        id: "app.reader-size-reset",
        title: "Reset text size",
        category: CommandCategory::Settings,
        keywords: &["reset", "size", "text", "default", "reader"],
        default_keys: &["Ctrl+0"],
        run: reset_text_size,
        enabled: |state: &WindowState| state.shared().prefs().reader.size != lmdv_core::DEFAULT_READER_SIZE,
        checked: None,
    },
    CommandSpec {
        id: "app.theme-cycle",
        title: "Cycle theme",
        category: CommandCategory::Settings,
        keywords: &["theme", "dark", "light", "sepia", "black", "system", "auto", "cycle"],
        default_keys: &[],
        run: cycle_theme,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "theme.system",
        title: "Theme: System",
        category: CommandCategory::Settings,
        keywords: &["theme", "system", "auto", "appearance"],
        default_keys: &[],
        run: theme_system,
        enabled: always,
        checked: Some(theme_is_system),
    },
    CommandSpec {
        id: "theme.light",
        title: "Theme: Light",
        category: CommandCategory::Settings,
        keywords: &["theme", "light", "appearance"],
        default_keys: &[],
        run: theme_light,
        enabled: always,
        checked: Some(theme_is_light),
    },
    CommandSpec {
        id: "theme.sepia",
        title: "Theme: Sepia",
        category: CommandCategory::Settings,
        keywords: &["theme", "sepia", "warm", "paper", "appearance"],
        default_keys: &[],
        run: theme_sepia,
        enabled: always,
        checked: Some(theme_is_sepia),
    },
    CommandSpec {
        id: "theme.dark",
        title: "Theme: Dark",
        category: CommandCategory::Settings,
        keywords: &["theme", "dark", "appearance"],
        default_keys: &[],
        run: theme_dark,
        enabled: always,
        checked: Some(theme_is_dark),
    },
    CommandSpec {
        id: "theme.black",
        title: "Theme: Black",
        category: CommandCategory::Settings,
        keywords: &["theme", "black", "oled", "appearance"],
        default_keys: &[],
        run: theme_black,
        enabled: always,
        checked: Some(theme_is_black),
    },
    CommandSpec {
        id: "app.reading-settings",
        title: "Reading settings",
        category: CommandCategory::Settings,
        keywords: &["reading", "settings", "size", "font", "spacing", "theme", "width"],
        default_keys: &[],
        run: reading_settings,
        enabled: always,
        checked: None,
    },
    CommandSpec {
        id: "share.menu",
        title: "Share…",
        category: CommandCategory::Export,
        keywords: &["share", "export", "pdf", "html", "docx", "print"],
        default_keys: &["Ctrl+Shift+E"],
        run: share_menu,
        enabled: document_open,
        checked: Some(|state: &WindowState| state.share.menu_open),
    },
    CommandSpec {
        id: "share.copy-rich",
        title: "Copy as rich text",
        category: CommandCategory::Export,
        keywords: &["copy", "rich", "html", "clipboard"],
        default_keys: &["Ctrl+Shift+C"],
        run: share_copy_rich,
        enabled: document_open,
        checked: None,
    },
    CommandSpec {
        id: "share.print",
        title: "Print…",
        category: CommandCategory::Export,
        keywords: &["print", "pdf", "paper"],
        default_keys: &["Ctrl+P"],
        run: share_print,
        enabled: document_open,
        checked: None,
    },
    CommandSpec {
        id: "share.pdf",
        title: "Save as PDF",
        category: CommandCategory::Export,
        keywords: &["pdf", "export", "save", "print"],
        default_keys: &[],
        run: share_pdf,
        enabled: document_open,
        checked: None,
    },
    CommandSpec {
        id: "share.toggle-break-h1",
        title: "New page at every heading 1",
        category: CommandCategory::Export,
        keywords: &["page", "break", "heading", "print", "pdf"],
        default_keys: &[],
        run: share_break_h1,
        enabled: document_open,
        checked: Some(|state: &WindowState| state.shared().prefs().print_break_h1),
    },
    CommandSpec {
        id: "share.export-html",
        title: "Export as HTML",
        category: CommandCategory::Export,
        keywords: &["html", "export", "web", "browser"],
        default_keys: &[],
        run: share_html,
        enabled: document_open,
        checked: None,
    },
    CommandSpec {
        id: "share.export-docx",
        title: "Export as DOCX",
        category: CommandCategory::Export,
        keywords: &["docx", "word", "export", "office"],
        default_keys: &[],
        run: share_docx,
        enabled: document_open,
        checked: None,
    },
    CommandSpec {
        id: "table.copy",
        title: "Copy table (for Excel)",
        category: CommandCategory::View,
        keywords: &["table", "copy", "tsv", "excel", "spreadsheet", "clipboard"],
        default_keys: &[],
        run: table_copy,
        enabled: table_selected,
        checked: None,
    },
    CommandSpec {
        id: "table.preview",
        title: "Open table preview",
        category: CommandCategory::View,
        keywords: &["table", "preview", "zoom", "filter", "sort", "full window"],
        default_keys: &[],
        run: table_preview,
        enabled: table_can_preview,
        checked: Some(table_preview_open),
    },
    CommandSpec {
        id: "table.filter-reset",
        title: "Clear the table filter",
        category: CommandCategory::View,
        keywords: &["table", "filter", "clear", "rows", "preview"],
        default_keys: &[],
        run: table_filter_reset,
        enabled: table_filter_active,
        checked: None,
    },
    CommandSpec {
        id: "table.sort-reset",
        title: "Restore the table's row order",
        category: CommandCategory::View,
        keywords: &["table", "sort", "order", "reset", "preview"],
        default_keys: &[],
        run: table_sort_reset,
        enabled: table_sorted,
        checked: None,
    },
];

/// Looks a command up by id.
pub fn lookup_command(id: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|command| command.id == id)
}

/// The commands that exist only while editing: the palette tags them “Editor”
/// and `edit.toggle` (which enters the editor) is not one of them (C-11).
pub fn is_editor_command(id: &str) -> bool {
    matches!(
        id,
        "edit.find"
            | "edit.replace"
            | "edit.find-next"
            | "edit.find-previous"
            | "edit.goto-line"
            | "edit.replace-current"
            | "edit.replace-all"
            | "edit.find-case"
            | "edit.find-whole-word"
            | "edit.find-regex"
            | "edit.bold"
            | "edit.italic"
            | "edit.strike"
            | "edit.code"
            | "edit.link"
            | "edit.heading-1"
            | "edit.heading-2"
            | "edit.heading-3"
            | "edit.paragraph"
            | "edit.list-bullet"
            | "edit.list-ordered"
            | "edit.list-task"
            | "edit.quote"
            | "edit.insert-table"
            | "edit.insert-mermaid"
            | "edit.insert-math"
            | "edit.insert-code-block"
            | "edit.insert-note"
            | "edit.insert-tip"
            | "edit.insert-warning"
            | "edit.insert-divider"
            | "edit.comment"
            | "edit.move-line-up"
            | "edit.move-line-down"
            | "edit.duplicate-line"
            | "edit.delete-line"
            | "edit.select-line"
    )
}

/// The theme a `theme.*` command chooses, if it is one (C-11's five themes).
pub fn theme_command(id: &str) -> Option<lmdv_core::ThemePref> {
    match id {
        "theme.system" => Some(lmdv_core::ThemePref::System),
        "theme.light" => Some(lmdv_core::ThemePref::Light),
        "theme.sepia" => Some(lmdv_core::ThemePref::Sepia),
        "theme.dark" => Some(lmdv_core::ThemePref::Dark),
        "theme.black" => Some(lmdv_core::ThemePref::Black),
        _ => None,
    }
}

/// A command's title in the UI language, or its English title when the
/// catalogs have no message for it (C-03).
///
/// The catalog strings the toolbar and menus wear keep their shortcut in
/// parentheses (`Save (Ctrl+S)`); a palette row draws the shortcut in its own
/// column, so it is cut here.
pub fn command_title(state: &WindowState, command: &CommandSpec) -> String {
    if let Some(theme) = theme_command(command.id) {
        return lmdv_i18n::t!("pal.theme", "name" => crate::settings::theme_label(theme));
    }
    let message: &'static str = match command.id {
        "nav.back" => lmdv_i18n::t!("ctx.back"),
        "nav.forward" => lmdv_i18n::t!("ctx.forward"),
        "file.open" => lmdv_i18n::t!("empty.open"),
        "file.paste" => lmdv_i18n::t!("keys.paste"),
        "file.copy-path" => lmdv_i18n::t!("file.copyPath"),
        "file.reveal" => lmdv_i18n::t!("file.reveal"),
        "file.open-folder" => lmdv_i18n::t!("browser.openFolder"),
        "file.new-tab" => lmdv_i18n::t!("tab.newTab"),
        "file.new-window" => lmdv_i18n::t!("keys.newWindow"),
        "file.quit" => lmdv_i18n::t!("cmd.quit"),
        "tab.move-new-window" => lmdv_i18n::t!("tab.moveToWindow"),
        "tab.move-to-window" => lmdv_i18n::t!("tab.moveToWindowMenu"),
        "tab.reopen" => lmdv_i18n::t!("tab.reopen"),
        "tab.next" => lmdv_i18n::t!("pal.nextTab"),
        "tab.previous" => lmdv_i18n::t!("pal.prevTab"),
        "file.close-tab" => lmdv_i18n::t!("tab.closeTab"),
        "file.save" => lmdv_i18n::t!("toolbar.save"),
        "file.save-as" => lmdv_i18n::t!("tab.saveAs"),
        "file.reload" => lmdv_i18n::t!("keys.reload"),
        "file.close-window" => lmdv_i18n::t!("keys.closeWindow"),
        // The command's label follows the mode it switches to (E-01).
        "edit.toggle" => {
            if command.checked.is_some_and(|checked| checked(state)) {
                lmdv_i18n::t!("toolbar.read")
            } else {
                lmdv_i18n::t!("toolbar.edit")
            }
        }
        "edit.find" => lmdv_i18n::t!("keys.find"),
        "edit.from-here" => lmdv_i18n::t!("ctx.editHere"),
        "edit.replace" => lmdv_i18n::t!("cmd.find"),
        "edit.find-next" => lmdv_i18n::t!("cmd.findNext"),
        "edit.find-previous" => lmdv_i18n::t!("cmd.findPrev"),
        "edit.replace-current" => lmdv_i18n::t!("cm.replaceOne"),
        "edit.replace-all" => lmdv_i18n::t!("cm.replaceAll"),
        "edit.find-case" => lmdv_i18n::t!("cm.matchCase"),
        "edit.find-whole-word" => lmdv_i18n::t!("cm.byWord"),
        "edit.find-regex" => lmdv_i18n::t!("cm.regexp"),
        "find.next" => lmdv_i18n::t!("cmd.findNextDocument"),
        "find.prev" => lmdv_i18n::t!("cmd.findPrevDocument"),
        "find.close" => lmdv_i18n::t!("cmd.closeFind"),
        "edit.goto-line" => lmdv_i18n::t!("keys.gotoLine"),
        "find.open" => lmdv_i18n::t!("ctx.findInDoc"),
        "edit.bold" => lmdv_i18n::t!("cmd.bold"),
        "edit.italic" => lmdv_i18n::t!("cmd.italic"),
        "edit.strike" => lmdv_i18n::t!("cmd.strike"),
        "edit.code" => lmdv_i18n::t!("cmd.code"),
        "edit.link" => lmdv_i18n::t!("cmd.insertLink"),
        "edit.heading-1" => lmdv_i18n::t!("cmd.h1"),
        "edit.heading-2" => lmdv_i18n::t!("cmd.h2"),
        "edit.heading-3" => lmdv_i18n::t!("cmd.h3"),
        "edit.paragraph" => lmdv_i18n::t!("cmd.paragraph"),
        "edit.list-bullet" => lmdv_i18n::t!("cmd.bullet"),
        "edit.list-ordered" => lmdv_i18n::t!("cmd.ordered"),
        "edit.list-task" => lmdv_i18n::t!("cmd.task"),
        "edit.quote" => lmdv_i18n::t!("cmd.quote"),
        "edit.insert-table" => lmdv_i18n::t!("cmd.table"),
        "edit.insert-mermaid" => lmdv_i18n::t!("cmd.mermaid"),
        "edit.insert-math" => lmdv_i18n::t!("cmd.math"),
        "edit.insert-code-block" => lmdv_i18n::t!("cmd.codeblock"),
        "edit.insert-note" => lmdv_i18n::t!("cmd.note"),
        "edit.insert-tip" => lmdv_i18n::t!("cmd.tip"),
        "edit.insert-warning" => lmdv_i18n::t!("cmd.warning"),
        "edit.insert-divider" => lmdv_i18n::t!("cmd.divider"),
        "edit.comment" => lmdv_i18n::t!("cmd.comment"),
        "edit.move-line-up" => lmdv_i18n::t!("cmd.moveLineUp"),
        "edit.move-line-down" => lmdv_i18n::t!("cmd.moveLineDown"),
        "edit.duplicate-line" => lmdv_i18n::t!("cmd.copyLineDown"),
        "edit.delete-line" => lmdv_i18n::t!("cmd.deleteLine"),
        "edit.select-line" => lmdv_i18n::t!("cmd.selectLine"),
        "view.toggle-preview" => lmdv_i18n::t!("toolbar.preview"),
        "view.toggle-browser" => lmdv_i18n::t!("toolbar.tree"),
        "view.toggle-outline" => lmdv_i18n::t!("toolbar.toc"),
        "view.pin" => lmdv_i18n::t!("keys.pin"),
        "browser.up" => lmdv_i18n::t!("pane.up"),
        "browser.search" => lmdv_i18n::t!("pane.searchPlaceholder"),
        "help.keys" => lmdv_i18n::t!("cmd.keys"),
        "app.help" => lmdv_i18n::t!("toolbar.help"),
        "app.settings" => lmdv_i18n::t!("cmd.settings"),
        "app.reader-larger" => lmdv_i18n::t!("rs.larger"),
        "app.reader-smaller" => lmdv_i18n::t!("rs.smaller"),
        "app.reader-size-reset" => lmdv_i18n::t!("pal.resetSize"),
        "app.theme-cycle" => lmdv_i18n::t!("cmd.cycleTheme"),
        "app.reading-settings" => lmdv_i18n::t!("toolbar.reading"),
        "share.menu" => lmdv_i18n::t!("toolbar.share"),
        "share.copy-rich" => lmdv_i18n::t!("share.copy"),
        "share.print" => lmdv_i18n::t!("share.print"),
        "share.pdf" => lmdv_i18n::t!("share.pdf"),
        "share.toggle-break-h1" => lmdv_i18n::t!("share.breakH1"),
        "share.export-html" => lmdv_i18n::t!("share.html"),
        "share.export-docx" => lmdv_i18n::t!("share.docx"),
        "table.copy" => lmdv_i18n::t!("render.copyTable"),
        "table.preview" => lmdv_i18n::t!("zoom.open"),
        "table.filter-reset" => lmdv_i18n::t!("cmd.clearTableFilter"),
        "table.sort-reset" => lmdv_i18n::t!("cmd.restoreTableOrder"),
        _ => return command.title.to_owned(),
    };
    open_string(command, message)
}

/// A command's title from a catalog message, with its shortcut in parentheses
/// cut; the English title when the message is empty.
fn open_string(command: &CommandSpec, message: &str) -> String {
    let label = strip_trailing_keys(message);
    if label.is_empty() { command.title.to_owned() } else { label.to_owned() }
}

/// `text` without a trailing parenthesized shortcut: “Save (Ctrl+S)” → “Save”.
fn strip_trailing_keys(text: &str) -> &str {
    let trimmed = text.trim_end();
    let Some(open) = trimmed.rfind(" (") else { return trimmed };
    if !trimmed.ends_with(')') {
        return trimmed;
    }
    let inside = &trimmed[open + 2..trimmed.len() - 1];
    let shortcut = inside.contains('+') || inside.starts_with('F') || matches!(inside, "Esc" | "Del" | "Ins" | "Tab");
    if shortcut { &trimmed[..open] } else { trimmed }
}

/// Looks a command up by its default shortcut.
pub fn command_for_key(event: &KeyEvent) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|command| command.default_keys.iter().any(|chord| key_matches(event, chord)))
}

/// Looks up the first *enabled* command matching `event` in `state`.
///
/// `Ctrl+F` belongs to `edit.find` while editing and to `find.open` while
/// reading; both carry the chord and their enabled predicates are disjoint, so
/// the context decides which one the key reaches (R-16/E-21).
pub(crate) fn command_for_key_in(state: &WindowState, event: &KeyEvent) -> Option<&'static CommandSpec> {
    COMMANDS
        .iter()
        .find(|command| (command.enabled)(state) && command.default_keys.iter().any(|chord| key_matches(event, chord)))
}

/// Runs a command by id. False when the id is unknown or the command is
/// disabled.
pub fn dispatch_command(state: &mut WindowState, id: &str, tx: &mut Tx) -> bool {
    let Some(command) = lookup_command(id) else { return false };
    if !(command.enabled)(state) {
        return false;
    }
    (command.run)(state, tx);
    state.ui.last_command = Some(id.to_owned());
    true
}

impl WindowState {
    /// Runs a command by id. See [`dispatch_command`].
    pub fn run_command(&mut self, id: &str, tx: &mut Tx) -> bool {
        dispatch_command(self, id, tx)
    }
}

/// Whether a key event matches one of the table's shortcut spellings.
///
/// The grammar covers plain `Ctrl+<letter>` chords, `Ctrl+Shift+<letter>`,
/// `Ctrl+Tab`/`Ctrl+Shift+Tab`, `Ctrl+PgDn`/`Ctrl+PgUp` and their Shift
/// forms, `Ctrl+F1`…`Ctrl+F24`, bare function keys, the `Alt+←`/`Alt+→`
/// navigation chords (the original's Back/Forward) and R-11's bare `?` (the
/// shifted character, with no command modifier). Sequences, localized keys and
/// `Ctrl+1`…`Ctrl+9` (read from the physical digit key, T-09) are not
/// spellings yet.
pub fn key_matches(event: &KeyEvent, chord: &str) -> bool {
    match chord {
        // R-11's keys overlay: `?` arrives as the shifted character itself, so
        // Shift is part of the character, not a modifier to reject.
        "?" => {
            return matches!(&event.key, Key::Text(text) if text == "?")
                && !event.mods.primary()
                && !event.mods.ctrl
                && !event.mods.alt
                && !event.mods.logo;
        }
        // C-11's tab stepping; Tab alone is focus traversal, not a command.
        "Ctrl+Tab" => {
            return event.key == Key::Tab && event.mods.primary() && !event.mods.alt && !event.mods.shift;
        }
        "Ctrl+Shift+Tab" => {
            return event.key == Key::Tab && event.mods.primary() && event.mods.shift && !event.mods.alt;
        }
        // T-08/T-10's Page keys: Page Up and Page Down move along the tabs
        // under Ctrl, and move the front tab itself with Shift (T-10).
        "Ctrl+PgDn" => {
            return event.key == Key::PageDown && event.mods.primary() && !event.mods.alt && !event.mods.shift;
        }
        "Ctrl+PgUp" => {
            return event.key == Key::PageUp && event.mods.primary() && !event.mods.alt && !event.mods.shift;
        }
        "Ctrl+Shift+PgDn" => {
            return event.key == Key::PageDown && event.mods.primary() && event.mods.shift && !event.mods.alt;
        }
        "Ctrl+Shift+PgUp" => {
            return event.key == Key::PageUp && event.mods.primary() && event.mods.shift && !event.mods.alt;
        }
        _ => {}
    }
    if let Some(key) = chord.strip_prefix("Alt+") {
        let key = match key {
            "Left" => Key::Left,
            "Right" => Key::Right,
            _ => return false,
        };
        return event.key == key && event.mods.alt && !event.mods.primary() && !event.mods.shift;
    }
    if let Some(function) = chord.strip_prefix("Ctrl+F").and_then(|number| number.parse::<u8>().ok()) {
        return event.key == Key::Function(function) && event.mods.primary() && !event.mods.alt && !event.mods.shift;
    }
    if let Some(function) = chord.strip_prefix('F').and_then(|number| number.parse::<u8>().ok()) {
        return event.key == Key::Function(function) && !event.mods.primary() && !event.mods.alt;
    }
    if let Some(rest) = chord.strip_prefix("Ctrl+Shift+") {
        let Some(letter) = rest.chars().next() else { return false };
        if rest.len() != letter.len_utf8() {
            return false;
        }
        return event.is_command(letter) && event.mods.shift && !event.mods.alt;
    }
    let Some(letter) = chord.strip_prefix("Ctrl+").and_then(|rest| rest.chars().next()) else {
        return false;
    };
    if chord.len() != "Ctrl+".len() + letter.len_utf8() {
        return false;
    }
    event.is_command(letter) && !event.mods.alt && !event.mods.shift
}

#[cfg(test)]
mod tests {
    use rune::{Key, KeyEvent, Mods, Tx};
    use tempfile::TempDir;

    use super::*;
    use crate::test_support;
    use crate::window::WindowState;

    fn window(temp: &TempDir) -> WindowState {
        WindowState::new(test_support::shared(temp))
    }

    fn key(letter: char) -> KeyEvent {
        KeyEvent { key: Key::Command(letter), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } }
    }

    #[test]
    fn every_command_has_a_unique_id_and_title() {
        for (index, command) in COMMANDS.iter().enumerate() {
            assert!(lookup_command(command.id).is_some(), "{} is not in the table", command.id);
            for other in &COMMANDS[index + 1..] {
                assert_ne!(command.id, other.id, "duplicate command id");
                assert_ne!(command.title, other.title, "duplicate command title");
            }
        }
        assert!(lookup_command("file.nope").is_none());
    }

    #[test]
    fn shortcut_strings_do_not_collide() {
        for (index, command) in COMMANDS.iter().enumerate() {
            for chord in command.default_keys {
                for other in &COMMANDS[index + 1..] {
                    for other_chord in other.default_keys {
                        if chord == other_chord {
                            // One chord, two explicitly context-exclusive
                            // commands: Ctrl+F is the editor's panel while
                            // editing and the reading bar while reading
                            // (R-16/E-21).
                            let exclusive = matches!(
                                (command.id, other.id),
                                ("edit.find", "find.open") | ("find.open", "edit.find")
                            );
                            assert!(exclusive, "{} and {} share {chord}", command.id, other.id);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn ctrl_o_and_f1_resolve_to_their_commands() {
        assert_eq!(command_for_key(&key('o')).map(|command| command.id), Some("file.open"));
        assert_eq!(command_for_key(&key('t')).map(|command| command.id), Some("file.new-tab"));
        assert_eq!(command_for_key(&key('w')).map(|command| command.id), Some("file.close-tab"));
        assert_eq!(command_for_key(&key('b')).map(|command| command.id), Some("view.toggle-browser"));
        assert_eq!(command_for_key(&key('i')).map(|command| command.id), Some("view.toggle-outline"));
        assert_eq!(command_for_key(&key('q')).map(|command| command.id), Some("file.quit"));

        let f1 = KeyEvent { key: Key::Function(1), mods: Mods::default() };
        assert_eq!(command_for_key(&f1).map(|command| command.id), Some("app.help"), "F1 opens Help (C-11)");

        let f4 = KeyEvent { key: Key::Function(4), mods: Mods { ctrl: true, cmd: true, ..Mods::default() } };
        assert_eq!(command_for_key(&f4).map(|command| command.id), Some("file.close-tab"));
        assert!(command_for_key(&KeyEvent { key: Key::Function(9), mods: Mods::default() }).is_none());
        assert!(!key_matches(&key('w'), "Ctrl+Shift+W"), "a shift chord is not the plain one");
    }

    #[test]
    fn the_page_keys_carry_the_tab_commands() {
        let ctrl_pgdn = KeyEvent { key: Key::PageDown, mods: Mods { cmd: true, ctrl: true, ..Mods::default() } };
        let ctrl_pgup = KeyEvent { key: Key::PageUp, mods: Mods { cmd: true, ctrl: true, ..Mods::default() } };
        let shift_pgdn =
            KeyEvent { key: Key::PageDown, mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } };
        let shift_pgup =
            KeyEvent { key: Key::PageUp, mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } };

        // T-08: Ctrl+Page Down/Up step like Ctrl+Tab (and T-10's Shift forms
        // are their own chords, not the stepping ones).
        assert_eq!(command_for_key(&ctrl_pgdn).map(|command| command.id), Some("tab.next"));
        assert_eq!(command_for_key(&ctrl_pgup).map(|command| command.id), Some("tab.previous"));
        assert!(key_matches(&shift_pgdn, "Ctrl+Shift+PgDn"));
        assert!(key_matches(&shift_pgup, "Ctrl+Shift+PgUp"));
        assert!(!key_matches(&shift_pgdn, "Ctrl+PgDn"), "Shift moves the tab, it does not step");
        assert!(!key_matches(&ctrl_pgdn, "Ctrl+PgUp"));
        assert!(!key_matches(&KeyEvent { key: Key::PageDown, mods: Mods::default() }, "Ctrl+PgDn"));

        // The Help list writes both chords beside their command.
        let next = lookup_command("tab.next").expect("tab.next");
        assert_eq!(next.default_keys, &["Ctrl+Tab", "Ctrl+PgDn"]);
        let previous = lookup_command("tab.previous").expect("tab.previous");
        assert_eq!(previous.default_keys, &["Ctrl+Shift+Tab", "Ctrl+PgUp"]);
    }

    #[test]
    fn question_mark_opens_and_closes_the_keys_overlay() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let question = KeyEvent { key: Key::Text("?".into()), mods: Mods { shift: true, ..Mods::default() } };
        assert!(key_matches(&question, "?"), "R-11: the shifted character is the chord");
        assert_eq!(command_for_key_in(&state, &question).map(|command| command.id), Some("help.keys"));

        // Ctrl/Alt chords and a plain `/` are not the overlay.
        let ctrl = KeyEvent { key: Key::Text("?".into()), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } };
        assert!(!key_matches(&ctrl, "?"));
        assert!(!key_matches(&KeyEvent { key: Key::Text("/".into()), mods: Mods::default() }, "?"));

        let mut tx = Tx::default();
        assert!(state.run_command("help.keys", &mut tx));
        assert_eq!(state.help_view(), Some(crate::window::HelpView::Shortcuts), "the keys dialog opens");
        state.close_help();
        assert!(!state.run_command("edit.from-here", &mut tx), "the reading anchor needs a document");
    }

    #[test]
    fn edit_from_here_opens_the_source_at_the_reading_block() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Reading", "# One\n\nfirst paragraph\n\n## Two\n\nsecond\n");
        let doc = state.active_reader().expect("a model");
        let block = 2;
        let line = doc.block_spans.get(block).expect("a block span").line;
        let key = doc.block_key(block).expect("a key").to_owned();
        state.reading.reader_block.set(Some(block));

        let mut tx = Tx::default();
        let event = KeyEvent { key: Key::Function(2), mods: Mods::default() };
        assert_eq!(command_for_key_in(&state, &event).map(|command| command.id), Some("edit.from-here"));
        assert!(state.run_command("edit.from-here", &mut tx));
        assert_eq!(state.active_tab().expect("a tab").view.mode, crate::window::Mode::Editor);
        let editor = state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("the editor");
        let expected = crate::preview_nav::line_start(editor.text(), line);
        assert_eq!(editor.selection(), expected..expected, "the caret is at the block's first line in {key}");
    }

    #[test]
    fn new_tab_appends_and_activates_a_start_page() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();

        assert!(state.run_command("file.new-tab", &mut tx));
        assert_eq!(state.tabs().len(), 2);
        assert_eq!(state.active_index(), 1);
        assert!(state.active_tab().is_some_and(crate::window::Tab::is_start));
        assert_eq!(state.ui.last_command.as_deref(), Some("file.new-tab"));
    }

    #[test]
    fn close_tab_handles_a_document_and_a_lone_start_page() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();
        let tab = state.open_synthetic("One", "text");

        assert!(state.run_command("file.close-tab", &mut tx));
        assert_eq!(state.tabs().len(), 1, "the last document left a start page");
        assert!(state.tabs()[0].is_start());

        assert!(state.run_command("file.close-tab", &mut tx), "closing the lone start page is a window close");
        assert!(state.ui.close_requested);
        assert_eq!(state.tabs().len(), 1);
        assert_ne!(state.tabs()[0].id, tab, "the start page is a fresh tab after the document closed");
    }

    #[test]
    fn view_toggles_report_their_checked_state() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();
        let browser = lookup_command("view.toggle-browser").expect("browser command");
        let outline = lookup_command("view.toggle-outline").expect("outline command");

        assert!(!state.left.is_open());
        assert!(!(browser.checked.expect("checked"))(&state));
        assert!(state.run_command("view.toggle-browser", &mut tx));
        assert!(state.left.is_open());
        assert!((browser.checked.expect("checked"))(&state));
        assert!(state.run_command("view.toggle-outline", &mut tx));
        assert!(state.right.is_open());
        assert!((outline.checked.expect("checked"))(&state));
    }

    #[test]
    fn help_and_quit_change_state() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();

        assert!(state.run_command("help.keys", &mut tx));
        assert_eq!(state.help_view(), Some(crate::window::HelpView::Shortcuts));
        assert!(state.run_command("file.quit", &mut tx));
        assert!(state.ui.quit_requested);
        assert!(state.shared().is_quitting());
    }

    #[test]
    fn new_window_uses_the_opener_seam_and_never_calls_rune() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();
        let chord =
            KeyEvent { key: Key::Command('n'), mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } };
        assert_eq!(command_for_key(&chord).map(|command| command.id), Some("file.new-window"));

        // Without an opener the command still succeeds and reports why.
        assert!(state.run_command("file.new-window", &mut tx));
        assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("new window")));
        assert_eq!(state.shared().windows().len(), 1, "a unit test never opened a native window");
    }

    #[test]
    fn close_window_goes_through_the_dirty_guard() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();
        let tab = state.open_synthetic("Dirty", "text");
        let doc = state.tab(tab).and_then(crate::window::Tab::document).cloned().expect("doc");
        doc.apply(0..4, "changed");
        state.sync_tab_dirty(&doc);

        let chord =
            KeyEvent { key: Key::Command('w'), mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } };
        assert_eq!(command_for_key(&chord).map(|command| command.id), Some("file.close-window"));
        assert!(state.run_command("file.close-window", &mut tx));
        assert!(matches!(state.prompt(), Some(crate::editor::Prompt::CloseWindow)));
        assert!(!state.ui.close_requested, "the close waits for the dialog");
    }

    #[test]
    fn ctrl_f_is_context_exclusive_between_viewer_and_editor() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Find", "# Note\n\nBody\n");
        let chord = KeyEvent { key: Key::Command('f'), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } };
        assert_eq!(
            command_for_key_in(&state, &chord).map(|command| command.id),
            Some("find.open"),
            "viewer Ctrl+F opens the reading bar"
        );
        assert!(!(lookup_command("edit.find").expect("edit.find").enabled)(&state));

        let mut tx = Tx::default();
        assert!(state.enter_editor(None, &mut tx));
        assert_eq!(
            command_for_key_in(&state, &chord).map(|command| command.id),
            Some("edit.find"),
            "editor Ctrl+F keeps the editor panel"
        );
        assert!(!(lookup_command("find.open").expect("find.open").enabled)(&state));
    }

    #[test]
    fn dispatch_rejects_unknown_ids() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();
        assert!(!state.run_command("file.nope", &mut tx));
        assert!(state.ui.last_command.is_none());
    }

    #[test]
    fn open_is_the_only_command_that_asks_the_host_for_a_dialog() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut tx = Tx::default();
        assert!(state.run_command("file.open", &mut tx));
        assert_eq!(state.tabs().len(), 1, "the dialog itself does not change tabs");
        assert_eq!(state.ui.last_command.as_deref(), Some("file.open"));
    }
}
