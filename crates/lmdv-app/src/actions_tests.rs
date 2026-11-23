//! P6-CMD: C-11's command coverage and the tab context menu (T-07/T-18/T-23).
//!
//! Clipboard reads and copies run through a headless `rune::Runtime` with its
//! in-memory clipboard; the pin and reveal seams record what the native calls
//! would do.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use lmdv_core::{Document, DocumentKey, ScrollAnchor, SourceBuffer, ThemePref};
use rune::{Runtime, Tx};
use tempfile::TempDir;

use crate::actions::{ClosedTab, MAX_CLOSED_TABS, pasted_text, pasted_title};
use crate::editor::Prompt;
use crate::palette::PaletteChord;
use crate::shared::AppShared;
use crate::test_support;
use crate::window::{Mode, Tab, WindowState};

fn shared(temp: &TempDir) -> Rc<AppShared> {
    let shared = test_support::shared(temp);
    shared.set_headless();
    shared
}

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(shared(temp))
}

fn ready_file(shared: &AppShared, path: &Path, text: &str) -> Rc<Document> {
    let key = DocumentKey::File(path.to_path_buf());
    let title = path.file_name().and_then(|name| name.to_str()).unwrap_or("file");
    let id = shared.docs_mut().allocate_id();
    let document = Rc::new(Document::ready(id, key, title, SourceBuffer::new(text)));
    shared.docs_mut().insert_ready(Rc::clone(&document));
    document
}

fn poll(state: &mut WindowState) {
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
}

fn clipboard_runtime(state: WindowState) -> (Runtime<WindowState>, rune::clipboard::MemoryClipboard) {
    let board = rune::clipboard::memory();
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.use_clipboard(&board);
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.frame(0.0);
    (runtime, board)
}

/// Pumps frames until `done` holds, failing after a few seconds.
fn pump_until(runtime: &mut Runtime<WindowState>, mut done: impl FnMut(&WindowState) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(&runtime.state) {
        assert!(Instant::now() < deadline, "the clipboard callback never landed");
        runtime.frame(0.0);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn notice_text(state: &WindowState) -> String {
    state.ui.notice.as_ref().map(|notice| notice.text.clone()).unwrap_or_default()
}

// ---- notice kinds and localized key caps (H-05/H-06) ---------------------

#[test]
fn the_confirmations_keep_their_honest_notice_kind() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);

    // The messages the older handlers raise through `notice` are successes...
    for message in ["msg.saved", "msg.pathCopied", "msg.tableCopied", "msg.imageCopied"] {
        state.notice(lmdv_i18n::text(lmdv_i18n::key(message)));
        let notice = state.ui.notice.as_ref().expect("a notice");
        assert_eq!(notice.kind, crate::window::NoticeKind::Success, "{message}");
    }
    // ...anything else stays a neutral status unless the caller names a kind.
    state.notice("The folder has no Markdown files.");
    assert_eq!(state.ui.notice.as_ref().map(|notice| notice.kind), Some(crate::window::NoticeKind::Info));
    state.notice_kind("The file is larger than 8 MiB.", crate::window::NoticeKind::Warning);
    assert_eq!(state.ui.notice.as_ref().map(|notice| notice.kind), Some(crate::window::NoticeKind::Warning));
    state.notice_kind("Save failed.", crate::window::NoticeKind::Error);
    assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.kind.is_error()));
}

#[test]
fn key_caps_are_localized_per_catalog() {
    let index = |id: &str| lmdv_i18n::find_locale(id).expect("a catalog");
    assert_eq!(crate::commands::keycaps_in(0, "Ctrl+Shift+PgDn"), ["Ctrl", "Shift", "PgDn"]);
    assert_eq!(
        crate::commands::keycaps_in(index("de"), "Ctrl+Shift+PgDn"),
        ["Strg", "Umschalt", "Bild\u{2193}"],
        "H-05: German names the keys"
    );
    assert_eq!(crate::commands::keycaps_in(index("de"), "Space"), ["Leertaste"]);
    assert_eq!(crate::commands::keycaps_in(index("de"), "Shift+Space"), ["Umschalt", "Leertaste"]);
    assert_eq!(crate::commands::keycaps_in(index("fr"), "Esc"), ["\u{c9}chap"]);
    assert_eq!(crate::commands::keycaps_in(index("it"), "Ctrl+="), ["Ctrl", "="], "unnamed keys are their own cap");
}

