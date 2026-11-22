//! P3-03 integration: the find/replace bar, format bar, `/` menu, go-to-line
//! and the editor keymap.

use rune::{ElementInfo, Key, KeyEvent, Mods, Tx};
use tempfile::TempDir;

use crate::edit_ui::{FormatAction, FormatMenu, FormatMenuRow};
use crate::test_support;
use crate::view::view;
use crate::window::{Mode, Tab, WindowState};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn enter_editor(state: &mut WindowState) {
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx), "edit mode");
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));
}

fn command(letter: char) -> KeyEvent {
    KeyEvent { key: Key::Command(letter), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } }
}

fn shift_command(letter: char) -> KeyEvent {
    KeyEvent { key: Key::Command(letter), mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } }
}

fn alt_key(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { alt: true, ..Mods::default() } }
}

fn text(state: &WindowState) -> String {
    state.active_tab().and_then(Tab::document).map(|doc| doc.text().text().to_owned()).unwrap_or_default()
}

fn selection(state: &WindowState) -> std::ops::Range<usize> {
    state.active_tab().and_then(|tab| tab.view.editor.as_ref()).map(|editor| editor.selection()).unwrap_or(0..0)
}

fn type_source(state: &mut WindowState, text: &str) {
    state.active_editor_edit().insert(text);
    state.editor_changed();
}

fn render(state: WindowState) -> Vec<ElementInfo> {
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.elements()
}

fn has_key(elements: &[ElementInfo], key: &str) -> bool {
    elements.iter().any(|element| element.key.as_deref() == Some(key))
}

#[test]
fn ctrl_f_opens_the_bar_and_steps_matches_both_ways() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Find", "one two one\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();

    // The editor's own keymap opens the bar even when a toolbar has focus.
    assert!(state.editor_context_key(&command('f'), &mut tx));
    assert!(state.find_bar_visible());
    state.set_find_query("one");
    assert_eq!(state.find_count(), (2, 0), "two hits, the first selected");

    // Ctrl+B inside the query field is not the editor's bold: it must not
    // reformat the document behind the bar.
    let before = text(&state);
    assert!(state.find_key(crate::edit_ui::FindField::Query, &command('b'), &mut tx));
    assert_eq!(text(&state), before, "the find field does not format the document");

    state.find_next_cmd(&mut tx);
    assert_eq!(selection(&state), 8..11, "the next hit is selected");
    state.find_next_cmd(&mut tx);
    assert_eq!(selection(&state), 0..3, "find wraps");
    state.find_prev_cmd(&mut tx);
    assert_eq!(selection(&state), 8..11, "previous wraps back");

    // Escape closes and keeps the query for the next F3.
    assert!(state.find_key(
        crate::edit_ui::FindField::Query,
        &KeyEvent { key: Key::Escape, mods: Mods::default() },
        &mut tx
    ));
    assert!(!state.find_bar_visible());
    assert!(state.find_key(
        crate::edit_ui::FindField::Query,
        &KeyEvent { key: Key::Function(3), mods: Mods::default() },
        &mut tx
    ));
    let _ = state;
}

#[test]
fn replace_current_and_replace_all_are_undo_groups() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Replace", "one two one\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.open_find(true, &mut tx);
    state.set_find_query("one");
    state.set_find_replace("1");

    state.replace_current_cmd(&mut tx);
    assert_eq!(text(&state), "1 two one\n", "the first hit was replaced");
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").undo();
    state.editor_changed();
    assert_eq!(text(&state), "one two one\n", "one replace is one undo step");

    state.set_find_query("one");
    state.set_find_replace("1");
    state.replace_all_cmd();
    assert_eq!(text(&state), "1 two 1\n");
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").undo();
    state.editor_changed();
    assert_eq!(text(&state), "one two one\n", "replace all is one undo group");
}