fn anchor(id: &str, offset: f32) -> ScrollAnchor {
    ScrollAnchor { id: id.to_owned(), offset }
}

// ---- paste (C-11) --------------------------------------------------------

#[test]
fn pasted_titles_follow_the_first_level_one_heading_then_the_first_line() {
    assert_eq!(pasted_title("# Pasted notes\n\nBody"), "Pasted notes");
    assert_eq!(pasted_title("Intro\n\n# Later heading"), "Later heading", "the first h1 wins anywhere");
    assert_eq!(pasted_title("## Only a level two\n\n### Three"), "Only a level two");
    assert_eq!(pasted_title("   \n\n  a first line  \nsecond"), "a first line");
    assert_eq!(pasted_title("> quoted **first** line"), "quoted first line");
    assert_eq!(pasted_title("- [ ] a task item\n- another"), "a task item");
    assert_eq!(pasted_title("```\nnot the title\n```\n\n# Real title"), "Real title");
    assert_eq!(pasted_title("---\ntitle: yaml\n---\n\n# After front matter"), "After front matter");
    assert_eq!(pasted_title("<!-- note -->\n# Commented out"), "Commented out");
    assert!(pasted_title("").is_empty(), "no text, no title");
    assert_eq!(pasted_title("`x` and [a link](https://example.com)"), "x and a link");
}

#[test]
fn pasted_titles_recognize_setext_headings() {
    assert_eq!(pasted_title("Pasted title\n============\n\nBody"), "Pasted title");
    assert_eq!(pasted_title("A wrapped\nsetext title\n==="), "A wrapped setext title");
    assert_eq!(pasted_title("A **bold** and `code` title\n==="), "A bold and code title");
    assert_eq!(
        pasted_title("Level two\n---\n\nbody"),
        "Level two",
        "the line above a --- underline is still the first line"
    );
    assert_eq!(pasted_title("Intro\n\n***\n\n# Real"), "Real", "a thematic break is no title");
    assert_eq!(pasted_title("- item\n==="), "item", "a list line does not become a setext heading");
    assert_eq!(pasted_title("> quote\n==="), "quote");
}

#[test]
fn pasted_titles_read_the_common_inline_forms() {
    assert_eq!(pasted_title("**Bold** and _italic_ and ~~gone~~"), "Bold and italic and gone");
    assert_eq!(pasted_title("`code` + [link](https://example.com) + ![alt](image.png)"), "code + link + alt");
    assert_eq!(pasted_title("Tom &amp; Jerry &mdash; 5 &#x2192; 6"), "Tom & Jerry — 5 → 6");
    assert_eq!(pasted_title("snake_case_name"), "snake_case_name");
    assert_eq!(pasted_title("1 < 2 & 3 > 4"), "1 < 2 & 3 > 4", "a comparison keeps its text");
    assert_eq!(pasted_title("array [0] value"), "array [0] value", "a bracket without a target stays");
    assert_eq!(pasted_title("see ![alt] here"), "see ![alt] here", "an image without a target stays");
    assert_eq!(pasted_title("[ref][target] text"), "ref text");
    assert_eq!(pasted_title("`[not a link](x)`"), "[not a link](x)", "code spans keep their text as written");
    assert_eq!(pasted_title("[^1] footnoted"), "footnoted", "footnote references show nothing");
    assert_eq!(pasted_title("line one<br>line two"), "line one line two");
    assert_eq!(pasted_title("<https://example.com> docs"), "https://example.com docs");
}

#[test]
fn pasted_titles_are_capped_at_eighty_characters() {
    let long = "word ".repeat(40);
    let title = pasted_title(&long);
    assert!(title.chars().count() <= 80, "{title:?}");
    assert!(title.ends_with('…'), "{title:?}");
}

#[test]
fn pasted_text_drops_the_bom_trailing_nuls_and_crlf() {
    assert_eq!(pasted_text("\u{feff}# Title\r\n\r\nBody\r\n\0\0"), "# Title\n\nBody\n");
}

#[test]
fn file_paste_opens_the_clipboard_as_a_new_untitled_document() {
    let temp = TempDir::new().unwrap();
    let state = window(&temp);
    let (mut runtime, board) = clipboard_runtime(state);
    board.set_text("# Pasted notes\r\n\r\nBody.\r\n");

    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(state.run_command("file.paste", tx), "the command is enabled in the viewer");
    });
    pump_until(&mut runtime, |state| state.active_document_id().is_some());

    let state = &runtime.state;
    assert_eq!(state.tabs().len(), 1, "the start page became the pasted document");
    let tab = state.active_tab().expect("tab");
    assert!(!tab.is_start());
    assert_eq!(tab.full_title(), "Pasted notes");
    assert_eq!(tab.title(), "Pasted notes");
    let document = tab.document().expect("document");
    assert!(matches!(*document.key.borrow(), DocumentKey::Untitled(_)), "pasted text has no file");
    assert_eq!(document.text().text(), "# Pasted notes\n\nBody.\n", "CRLF became LF");
    assert!(notice_text(state).contains("Pasted notes"), "{}", notice_text(state));
}

#[test]
fn paste_of_nothing_or_a_picture_alone_says_so_and_opens_no_tab() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tabs = state.tabs().len();

    state.open_clipboard_text("   \n\t", false);
    assert_eq!(notice_text(&state), lmdv_i18n::t!("msg.pasteEmpty"));
    assert_eq!(state.tabs().len(), tabs);

    state.open_clipboard_text("", true);
    assert_eq!(notice_text(&state), lmdv_i18n::t!("msg.pasteImage"));
    assert_eq!(state.tabs().len(), tabs);
}

#[test]
fn file_paste_is_disabled_while_editing_so_ctrl_v_pastes_into_the_text() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n");
    let command = crate::commands::lookup_command("file.paste").expect("command");
    assert!((command.enabled)(&state));

    state.tabs[0].view.mode = Mode::Editor;
    assert!(!(command.enabled)(&state), "in the editor Ctrl+V belongs to the text field");
}

// ---- copy path / reveal (T-18) -------------------------------------------

#[test]
fn copy_path_follows_the_document_or_the_focused_browser_row() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    assert!(state.copy_path_target().is_none(), "the start page has no path");

    let path = temp.path().join("notes.md");
    let document = ready_file(state.shared(), &path, "# Notes\n");
    state.attach_document(document);
    assert_eq!(state.copy_path_target().as_deref(), Some(path.as_path()));

    // The document in front wins; a browser row is the fallback on a start page.
    let row = temp.path().join("row.md");
    state.browser.focus = Some(row.to_string_lossy().into_owned());
    assert_eq!(state.copy_path_target().as_deref(), Some(path.as_path()));

    let command = crate::commands::lookup_command("file.copy-path").expect("command");
    assert!((command.enabled)(&state));
    state.reset_to_start_page();
    assert_eq!(state.copy_path_target().as_deref(), Some(row.as_path()));
}

#[test]
fn copy_path_writes_the_path_to_the_clipboard() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let path = temp.path().join("notes.md");
    let document = ready_file(state.shared(), &path, "# Notes\n");
    state.attach_document(document);
    let (mut runtime, board) = clipboard_runtime(state);

    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(state.run_command("file.copy-path", tx));
    });
    runtime.frame(0.0);
    assert_eq!(board.text().map(|text| text.to_string()), Some(path.to_string_lossy().into_owned()));
    assert!(notice_text(&runtime.state).contains("Path copied"), "{}", notice_text(&runtime.state));
}

#[test]
fn reveal_opens_the_containing_folder_through_the_external_job() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let path = temp.path().join("docs/notes.md");
    let document = ready_file(state.shared(), &path, "# Notes\n");
    state.attach_document(document);

    let seen = Rc::new(RefCell::new(Vec::new()));
    state.shared().set_external_sink({
        let seen = Rc::clone(&seen);
        Box::new(move |path: &Path| seen.borrow_mut().push(path.to_path_buf()))
    });
    let mut tx = Tx::default();
    assert!(state.run_command("file.reveal", &mut tx));
    assert_eq!(seen.borrow().as_slice(), [temp.path().join("docs")]);
}