#[test]
fn find_options_and_an_invalid_regex_show_an_error_instead_of_panicking() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Find", "Cat cat CAT\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.open_find(false, &mut tx);
    state.set_find_query("cat");
    assert_eq!(state.find_count(), (3, 0));

    state.toggle_find_case();
    assert_eq!(state.find_count(), (1, 0), "case-sensitive finds one");
    state.toggle_find_case();

    state.toggle_find_whole_word();
    assert_eq!(state.find_count(), (3, 0), "all three are whole words");
    state.toggle_find_whole_word();

    state.toggle_find_regex();
    state.set_find_query("(");
    assert!(state.find_error().is_some(), "the compile error is shown");
    assert_eq!(state.find_count(), (0, 0));
    let elements = render(state);
    assert!(has_key(&elements, "find-error"), "the error is drawn, not a crash");
    assert!(has_key(&elements, "find-query"));
    assert!(has_key(&elements, "find-count"));
}

#[test]
fn an_empty_query_clears_the_hits() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Find", "one one\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.open_find(false, &mut tx);
    state.set_find_query("one");
    assert_eq!(state.find_count(), (2, 0));
    state.set_find_query("");
    assert_eq!(state.find_count(), (0, 0));
}

#[test]
fn the_find_bar_is_per_tab_and_scene_editor_shows_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("One", "one\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.open_find(false, &mut tx);
    assert!(state.find_bar_visible());
    let second = state.open_synthetic("Two", "two\n");
    assert!(!state.find_bar_visible(), "the second tab has its own closed bar");
    assert!(state.activate_tab(first));
    assert!(state.find_bar_visible(), "the first tab's bar comes back");

    let mut state = window(&temp);
    crate::view::scene_editor(&mut state);
    assert!(state.find_bar_visible());
    assert!(state.find_count().0 > 0, "the scene selects a real match");
    let elements = render(state);
    for key in ["find-bar", "find-query", "find-replace", "find-count", "format-bar", "format-inline"] {
        assert!(has_key(&elements, key), "{key} is drawn");
    }
    assert!(elements.iter().any(|element| element.key.as_deref() == Some("find-query") && element.role == "textbox"));
    let _ = second;
}

#[test]
fn the_editor_keymap_formats_and_does_not_toggle_the_reading_panes() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Keys", "word\nsecond\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.active_editor_edit().set_selection(0, 4);

    assert!(state.editor_key(&command('b'), &mut tx));
    assert_eq!(text(&state), "**word**\nsecond\n");
    assert_eq!(state.ui.last_command.as_deref(), Some("edit.bold"));
    assert!(!state.left.is_open() && !state.right.is_open(), "E-22: the editor's Ctrl+B is bold");

    state.active_editor_edit().set_selection(2, 6);
    assert!(state.editor_key(&command('i'), &mut tx));
    assert_eq!(text(&state), "***word***\nsecond\n", "italic joins the bold run");
    assert_eq!(state.ui.last_command.as_deref(), Some("edit.italic"));

    state.active_editor_edit().set_caret(13);
    assert!(state.editor_key(&shift_command('x'), &mut tx));
    assert!(state.editor_key(&shift_command('m'), &mut tx));
    assert!(state.editor_key(&shift_command('k'), &mut tx), "Ctrl+Shift+K deletes the line");
    assert_eq!(text(&state), "***word***\n", "strike + code + delete line");

    // Ctrl+1/2/3 set the heading level on the caret's line.
    state.active_editor_edit().set_caret(0);
    assert!(state.editor_key(&command('2'), &mut tx));
    assert!(text(&state).starts_with("## "));
    assert!(state.editor_key(&command('2'), &mut tx));
    assert!(text(&state).starts_with("***word***"), "the same heading again removes it");

    // Alt+Up/Down and Alt+L operate on the selection.
    state.active_editor_edit().set_caret(1);
    assert!(state.editor_key(&alt_key(Key::Up), &mut tx));
    assert!(state.editor_key(&alt_key(Key::Down), &mut tx));
    assert!(state.editor_key(&alt_key(Key::Command('l')), &mut tx));
    assert_eq!(state.ui.last_command.as_deref(), Some("edit.select-line"));
}