#[test]
fn the_tab_menu_copy_and_reveal_act_on_that_tab() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let path = temp.path().join("docs/notes.md");
    let document = ready_file(state.shared(), &path, "# Notes\n");
    let tab = state.attach_document(document);
    state.open_synthetic("Other", "# Other\n");

    let seen = Rc::new(RefCell::new(Vec::new()));
    state.shared().set_external_sink({
        let seen = Rc::clone(&seen);
        Box::new(move |path: &Path| seen.borrow_mut().push(path.to_path_buf()))
    });
    let (mut runtime, board) = clipboard_runtime(state);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(state.copy_tab_path(tab, tx), "the background tab's path copies");
    });
    runtime.frame(0.0);
    assert_eq!(board.text().map(|text| text.to_string()), Some(path.to_string_lossy().into_owned()));
    assert!(runtime.state.reveal_tab(tab));
    assert_eq!(seen.borrow().as_slice(), [temp.path().join("docs")]);
    assert!(!runtime.state.reveal_tab(runtime.state.tabs()[1].id), "a pasted tab has no folder");
}

// ---- pin (C-11) ----------------------------------------------------------

#[test]
fn pin_toggles_the_checked_state_and_sends_the_native_request() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.shared().set_pin_available(true);
    let calls = Rc::new(RefCell::new(Vec::new()));
    state.shared().set_pin_sink({
        let calls = Rc::clone(&calls);
        Box::new(move |id, on| calls.borrow_mut().push((id, on)))
    });
    let id = state.id();
    let command = crate::commands::lookup_command("view.pin").expect("command");
    let checked = command.checked.expect("checked");
    assert!(!checked(&state));
    let mut tx = Tx::default();

    assert!(state.run_command("view.pin", &mut tx));
    assert!(state.pinned);
    assert!(checked(&state));
    assert_eq!(calls.borrow().as_slice(), [(id, true)]);

    assert!(state.run_command("view.pin", &mut tx));
    assert!(!state.pinned);
    assert!(!checked(&state));
    assert_eq!(calls.borrow().as_slice(), [(id, true), (id, false)]);
}

#[test]
fn the_pin_check_belongs_to_one_window_and_is_not_carried_to_the_next() {
    let temp = TempDir::new().unwrap();
    let shared = shared(&temp);
    shared.set_pin_available(true);
    shared.set_pin_sink(Box::new(|_, _| {}));
    let mut first = WindowState::new(Rc::clone(&shared));
    let second = WindowState::new(Rc::clone(&shared));
    let command = crate::commands::lookup_command("view.pin").expect("command");
    let checked = command.checked.expect("checked");
    let mut tx = Tx::default();

    assert!(first.run_command("view.pin", &mut tx));
    assert!(first.pinned);
    assert!(checked(&first));
    assert!(!second.pinned, "W-07: the pin is per window");
    assert!(!checked(&second));

    // A window opened later starts unpinned: the state is never persisted.
    let third = WindowState::new(Rc::clone(&shared));
    assert!(!third.pinned);
    assert!(!checked(&third));
}

#[test]
fn pin_refuses_on_wayland_with_a_clear_notice() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.shared().set_pin_available(false);
    let calls = Rc::new(RefCell::new(Vec::new()));
    state.shared().set_pin_sink({
        let calls = Rc::clone(&calls);
        Box::new(move |id, on| calls.borrow_mut().push((id, on)))
    });
    let mut tx = Tx::default();

    assert!(state.run_command("view.pin", &mut tx), "the command is offered; it explains itself");
    assert!(!state.pinned, "a check that does nothing is never shown");
    assert!(calls.borrow().is_empty(), "no native call under Wayland");
    assert!(notice_text(&state).contains("Wayland"), "{}", notice_text(&state));
}

// ---- tab stepping (C-11) -------------------------------------------------

#[test]
fn next_and_previous_wrap_around_the_strip() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "one");
    state.open_synthetic("Two", "two");
    state.open_synthetic("Three", "three");
    let mut tx = Tx::default();
    assert_eq!(state.active_index(), 2);

    assert!(state.run_command("tab.next", &mut tx));
    assert_eq!(state.active_index(), 0, "wraps to the first tab");
    assert!(state.run_command("tab.next", &mut tx));
    assert_eq!(state.active_index(), 1);
    assert!(state.run_command("tab.previous", &mut tx));
    assert_eq!(state.active_index(), 0);
    assert!(state.run_command("tab.previous", &mut tx));
    assert_eq!(state.active_index(), 2, "wraps to the last tab");
}

#[test]
fn tab_stepping_needs_more_than_one_tab() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let next = crate::commands::lookup_command("tab.next").expect("command");
    let previous = crate::commands::lookup_command("tab.previous").expect("command");
    assert!(!(next.enabled)(&state));
    assert!(!(previous.enabled)(&state));
    state.open_synthetic("One", "one");
    state.new_start_tab();
    assert!((next.enabled)(&state));
    assert!((previous.enabled)(&state));
}

#[test]
fn ctrl_tab_and_ctrl_shift_tab_are_the_stepping_chords() {
    use rune::{Key, KeyEvent, Mods};
    let plain = KeyEvent { key: Key::Command('v'), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } };
    assert_eq!(crate::commands::command_for_key(&plain).map(|c| c.id), Some("file.paste"));

    let next = KeyEvent { key: Key::Tab, mods: Mods { cmd: true, ctrl: true, ..Mods::default() } };
    assert_eq!(crate::commands::command_for_key(&next).map(|c| c.id), Some("tab.next"));
    let previous = KeyEvent { key: Key::Tab, mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } };
    assert_eq!(crate::commands::command_for_key(&previous).map(|c| c.id), Some("tab.previous"));
    let reopen =
        KeyEvent { key: Key::Command('t'), mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } };
    assert_eq!(crate::commands::command_for_key(&reopen).map(|c| c.id), Some("tab.reopen"));
}

// ---- themes / reading settings / help / browser up (C-11) ----------------

#[test]
fn theme_commands_pick_each_theme_and_check_the_active_one() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = Tx::default();
    let cases = [
        ("theme.system", ThemePref::System),
        ("theme.light", ThemePref::Light),
        ("theme.sepia", ThemePref::Sepia),
        ("theme.dark", ThemePref::Dark),
        ("theme.black", ThemePref::Black),
    ];
    for (id, theme) in cases {
        assert!(state.run_command(id, &mut tx), "{id} runs");
        assert_eq!(state.settings_theme(), theme);
        for (other_id, other) in cases {
            let checked =
                (crate::commands::lookup_command(other_id).expect("command").checked.expect("checked"))(&state);
            assert_eq!(checked, other == theme, "{other_id} checked state for {theme:?}");
        }
    }
    assert!(state.run_command("app.theme-cycle", &mut tx), "the cycle command stays");
    assert_eq!(state.settings_theme(), ThemePref::System, "black cycles back to system");
}

#[test]
fn theme_command_titles_name_the_theme() {
    let temp = TempDir::new().unwrap();
    let state = window(&temp);
    let title = crate::commands::command_title(&state, crate::commands::lookup_command("theme.sepia").expect("cmd"));
    assert!(title.contains(crate::settings::theme_label(ThemePref::Sepia)), "{title}");
    let help = crate::commands::command_title(&state, crate::commands::lookup_command("app.help").expect("cmd"));
    assert_eq!(help, "Help", "the shortcut in parentheses is cut");
}

#[test]
fn reading_settings_opens_the_dialog_on_the_reader_section() {
    let temp = TempDir::new().unwrap();
    let mut runtime = Runtime::new(window(&temp), crate::view::view, (1280.0, 900.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(!state.settings_is_open());
        assert!(state.run_command("app.reading-settings", tx));
        assert!(state.settings_is_open());
    });
    // In a live frame the reader size stepper takes the keyboard.
    runtime.frame(0.0);
    let focused = runtime.elements().iter().find(|element| element.focused).and_then(|element| element.key.clone());
    assert_eq!(focused.as_deref(), Some("settings-size-value"));
}

#[test]
fn app_help_opens_the_help_dialog() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = Tx::default();
    assert!(state.run_command("app.help", &mut tx));
    assert_eq!(state.help_view(), Some(crate::window::HelpView::Full), "the full guide opens");
}

#[test]
fn browser_up_moves_the_root_to_its_parent() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    state.set_browser_root(root.clone());
    let command = crate::commands::lookup_command("browser.up").expect("command");
    assert!((command.enabled)(&state));
    let mut tx = Tx::default();
    assert!(state.run_command("browser.up", &mut tx));
    assert_eq!(state.browser.root.as_deref(), Some(temp.path()));
    state.browser.root = Some(std::path::PathBuf::from("/"));
    assert!(!(command.enabled)(&state), "a filesystem root has nowhere to go");
}

// ---- closed tabs (T-07) --------------------------------------------------