#[test]
fn format_commands_apply_the_selection_and_report_pressed_state() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Format", "word\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.active_editor_edit().set_selection(0, 4);

    assert!(state.run_command("edit.bold", &mut tx));
    assert_eq!(text(&state), "**word**\n");
    assert!(crate::edit_ui::format_state(state.active_tab().expect("tab")).bold);
    assert!(state.run_command("edit.bold", &mut tx));
    assert_eq!(text(&state), "word\n");
    assert!(!crate::edit_ui::format_state(state.active_tab().expect("tab")).bold);

    state.active_editor_edit().set_caret(0);
    assert!(state.run_command("edit.heading-2", &mut tx));
    assert_eq!(text(&state), "## word\n");
    assert_eq!(crate::edit_ui::format_state(state.active_tab().expect("tab")).heading, 2);
    assert!(state.run_command("edit.list-bullet", &mut tx));
    assert_eq!(text(&state), "- ## word\n", "prefix commands edit the line after the heading marker");

    // Blocks insert at the caret with blank-line separation.
    let end = text(&state).len();
    state.active_editor_edit().set_caret(end);
    assert!(state.run_command("edit.insert-table", &mut tx));
    assert!(text(&state).contains("| Column | Column | Column |"));
}

#[test]
fn the_format_bar_reads_setext_headings_while_the_commands_toggle_atx() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Setext", "Title\n=====\n\nSubtitle\n--------\n\nParagraph\n\n---\n");
    enter_editor(&mut state);
    let heading = |state: &WindowState| crate::edit_ui::format_state(state.active_tab().expect("tab")).heading;

    // H1: the text line and its underline both read as level 1.
    state.active_editor_edit().set_caret(2);
    assert_eq!(heading(&state), 1, "the text above `=====` is H1");
    state.active_editor_edit().set_caret(8);
    assert_eq!(heading(&state), 1, "the `=====` underline reads as H1 too");

    // H2: the same for `--------`.
    let subtitle = text(&state).find("Subtitle").expect("H2 text");
    state.active_editor_edit().set_caret(subtitle + 2);
    assert_eq!(heading(&state), 2);
    let underline = text(&state).find("--------").expect("H2 underline");
    state.active_editor_edit().set_caret(underline + 2);
    assert_eq!(heading(&state), 2);

    // A plain paragraph and a thematic break after a blank line are not headings.
    let paragraph = text(&state).find("Paragraph").expect("paragraph");
    state.active_editor_edit().set_caret(paragraph);
    assert_eq!(heading(&state), 0);
    let rule = text(&state).find("\n---\n").expect("break") + 2;
    state.active_editor_edit().set_caret(rule);
    assert_eq!(heading(&state), 0, "`---` after a blank line is a thematic break");

    // The command still toggles ATX: H1 on the Setext text line inserts `# `.
    state.active_editor_edit().set_caret(2);
    let mut tx = Tx::default();
    assert!(state.run_command("edit.heading-1", &mut tx));
    assert!(text(&state).starts_with("# Title\n=====\n"), "{:?}", text(&state));
    assert_eq!(heading(&state), 1, "the ATX marker now names the level");
}

#[test]
fn the_slash_menu_filters_and_inserts_in_place_of_the_query() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Slash", "intro\n\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.active_editor_edit().set_caret(7);
    type_source(&mut state, "/ta");

    let menu = state.slash_menu().expect("the menu opened").clone();
    assert_eq!(menu.query, "ta");
    let items = state.slash_items();
    assert!(items.iter().any(|item| item.command == "edit.insert-table"), "{items:?}");
    assert!(items.iter().any(|item| item.command == "edit.list-task"), "{items:?}");
    assert!(!items.iter().any(|item| item.command == "edit.insert-tip"), "`ta` does not match Tip: {items:?}");
    assert!(!items.iter().any(|item| item.command == "edit.quote"), "{items:?}");

    // Arrows move the highlight, Enter inserts.
    state.active_editor_edit().set_caret(10);
    state.editor_changed();
    assert_eq!(state.slash_menu().map(|menu| menu.selected), Some(0));
    assert!(state.editor_key(&KeyEvent { key: Key::Down, mods: Mods::default() }, &mut tx));
    assert_eq!(state.slash_menu().map(|menu| menu.selected), Some(1));
    assert!(state.editor_key(&KeyEvent { key: Key::Up, mods: Mods::default() }, &mut tx));
    assert_eq!(state.slash_menu().map(|menu| menu.selected), Some(0));
    assert!(state.editor_key(&KeyEvent { key: Key::Down, mods: Mods::default() }, &mut tx));
    assert!(state.editor_key(&KeyEvent { key: Key::Enter, mods: Mods::default() }, &mut tx));

    let after = text(&state);
    assert!(after.starts_with("intro\n\n| Column | Column | Column |"), "{after:?}");
    assert!(!after.contains("/ta"), "the query text was replaced: {after:?}");
    assert!(state.slash_menu().is_none(), "the menu closed");
    assert!(selection(&state).is_empty(), "the caret is left after the inserted template");
    assert!(selection(&state).start > 8);
}

#[test]
fn an_indented_slash_command_replaces_the_whole_line_and_leaves_the_caret_at_the_end() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Slash", "intro\n\n  ");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.active_editor_edit().set_caret(9);
    type_source(&mut state, "/table");
    assert!(state.slash_menu().is_some());
    assert!(state.editor_key(&KeyEvent { key: Key::Enter, mods: Mods::default() }, &mut tx));
    let after = text(&state);
    assert!(after.starts_with("intro\n\n| Column"), "{after:?}");
    assert!(!after.contains("  |"), "the indentation went with the query: {after:?}");
    assert_eq!(selection(&state).start, after.len(), "the caret is after the template");
}

#[test]
fn the_slash_menu_escape_keeps_the_query_text() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Slash", "\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    type_source(&mut state, "/table");
    assert!(state.slash_menu().is_some());
    assert!(state.editor_key(&KeyEvent { key: Key::Escape, mods: Mods::default() }, &mut tx));
    assert!(state.slash_menu().is_none());
    assert_eq!(text(&state), "/table\n");
}

#[test]
fn slash_inside_a_fence_does_not_open_the_menu() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Fence", "```\n\n```\n");
    enter_editor(&mut state);
    state.active_editor_edit().set_caret(4);
    type_source(&mut state, "/table");
    assert!(state.slash_menu().is_none());
    assert_eq!(text(&state), "```\n/table\n```\n");
}

#[test]
fn go_to_line_moves_the_caret_and_refuses_a_past_end_number() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Lines", "one\ntwo\nthree\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();

    assert!(state.editor_key(&command('g'), &mut tx));
    assert!(state.editor.goto_line.is_some());
    state.active_goto_edit().replace_text("2");
    state.submit_goto_line(&mut tx);
    assert!(state.editor.goto_line.is_none());
    assert_eq!(selection(&state), 4..4, "line 2 starts at byte 4");

    assert!(state.editor_key(&command('g'), &mut tx));
    state.active_goto_edit().replace_text("99");
    state.submit_goto_line(&mut tx);
    assert!(state.editor.goto_line.as_ref().is_some_and(|goto| goto.error.is_some()));
    assert_eq!(selection(&state), 4..4, "an invalid line does not move the caret");
    state.cancel_goto_line();
    assert!(state.editor.goto_line.is_none());

    assert!(state.editor_key(&command('g'), &mut tx));
    state.active_goto_edit().replace_text("3");
    state.submit_goto_line(&mut tx);
    assert_eq!(selection(&state), 8..8, "line 3 starts at byte 8");
}