#[test]
fn closing_a_document_remembers_its_place_but_a_start_page_never() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "one");
    let second = state.open_synthetic("Two", "two");
    state.open_synthetic("Three", "three");
    assert!(!state.can_reopen_tab());
    assert!(!(crate::commands::lookup_command("tab.reopen").expect("cmd").enabled)(&state));

    state.activate_index(1);
    state.tabs[1].view.anchor = anchor("h-1", 24.0);
    state.close_tab(second);
    let closed = state.closed_tabs();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].index(), 1);
    assert_eq!(closed[0].path(), None);
    assert!(matches!(&closed[0], ClosedTab::Pasted { title, text, .. } if title == "Two" && text == "two"));
    assert!(state.can_reopen_tab());

    let start = state.new_start_tab();
    state.close_tab(start);
    assert_eq!(state.closed_tabs().len(), 1, "start pages are never remembered");
    assert_eq!(state.tabs().len(), 2, "closing the extra start page left the documents");
}

#[test]
fn the_closed_stack_stops_at_the_original_cap() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    for index in 0..MAX_CLOSED_TABS + 5 {
        let tab = state.open_synthetic(&format!("Note {index}"), "text");
        state.close_tab(tab);
    }
    assert_eq!(state.closed_tabs().len(), MAX_CLOSED_TABS);
    let oldest = &state.closed_tabs()[0];
    assert!(
        matches!(oldest, ClosedTab::Pasted { title, .. } if title == &format!("Note {}", 5)),
        "the oldest five went"
    );
}

#[test]
fn reopen_restores_a_file_tab_at_its_index_and_place() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let path = temp.path().join("notes.md");
    std::fs::write(&path, "# Notes\n").unwrap();
    let document = ready_file(state.shared(), &path, "# Notes\n");
    let tab = state.attach_document(document);
    state.open_synthetic("Second", "second");
    state.open_synthetic("Third", "third");
    state.tabs[0].view.anchor = anchor("heading-2", 40.0);
    assert_eq!(state.close_tab(tab), crate::window::CloseOutcome::Closed);
    assert_eq!(state.closed_tabs()[0].path(), Some(path.as_path()));

    assert!(state.reopen_tab());
    assert_eq!(state.active_index(), 0, "back at its strip index");
    assert_eq!(state.tabs().len(), 3);
    assert_eq!(state.active_tab().map(|tab| tab.view.anchor.clone()), Some(anchor("heading-2", 40.0)));
    let reopened = state.active_tab().and_then(Tab::document).expect("document");
    assert_eq!(reopened.key.borrow().path(), Some(path.as_path()));
}

#[test]
fn the_closed_stack_is_per_window() {
    let temp = TempDir::new().unwrap();
    let shared = shared(&temp);
    let mut first = WindowState::new(Rc::clone(&shared));
    let mut second = WindowState::new(Rc::clone(&shared));
    let tab = first.open_synthetic("Only", "only");
    first.close_tab(tab);
    assert!(first.can_reopen_tab());
    assert!(!second.can_reopen_tab(), "another window has its own history");
    assert!(!second.reopen_tab());
}

#[test]
fn reopening_after_dont_save_restores_the_text_as_pasted() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Pasted notes", "original text\n");
    let document = state.tab(tab).and_then(Tab::document).cloned().expect("doc");
    dirty(&document);
    state.sync_tab_dirty(&document);
    state.request_close_tab(tab);
    assert!(matches!(state.prompt(), Some(Prompt::CloseTab { tab: asked, .. }) if *asked == tab));
    let mut tx = Tx::default();
    state.prompt_secondary(&mut tx);
    assert!(state.tab(tab).is_none(), "Don't save closed the tab");

    assert!(state.reopen_tab());
    let reopened = state.active_tab().and_then(Tab::document).expect("document");
    assert_eq!(reopened.text().text(), "original text\n", "the discarded edits never come back through reopen");
    assert!(matches!(*reopened.key.borrow(), DocumentKey::Untitled(_)));
}

#[test]
fn reopen_reuses_a_document_another_window_still_shows() {
    let temp = TempDir::new().unwrap();
    let shared = shared(&temp);
    let mut first = WindowState::new(Rc::clone(&shared));
    let mut second = WindowState::new(Rc::clone(&shared));
    let path = temp.path().join("shared.md");
    let document = ready_file(&shared, &path, "# Shared\n");
    let first_tab = first.attach_document(Rc::clone(&document));
    second.attach_document(Rc::clone(&document));
    first.close_tab(first_tab);
    assert!(first.reopen_tab());
    let reopened = first.active_tab().and_then(Tab::document).expect("document");
    assert!(Rc::ptr_eq(reopened, &document), "the registry kept the live document");
    assert_eq!(shared.docs().get(document.id).map(|doc| doc.id), Some(document.id));
}

#[test]
fn reopen_restores_pasted_text_with_its_paste_id_and_place() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("First", "first");
    let tab = state.open_synthetic("Pasted notes", "# Pasted notes\n\nBody.\n");
    state.open_synthetic("Third", "third");
    let paste = state.tab(tab).and_then(Tab::document).and_then(|doc| match &*doc.key.borrow() {
        DocumentKey::Untitled(paste) => Some(*paste),
        DocumentKey::File(_) | DocumentKey::Start => None,
    });
    assert!(paste.is_some());
    if let Some(tab) = state.tabs.iter_mut().find(|candidate| candidate.id == tab) {
        tab.view.anchor = anchor("pasted-heading", 8.0);
    }
    state.close_tab(tab);
    let mut tx = Tx::default();
    assert!(state.run_command("tab.reopen", &mut tx));
    assert_eq!(state.active_index(), 1);
    let reopened = state.active_tab().expect("tab");
    assert_eq!(reopened.full_title(), "Pasted notes");
    assert_eq!(reopened.view.anchor, anchor("pasted-heading", 8.0));
    let document = reopened.document().expect("document");
    assert_eq!(document.text().text(), "# Pasted notes\n\nBody.\n");
    assert_eq!(document.key.borrow().clone(), DocumentKey::Untitled(paste.expect("paste id")));
}

// ---- close others / right (T-23) -----------------------------------------

fn dirty(document: &Document) {
    let len = document.text().text().len();
    document.apply(0..len, "changed");
}

#[test]
fn close_other_tabs_keeps_only_the_chosen_tab() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "one");
    let keep = state.open_synthetic("Two", "two");
    state.open_synthetic("Three", "three");
    state.open_synthetic("Four", "four");

    assert!(state.close_other_tabs(keep));
    assert_eq!(state.tabs().len(), 1);
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(keep));
    assert_eq!(state.closed_tabs().len(), 3);
}

#[test]
fn close_tabs_to_the_right_closes_from_the_right() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "one");
    let second = state.open_synthetic("Two", "two");
    let third = state.open_synthetic("Three", "three");
    let fourth = state.open_synthetic("Four", "four");

    assert!(state.close_tabs_to_the_right(second));
    assert_eq!(state.tabs().len(), 2);
    let closed: Vec<usize> = state.closed_tabs().iter().map(ClosedTab::index).collect();
    assert_eq!(closed, vec![3, 2], "right to left (T-23)");
    assert!(state.tab(third).is_none() && state.tab(fourth).is_none());
    assert!(!state.close_tabs_to_the_right(second), "nothing further right");
}

#[test]
fn a_dirty_tab_in_the_sequence_asks_and_cancel_stops_the_run() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let keep = state.open_synthetic("Keep", "keep");
    let dirty_tab = state.open_synthetic("Dirty", "dirty");
    let last = state.open_synthetic("Last", "last");
    let document = state.tab(dirty_tab).and_then(Tab::document).cloned().expect("doc");
    dirty(&document);
    state.sync_tab_dirty(&document);

    assert!(state.close_other_tabs(keep));
    assert!(matches!(state.prompt(), Some(Prompt::CloseTab { tab, .. }) if *tab == dirty_tab));
    assert_eq!(state.tabs().len(), 3, "nothing closed while the question is open");
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(dirty_tab), "the asked tab is on screen");

    state.prompt_cancel();
    poll(&mut state);
    assert_eq!(state.tabs().len(), 3, "Cancel leaves the rest open");
    assert!(state.tab(last).is_some());
    assert_eq!(state.closed_tabs().len(), 0);
}

#[test]
fn dont_save_continues_the_sequence() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let keep = state.open_synthetic("Keep", "keep");
    let dirty_tab = state.open_synthetic("Dirty", "dirty");
    state.open_synthetic("Last", "last");
    let document = state.tab(dirty_tab).and_then(Tab::document).cloned().expect("doc");
    dirty(&document);
    state.sync_tab_dirty(&document);

    assert!(state.close_other_tabs(keep));
    assert!(matches!(state.prompt(), Some(Prompt::CloseTab { tab, .. }) if *tab == dirty_tab));
    let mut tx = Tx::default();
    state.prompt_secondary(&mut tx);
    poll(&mut state);
    assert_eq!(state.tabs().len(), 1, "the run finished after Don't save");
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(keep));
    assert_eq!(state.closed_tabs().len(), 2);
}