#[test]
fn line_commands_are_one_undo_step_each() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Lines", "a\nb\nc\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.active_editor_edit().set_caret(2);

    assert!(state.run_command("edit.duplicate-line", &mut tx));
    assert_eq!(text(&state), "a\nb\nb\nc\n");
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").undo();
    state.editor_changed();
    assert_eq!(text(&state), "a\nb\nc\n");

    assert!(state.run_command("edit.delete-line", &mut tx));
    assert_eq!(text(&state), "a\nc\n");
    state.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()).expect("editor").undo();
    state.editor_changed();
    assert_eq!(text(&state), "a\nb\nc\n");
}

#[test]
fn the_insert_menu_and_the_tools_menu_draw_their_rows_and_the_hint_is_plain() {
    let temp = TempDir::new().unwrap();

    // Insert: the eight block rows, the plain hint.
    let mut state = window(&temp);
    state.open_synthetic("Menus", "intro\n");
    enter_editor(&mut state);
    state.toggle_format_menu(FormatMenu::Insert);
    assert_eq!(state.format_menu(), Some(FormatMenu::Insert));
    let elements = render(state);
    for key in ["format-insert", "format-insert-menu", "format-tools", "format-hint"] {
        assert!(has_key(&elements, key), "{key} is drawn");
    }
    for index in 0..crate::edit_ui::FORMAT_INSERT_ROWS.len() {
        assert!(has_key(&elements, &format!("format-insert-item{index}")), "Insert row {index} is drawn");
    }
    assert!(
        !elements.iter().any(|element| element.key.as_deref() == Some("format-edit.insert-table")),
        "the old eight block buttons moved into the menu"
    );
    let hint = elements
        .iter()
        .find(|element| element.key.as_deref() == Some("format-hint"))
        .and_then(|element| element.text.clone())
        .expect("the hint draws its text");
    assert_eq!(hint, lmdv_i18n::text_in(0, lmdv_i18n::key("fb.slashPlain")));
    assert_eq!(lmdv_i18n::text_in(0, lmdv_i18n::key("fb.slashPlain")), "Type / for blocks");
    assert!(!hint.contains('<'), "the hint is plain text, not kbd markup: {hint:?}");

    // Tools: find/replace, go-to-line, the five line commands and spellcheck,
    // with the two dividers the spec groups them by.
    let mut state = window(&temp);
    state.open_synthetic("Menus", "intro\n");
    enter_editor(&mut state);
    state.toggle_format_menu(FormatMenu::Tools);
    let elements = render(state);
    assert!(has_key(&elements, "format-tools-menu"));
    // The two separators take an entry slot each, so row keys shift after them.
    let separators_at = [3, crate::edit_ui::FORMAT_TOOLS_ROWS.len() - 1];
    let mut entry = 0usize;
    for index in 0..crate::edit_ui::FORMAT_TOOLS_ROWS.len() {
        if separators_at.contains(&index) {
            assert!(has_key(&elements, &format!("sep{entry}")), "separator before row {index}");
            entry += 1;
        }
        let key = format!("format-tools-item{entry}");
        assert!(has_key(&elements, &key), "Tools row {index} is drawn as {key}");
        entry += 1;
    }
    assert!(has_key(&elements, "sep3"), "the line commands are a separate group");
    assert!(has_key(&elements, "sep9"), "spellcheck is a separate group");
}

#[test]
fn the_insert_menu_rows_insert_the_same_blocks_as_the_slash_menu() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Menu", "intro\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    for row in crate::edit_ui::FORMAT_INSERT_ROWS {
        let Some(command) = row.command() else { panic!("every Insert row is a command") };
        assert!(command.starts_with("edit.insert-"), "{command}");
    }

    state.toggle_format_menu(FormatMenu::Insert);
    assert!(
        state.run_format_menu_row(FormatMenuRow::Action(FormatAction::Block(lmdv_edit::BlockKind::Table)), &mut tx)
    );
    assert!(text(&state).contains("| Column | Column | Column |"), "{:?}", text(&state));
    assert_eq!(state.format_menu(), None, "a chosen row closes the menu");

    // The divider row is the same command the `/` menu runs.
    let end = text(&state).len();
    state.active_editor_edit().set_caret(end);
    state.toggle_format_menu(FormatMenu::Insert);
    assert!(
        state.run_format_menu_row(FormatMenuRow::Action(FormatAction::Block(lmdv_edit::BlockKind::Divider)), &mut tx)
    );
    assert!(text(&state).trim_end().ends_with("---"), "{:?}", text(&state));
    assert_eq!(state.ui.last_command.as_deref(), Some("edit.insert-divider"));
}

#[test]
fn the_tools_menu_opens_find_runs_line_commands_and_toggles_spellcheck() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Tools", "a\nb\nc\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.active_editor_edit().set_caret(2);

    state.toggle_format_menu(FormatMenu::Tools);
    assert!(state.run_format_menu_row(FormatMenuRow::Find, &mut tx));
    assert!(state.find_bar_visible(), "Find opens the editor bar");
    state.close_find(&mut tx);

    state.toggle_format_menu(FormatMenu::Tools);
    assert!(state.run_format_menu_row(FormatMenuRow::Action(FormatAction::MoveLineDown), &mut tx));
    assert_eq!(text(&state), "a\nc\nb\n", "the line command ran");
    assert_eq!(state.format_menu(), None);

    // E-SPELL: the row is a checked toggle over the preference; the Settings
    // switch reads the same value.
    let before = state.shared().prefs().editor.spellcheck;
    state.toggle_format_menu(FormatMenu::Tools);
    assert!(state.run_format_menu_row(FormatMenuRow::Spellcheck, &mut tx));
    assert_eq!(state.shared().prefs().editor.spellcheck, !before);
}

#[test]
fn the_insert_menu_opens_on_its_button_and_inserts_from_a_clicked_row() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Menu", "intro\n");
    enter_editor(&mut state);
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);

    let (x, y) = runtime.find("format-insert").expect("the Insert trigger");
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.format_menu(), Some(FormatMenu::Insert), "the button opens the menu");

    let (x, y) = runtime.find("format-insert-item0").expect("the Table row");
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.format_menu(), None, "a chosen row closes the menu");
    let after =
        runtime.state.active_tab().and_then(Tab::document).map(|doc| doc.text().text().to_owned()).unwrap_or_default();
    assert!(after.contains("| Column | Column | Column |"), "{after:?}");
}

#[test]
fn the_tools_menu_opens_on_its_button_and_opens_the_find_bar() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Menu", "intro\n");
    enter_editor(&mut state);
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);

    let (x, y) = runtime.find("format-tools").expect("the Tools trigger");
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.format_menu(), Some(FormatMenu::Tools), "the button opens the menu");

    let (x, y) = runtime.find("format-tools-item0").expect("the Find row");
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.format_menu(), None, "a chosen row closes the menu");
    assert!(runtime.state.find_bar_visible(), "Find opened the editor's find bar");
    runtime.frame(1.0 / 60.0);
    let focused =
        runtime.elements().into_iter().find(|element| element.focused).and_then(|element| element.key.clone());
    assert_eq!(focused.as_deref(), Some("find-query"), "the keyboard lands in the query field");
}

#[test]
fn find_fields_are_single_line_and_the_replace_row_follows_ctrl_h() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Rows", "one two\n");
    enter_editor(&mut state);
    let mut tx = Tx::default();
    state.open_find(false, &mut tx);
    assert!(!state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("editor").find.replace_open);
    assert!(state.editor_context_key(&command('h'), &mut tx));
    assert!(state.active_tab().and_then(|tab| tab.view.editor.as_ref()).expect("editor").find.replace_open);
    let elements = render(state);
    assert!(
        elements.iter().any(|element| { element.key.as_deref() == Some("find-replace") && element.role == "textbox" })
    );
}