#[test]
fn a_dirty_tab_to_the_right_asks_before_the_clean_ones_close() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "first");
    let dirty_tab = state.open_synthetic("Dirty", "dirty");
    let last = state.open_synthetic("Last", "last");
    let document = state.tab(dirty_tab).and_then(Tab::document).cloned().expect("doc");
    dirty(&document);
    state.sync_tab_dirty(&document);

    assert!(state.close_tabs_to_the_right(first));
    // Right to left: the last (clean) tab closes first, then the dirty one asks.
    assert!(state.tab(last).is_none());
    assert!(matches!(state.prompt(), Some(Prompt::CloseTab { tab, .. }) if *tab == dirty_tab));
    assert_eq!(state.tabs().len(), 2);
    state.prompt_cancel();
    poll(&mut state);
    assert_eq!(state.tabs().len(), 2, "the dirty tab and the kept tab stay");
}

// ---- palette coverage (C-11) ---------------------------------------------

#[test]
fn the_palette_lists_the_new_commands() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let path = temp.path().join("notes.md");
    let document = ready_file(state.shared(), &path, "# Notes\n");
    state.attach_document(document);
    state.open_synthetic("Two", "two");
    let closed = state.open_synthetic("Closed", "closed");
    state.close_tab(closed);
    state.activate_index(0);
    state.browser.root = Some(temp.path().join("docs"));
    let mut tx = Tx::default();
    state.open_palette(Some(PaletteChord::CtrlK), &mut tx);
    assert!(state.palette_is_open());

    let ids: Vec<String> =
        state.palette.commands.iter().map(|entry| entry.key.trim_start_matches("c:").to_owned()).collect();
    for wanted in [
        "file.paste",
        "file.copy-path",
        "file.reveal",
        "view.pin",
        "tab.next",
        "tab.previous",
        "tab.reopen",
        "theme.system",
        "theme.light",
        "theme.sepia",
        "theme.dark",
        "theme.black",
        "app.reading-settings",
        "app.help",
        "browser.up",
    ] {
        assert!(ids.iter().any(|candidate| candidate == wanted), "{wanted} is missing from the palette: {ids:?}");
    }
}

#[test]
fn the_start_group_leads_with_open_and_paste() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = Tx::default();
    state.open_palette(Some(PaletteChord::CtrlK), &mut tx);
    let group = state.palette.view.groups.first().expect("a commands group");
    let first: Vec<String> =
        group.rows().filter_map(|index| state.palette.entry(index)).map(|entry| entry.key.clone()).collect();
    assert!(first.contains(&"c:file.paste".to_owned()), "{first:?}");
    assert!(first.contains(&"c:app.help".to_owned()), "{first:?}");
}

#[test]
fn the_new_commands_have_titles_and_categories() {
    use crate::commands::CommandCategory;
    for (id, title, category) in [
        ("file.paste", "Paste as a new document", CommandCategory::File),
        ("file.copy-path", "Copy path", CommandCategory::File),
        ("file.reveal", "Show in folder", CommandCategory::File),
        ("view.pin", "Keep on top", CommandCategory::View),
        ("browser.up", "Go to parent folder", CommandCategory::View),
        ("tab.reopen", "Reopen closed tab", CommandCategory::Tab),
        ("tab.next", "Next tab", CommandCategory::Tab),
        ("tab.previous", "Previous tab", CommandCategory::Tab),
        ("app.reading-settings", "Reading settings", CommandCategory::Settings),
        ("app.help", "Help", CommandCategory::Help),
    ] {
        let command = crate::commands::lookup_command(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(command.title, title, "{id}");
        assert_eq!(command.category, category, "{id}");
        assert!(!command.keywords.is_empty(), "{id} is searchable");
    }
}

#[test]
fn tab_menu_opens_on_a_background_tab() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("One", "one");
    state.open_synthetic("Two", "two");
    state.show_tab_menu(tab, rune::PointerPress { x: 20.0, y: 20.0 });
    assert_eq!(state.tab_menu().map(|menu| menu.tab), Some(tab));
    assert!(state.tab(tab).is_some(), "a background tab's menu never forces it forward");
    state.close_tab_menu();
}
