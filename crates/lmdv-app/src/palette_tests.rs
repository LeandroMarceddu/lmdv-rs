//! Command palette integration tests (C-01–C-13, P-10).

use std::path::PathBuf;

use rune::{Env, Key, KeyEvent, Mods, PointerButton, Runtime, Tx};
use tempfile::TempDir;

use crate::browser::BrowserRow;
use crate::palette::{
    COMMANDS_CAP, FILES_CAP, HEADINGS_CAP, OUTLINE_CAP, PALETTE_INPUT_KEY, PALETTE_LIST_KEY, PALETTE_OPTION_PREFIX,
    PALETTE_STATUS_KEY, PaletteChord, PaletteEntry, PaletteMode, PaletteWhere, START_FILES_CAP, START_TABS_CAP,
    TABS_CAP,
};
use crate::test_support;
use crate::window::{Mode, Tab, WindowState};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn ctrl(letter: char) -> KeyEvent {
    KeyEvent { key: Key::Command(letter), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } }
}

fn ctrl_shift(letter: char) -> KeyEvent {
    KeyEvent { key: Key::Command(letter), mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } }
}

fn key_event(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

/// Opens the palette and types `query`, as the keys do.
fn opened(state: &mut WindowState, query: &str) -> Tx {
    let mut tx = Tx::default();
    state.open_palette(Some(PaletteChord::CtrlK), &mut tx);
    state.set_palette_query(query, &mut tx);
    tx
}

/// A document with headings, the palette's tab and heading groups.
fn with_notes(state: &mut WindowState) {
    state.open_synthetic("Reading notes", "# Reading notes\n\n## Install\n\n### Windows\n\n## Attività\n\nBody.\n");
}

/// A browser root with the given files, as the tree would list them.
fn with_files(state: &mut WindowState, root: &str, files: &[(&str, &str)]) {
    state.browser.root = Some(PathBuf::from(root));
    state.browser.root_name = "docs".to_owned();
    state.browser.rows = files
        .iter()
        .map(|(path, name)| BrowserRow {
            path: (*path).to_owned(),
            name: (*name).to_owned(),
            depth: 0,
            dir: false,
            expanded: false,
        })
        .collect();
}

fn group_rows<'a>(state: &'a WindowState, title: &str) -> Vec<&'a PaletteEntry> {
    let Some(group) = state.palette.view.groups.iter().find(|group| group.title == title) else { return Vec::new() };
    group.rows().filter_map(|index| state.palette.entry(index)).collect()
}

fn group_labels(state: &WindowState, title: &str) -> Vec<String> {
    group_rows(state, title).into_iter().map(|entry| entry.label.clone()).collect()
}

fn group_titles(state: &WindowState) -> Vec<String> {
    state.palette.view.groups.iter().map(|group| group.title.clone()).collect()
}

/// The row whose entry key is `key`.
fn row_of(state: &WindowState, key: &str) -> usize {
    (0..state.palette.len())
        .find(|index| state.palette.entry(*index).is_some_and(|entry| entry.key == key))
        .unwrap_or_else(|| {
            panic!(
                "no row {key} in {:?}",
                (0..state.palette.len()).map(|i| state.palette.entry(i).map(|e| e.key.clone())).collect::<Vec<_>>()
            )
        })
}

fn file_row(state: &WindowState, path: &str) -> usize {
    row_of(state, &format!("f:{}", path.to_lowercase()))
}

fn pick(state: &mut WindowState, index: usize, where_: PaletteWhere) -> Tx {
    let mut tx = Tx::default();
    state.palette.active = Some(index);
    state.palette_pick(index, where_, &mut tx);
    tx
}

// ---- open and close (C-01) ------------------------------------------------

#[test]
fn ctrl_k_opens_and_closes_the_palette_while_reading() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = Tx::default();

    assert!(crate::app::on_key(&mut state, &ctrl('k'), &mut tx), "the key is the palette's");
    assert!(state.palette_is_open(), "Ctrl+K opens it while reading");
    assert_eq!(state.palette.chord, Some(PaletteChord::CtrlK));

    assert!(crate::app::on_key(&mut state, &ctrl('k'), &mut tx));
    assert!(!state.palette_is_open(), "a second press closes it");
}

#[test]
fn a_held_opening_key_does_not_close_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    // The auto-repeat of the opening key is the runtime's to flag.
    let mut tx = Tx::default();
    state.open_palette(Some(PaletteChord::CtrlK), &mut tx);
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.repeat_key(ctrl('k'));
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.palette_is_open(), "the held key's repeat leaves it open");
    runtime.key(ctrl('k'));
    runtime.frame(2.0 / 60.0);
    assert!(!runtime.state.palette_is_open(), "a fresh press closes it");
}

#[test]
fn the_editor_keeps_ctrl_k_for_its_link_command() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx));
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));

    assert!(crate::app::on_key(&mut state, &ctrl('k'), &mut tx), "the editor takes Ctrl+K");
    assert!(!state.palette_is_open(), "it is not the palette's key in the editor");
    assert_eq!(state.ui.last_command.as_deref(), Some("edit.link"), "Ctrl+K inserts a link while editing");
}

#[test]
fn ctrl_shift_p_opens_it_anywhere() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = Tx::default();

    assert!(crate::app::on_key(&mut state, &ctrl_shift('p'), &mut tx), "it opens on the start page");
    assert!(state.palette_is_open());
    assert_eq!(state.palette.chord, Some(PaletteChord::CtrlShiftP));
    assert!(crate::app::on_key(&mut state, &ctrl('k'), &mut tx), "Ctrl+K is held back, not a second opening");
    assert!(state.palette_is_open(), "only the opening chord closes it again");

    let mut tx = Tx::default();
    crate::app::on_key(&mut state, &ctrl_shift('p'), &mut tx);
    assert!(!state.palette_is_open(), "its own second press closes it");

    let mut tx = Tx::default();
    with_notes(&mut state);
    assert!(state.enter_editor(None, &mut tx));
    assert!(crate::app::on_key(&mut state, &ctrl_shift('p'), &mut tx), "it opens while editing too");
    assert!(state.palette_is_open());
}

#[test]
fn alt_meta_and_shift_chords_do_not_open_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = Tx::default();
    let altgr = KeyEvent { key: Key::Command('k'), mods: Mods { cmd: true, ctrl: true, alt: true, ..Mods::default() } };
    let meta = KeyEvent { key: Key::Command('k'), mods: Mods { cmd: true, ctrl: true, logo: true, ..Mods::default() } };
    let shifted =
        KeyEvent { key: Key::Command('k'), mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } };
    assert!(!crate::app::on_key(&mut state, &altgr, &mut tx), "AltGr types, it does not open the palette");
    assert!(!crate::app::on_key(&mut state, &meta, &mut tx));
    assert!(!state.palette_is_open());
    crate::app::on_key(&mut state, &shifted, &mut tx);
    assert!(!state.palette_is_open(), "Ctrl+Shift+K is not the palette");
}

#[test]
fn escape_and_the_backdrop_close_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = opened(&mut state, "");
    assert!(crate::app::on_key(&mut state, &key_event(Key::Escape), &mut tx));
    assert!(!state.palette_is_open(), "Escape closes it");

    let mut state = window(&temp);
    with_notes(&mut state);
    state.open_palette(None, &mut Tx::default());
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    runtime.pointer_down(6.0, 740.0);
    runtime.pointer_up(6.0, 740.0);
    runtime.frame(2.0 / 60.0);
    assert!(!runtime.state.palette_is_open(), "a press and release on the backdrop closes it");
}

// ---- contents and caps (C-02) --------------------------------------------

#[test]
fn the_start_list_shows_commands_tabs_and_recent_files_with_their_caps() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    // Seven more documents: the front one is left out and the rest fill the cap.
    for index in 0..7 {
        state.open_synthetic(&format!("Second {index}"), &format!("# Second {index}\n"));
    }
    with_files(&mut state, "/docs", &[("/docs/a.md", "a.md"), ("/docs/b.md", "b.md")]);
    state.shared().record_recent(PathBuf::from("/docs/one.md"), 10);
    state.shared().record_recent(PathBuf::from("/docs/two.md"), 20);
    for index in 0..8 {
        state.shared().record_recent(PathBuf::from(format!("/docs/r{index}.md")), 30 + index);
    }

    let _tx = opened(&mut state, "");
    assert_eq!(state.palette.view.mode, PaletteMode::Start);
    assert_eq!(group_titles(&state), ["Commands", "Open tabs", "Files"]);
    assert!(group_rows(&state, "Commands").len() <= 6);
    assert_eq!(group_rows(&state, "Open tabs").len(), START_TABS_CAP);
    assert_eq!(group_rows(&state, "Files").len(), START_FILES_CAP);
    assert_eq!(group_labels(&state, "Files")[0], "r7.md", "the latest recent comes first");
}

#[test]
fn typing_caps_every_group() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    for index in 0..8 {
        state.new_start_tab();
        state.open_synthetic(&format!("Notes {index}"), &format!("# Notes {index}\n"));
    }
    // Twelve distinct files, all matching "notes".
    state.browser.root = Some(PathBuf::from("/docs"));
    state.browser.root_name = "docs".to_owned();
    state.browser.rows = (0..12)
        .map(|index| BrowserRow {
            path: format!("/docs/notes-{index}.md"),
            name: format!("notes-{index}.md"),
            depth: 0,
            dir: false,
            expanded: false,
        })
        .collect();

    let _tx = opened(&mut state, "notes");
    assert_eq!(state.palette.view.mode, PaletteMode::All);
    assert!(group_rows(&state, "Commands").len() <= COMMANDS_CAP);
    assert!(group_rows(&state, "In this document").len() <= HEADINGS_CAP);
    assert!(group_rows(&state, "Open tabs").len() <= TABS_CAP);
    assert_eq!(group_rows(&state, "Files").len(), FILES_CAP);
}

#[test]
fn the_front_tab_and_open_files_are_left_out() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("First", "# First\n");
    state.open_synthetic("Second", "# Second\n");
    // The open file becomes the front tab; the two documents were shown before it.
    state.open_path(PathBuf::from("/docs/open.md"));
    with_files(&mut state, "/docs", &[("/docs/first.md", "first.md"), ("/docs/other.md", "other.md")]);
    assert_eq!(state.tabs().len(), 3);

    let _tx = opened(&mut state, "");
    assert_eq!(
        group_labels(&state, "Open tabs"),
        vec!["Second".to_owned(), "First".to_owned()],
        "the front tab is left out and the others are most recently shown first"
    );

    let mut tx = Tx::default();
    state.set_palette_query("md", &mut tx);
    let files = group_labels(&state, "Files");
    assert!(files.contains(&"other.md".to_owned()), "{files:?}");
    assert!(!files.iter().any(|name| name == "open.md"), "a file open in a tab is left out: {files:?}");
}

#[test]
fn headings_carry_the_sections_they_are_in() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, "");
    // `scene` outline mode is the one that shows the whole tree.
    let mut tx = Tx::default();
    state.set_palette_query("#", &mut tx);
    assert_eq!(state.palette.view.mode, PaletteMode::Outline);
    let rows = group_rows(&state, "In this document");
    let windows = rows.iter().find(|entry| entry.label == "Windows").expect("the h3");
    assert_eq!(windows.detail, "Reading notes › Install");
    assert_eq!(windows.depth, 2, "h3 under h1 is two levels in");
    assert!(rows.iter().find(|entry| entry.label == "Install").is_some_and(|entry| entry.depth == 1));
}

#[test]
fn only_commands_that_can_run_now_are_listed() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = Tx::default();
    state.open_palette(None, &mut tx);
    state.set_palette_query(">", &mut tx);
    let labels: Vec<String> = state.palette.commands.iter().map(|entry| entry.label.clone()).collect();
    assert!(!labels.iter().any(|label| label == "Save"), "the start page has nothing to save: {labels:?}");
    assert!(!labels.iter().any(|label| label == "Edit document"), "and no document to edit");
    assert!(!labels.iter().any(|label| label == "Bold"), "no editor commands while reading");
    assert!(labels.iter().any(|label| label == "New tab"), "what can run is listed");

    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx));
    state.open_palette(None, &mut tx);
    state.set_palette_query("bold", &mut tx);
    let bold = state.palette.commands.iter().find(|entry| entry.label == "Bold").expect("Bold is listed while editing");
    assert_eq!(bold.tag, "Editor");
    assert_eq!(bold.checked, Some(false), "the setting shows its state");
    let labels: Vec<String> = state.palette.commands.iter().map(|entry| entry.label.clone()).collect();
    assert!(labels.iter().any(|label| label == "Find & replace"), "the editor's own find: {labels:?}");
}

// ---- matching and prefix modes (C-03/C-04) -------------------------------

#[test]
fn accents_and_letter_order_find_a_heading() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, "attivita");
    assert!(group_labels(&state, "In this document").contains(&"Attività".to_owned()), "the accent is no difference");

    let mut tx = Tx::default();
    state.set_palette_query("wndws", &mut tx);
    assert!(group_labels(&state, "In this document").contains(&"Windows".to_owned()), "letters in order find it");
}

#[test]
fn greater_than_lists_only_commands_and_all_of_them() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, ">");
    assert_eq!(state.palette.view.mode, PaletteMode::Commands);
    assert_eq!(group_titles(&state), ["Commands"]);
    assert_eq!(group_rows(&state, "Commands").len(), state.palette.commands.len(), "no cap for the prefix");
    assert!(state.palette.commands.len() > COMMANDS_CAP);
}

#[test]
fn hash_lists_the_outline_in_order_and_caps_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut text = String::from("# Many\n\n");
    for index in 0..300 {
        text.push_str(&format!("## Section {index}\n\nBody {index}.\n\n"));
    }
    state.open_synthetic("Many", &text);
    let _tx = opened(&mut state, "#");
    assert_eq!(state.palette.view.mode, PaletteMode::Outline);
    let rows = group_rows(&state, "In this document");
    assert_eq!(rows.len(), OUTLINE_CAP);
    assert_eq!(rows[0].label, "Many", "document order starts at the top");

    // Centred on the heading being read: the rows around it, not the first ones.
    let mut tx = Tx::default();
    state.set_palette_current_heading(Some(250), &mut tx);
    let rows = group_rows(&state, "In this document");
    let current = rows.iter().position(|entry| entry.current);
    assert!(current.is_some_and(|at| at > 20 && at < OUTLINE_CAP - 20), "the current heading sits in the middle");
    assert!(state.palette.active.is_some());
    assert!(rows[state.palette.active.unwrap()].current, "the outline highlights the heading being read");
}

#[test]
fn hash_with_words_ranks_headings_only() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, "#win");
    assert_eq!(state.palette.view.mode, PaletteMode::Headings);
    assert_eq!(group_titles(&state), ["In this document"]);
    assert_eq!(group_labels(&state, "In this document")[0], "Windows");
}

#[test]
fn the_english_words_find_a_command() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, "print");
    assert!(
        group_labels(&state, "Commands").iter().any(|label| label == "Print…"),
        "the command's keywords find it: {:?}",
        group_labels(&state, "Commands")
    );
}

// ---- ranking and recents (C-05/P-10) -------------------------------------

#[test]
fn a_command_run_from_the_palette_is_remembered_first() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, ">toggle outline");
    let index = row_of(&state, "c:view.toggle-outline");
    let mut tx = Tx::default();
    state.palette_pick(index, PaletteWhere::Here, &mut tx);
    assert_eq!(state.palette.last_pick, Some(crate::palette::EntryAction::Command("view.toggle-outline")));
    assert_eq!(state.shared().ui().palette_recent.first().map(String::as_str), Some("view.toggle-outline"));

    let mut tx = Tx::default();
    state.open_palette(None, &mut tx);
    assert_eq!(group_labels(&state, "Commands")[0], "Toggle outline", "the recent command comes first");
}

#[test]
fn a_better_score_beats_a_recent_command() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut ui = state.shared().ui().clone();
    ui.palette_recent = vec!["file.quit".to_owned()];
    state.shared().set_ui(ui);

    let _tx = opened(&mut state, "toggle outline");
    let labels = group_labels(&state, "Commands");
    assert_eq!(labels[0], "Toggle outline", "the exact match wins over the recent list: {labels:?}");
}

#[test]
fn the_recent_list_stays_at_eight_and_never_repeats() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    for (query, id) in [("keys", "help.keys"), ("new tab", "file.new-tab"), ("keys", "help.keys")] {
        let _tx = opened(&mut state, query);
        let index = row_of(&state, &format!("c:{id}"));
        state.palette_pick(index, PaletteWhere::Here, &mut Tx::default());
    }
    let recent = state.shared().ui().palette_recent.clone();
    assert_eq!(recent, vec!["help.keys".to_owned(), "file.new-tab".to_owned()], "one entry per command, newest first");
    assert!(recent.len() <= crate::palette::RECENT_MAX);
}

// ---- activation (C-06) ---------------------------------------------------

#[test]
fn enter_opens_a_file_here() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    with_files(&mut state, "/docs", &[("/docs/other.md", "other.md")]);
    opened(&mut state, "other");
    let row = file_row(&state, "/docs/other.md");
    let _tx = pick(&mut state, row, PaletteWhere::Here);
    assert!(!state.palette_is_open());
    assert_eq!(state.active_tab().map(Tab::full_title).as_deref(), Some("other.md"), "the file opened in front");
}

#[test]
fn ctrl_enter_leaves_the_tab_behind_in_front() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    with_files(&mut state, "/docs", &[("/docs/other.md", "other.md")]);
    let front = state.active_tab().map(|tab| tab.id);
    opened(&mut state, "other");
    let row = file_row(&state, "/docs/other.md");
    let _tx = pick(&mut state, row, PaletteWhere::BackgroundTab);
    assert_eq!(state.active_tab().map(|tab| tab.id), front, "the tab behind does not come forward");
    assert_eq!(state.tabs().len(), 2);
    assert_eq!(state.tabs()[1].full_title(), "other.md");
}

#[test]
fn ctrl_shift_enter_opens_the_file_in_front() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    with_files(&mut state, "/docs", &[("/docs/other.md", "other.md")]);
    opened(&mut state, "other");
    let row = file_row(&state, "/docs/other.md");
    let _tx = pick(&mut state, row, PaletteWhere::ForegroundTab);
    assert_eq!(state.active_tab().map(Tab::full_title).as_deref(), Some("other.md"));
}

#[test]
fn shift_enter_asks_for_a_new_window() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    with_files(&mut state, "/docs", &[("/docs/other.md", "other.md")]);
    opened(&mut state, "other");
    let row = file_row(&state, "/docs/other.md");
    let _tx = pick(&mut state, row, PaletteWhere::NewWindow);
    assert_eq!(state.tabs().len(), 1, "the file did not open here");
    assert!(
        state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("new window")),
        "a new window needs the host's opener"
    );
}

#[test]
fn commands_ignore_the_modifiers_they_were_chosen_with() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    opened(&mut state, ">new tab");
    let row = row_of(&state, "c:file.new-tab");
    let _tx = pick(&mut state, row, PaletteWhere::BackgroundTab);
    assert_eq!(state.tabs().len(), 2, "the command ran");
    assert!(state.active_tab().is_some_and(Tab::is_start), "and brought its tab forward");
}

#[test]
fn choosing_a_tab_brings_it_forward() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "# First\n");
    state.open_synthetic("Second", "# Second\n");
    opened(&mut state, "");
    let row = row_of(&state, &format!("t:{first}"));
    let _tx = pick(&mut state, row, PaletteWhere::Here);
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(first));
}

#[test]
fn choosing_a_heading_jumps_like_the_outline() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_reader(&mut state);
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.update(|state, tx| {
        state.open_palette(None, tx);
        state.set_palette_query("#", tx);
    });
    runtime.frame(1.0 / 60.0);
    let last = runtime.state.palette.len() - 1;
    assert!(last > 0, "the outline lists the headings");
    let mut action = None;
    runtime.update(|state, tx| {
        action = state.palette.entry(last).map(|entry| entry.action.clone());
        state.palette_pick(last, PaletteWhere::Here, tx);
    });
    assert!(matches!(action, Some(crate::palette::EntryAction::Heading(_))));
    for step in 2..12 {
        runtime.frame(f64::from(step) / 60.0);
    }
    assert!(runtime.state.reading.scroll.get().offset > 0.0, "the article scrolled to the heading");
}

// ---- the search rows (C-07) ----------------------------------------------

#[test]
fn the_search_rows_come_after_the_normal_groups() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    with_files(&mut state, "/docs", &[("/docs/other.md", "other.md")]);
    let _tx = opened(&mut state, "read");
    let titles = group_titles(&state);
    assert_eq!(titles.last().map(String::as_str), Some("Search"));
    let rows = group_rows(&state, "Search");
    assert_eq!(rows[0].label, "Find “read” in the document");
    assert_eq!(rows[1].label, "Search the folder for “read”");
}

#[test]
fn the_search_rows_need_a_document_and_a_folder() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let _tx = opened(&mut state, "read");
    assert!(!group_titles(&state).contains(&"Search".to_owned()), "no document on the start page");

    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, "read");
    let rows = group_rows(&state, "Search");
    assert_eq!(rows.len(), 1, "a document but no folder root");
    assert_eq!(rows[0].kind, crate::palette::EntryKind::FindInDocument);
}

#[test]
fn find_in_the_document_opens_the_reading_bar_with_the_query() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    with_files(&mut state, "/docs", &[("/docs/other.md", "other.md")]);
    opened(&mut state, "body");
    let row = row_of(&state, "s:doc");
    let _tx = pick(&mut state, row, PaletteWhere::Here);
    let find = &state.active_tab().expect("tab").view.reader_find;
    assert!(find.open, "the reading find bar opened");
    assert_eq!(find.query.text, "body");
    assert!(!find.matches.is_empty(), "the query was found");
}

#[test]
fn search_the_folder_starts_the_pane_search() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    state.browser.root = Some(root.clone());
    state.browser.root_name = "fixtures".to_owned();
    opened(&mut state, "markdown");
    let row = row_of(&state, "s:folder");
    let _tx = pick(&mut state, row, PaletteWhere::Here);
    assert!(state.left.is_open(), "the pane shows");
    assert_eq!(state.browser.search_query, "markdown");
    assert!(state.browser.search_pending, "the search started");
}

// ---- navigation (C-08) ---------------------------------------------------

#[test]
fn up_and_down_wrap() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = opened(&mut state, "");
    let count = state.palette.len();
    assert!(count >= 3);
    state.palette.active = Some(0);
    assert!(state.palette_key(&key_event(Key::Up), &mut tx));
    assert_eq!(state.palette.active, Some(count - 1), "Up from the first wraps to the last");
    assert!(state.palette_key(&key_event(Key::Down), &mut tx));
    assert_eq!(state.palette.active, Some(0), "Down from the last wraps to the first");
}

#[test]
fn page_down_moves_to_the_next_group_and_page_up_back() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    state.new_start_tab();
    state.open_synthetic("Second", "# Second\n");
    with_files(&mut state, "/docs", &[("/docs/other.md", "other.md")]);
    state.shared().record_recent(PathBuf::from("/docs/recent.md"), 5);
    let mut tx = opened(&mut state, "");
    assert_eq!(group_titles(&state).len(), 3);
    state.palette.active = Some(0);
    assert!(state.palette_key(&key_event(Key::PageDown), &mut tx));
    let tabs_start = state.palette.view.groups[1].first;
    assert_eq!(state.palette.active, Some(tabs_start), "PageDown goes to the next group's first row");
    assert!(state.palette_key(&key_event(Key::PageDown), &mut tx));
    let files_start = state.palette.view.groups[2].first;
    assert_eq!(state.palette.active, Some(files_start), "the last group's first row");
    assert!(state.palette_key(&key_event(Key::PageUp), &mut tx));
    assert_eq!(state.palette.active, Some(tabs_start), "PageUp at a group's start goes back one");
    assert!(state.palette_key(&key_event(Key::PageUp), &mut tx));
    assert_eq!(state.palette.active, Some(0), "and then to the current group's start");
}

#[test]
fn tab_is_swallowed() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = opened(&mut state, "");
    assert!(state.palette_key(&key_event(Key::Tab), &mut tx), "Tab does not leave the palette");
}

#[test]
fn the_pointer_moves_the_highlight_only_when_it_moves() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, "");
    state.palette.active = Some(0);
    state.palette_pointer_moved(2, 40.0, 90.0);
    assert_eq!(state.palette.active, Some(2));
    state.palette_pointer_moved(1, 40.0, 90.0);
    assert_eq!(state.palette.active, Some(2), "a pointer at rest leaves the choice alone");
    state.palette_pointer_moved(1, 40.0, 128.0);
    assert_eq!(state.palette.active, Some(1));
}

// ---- the modal (C-09/C-10) ----------------------------------------------

#[test]
fn shortcuts_do_not_reach_the_window_behind() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = opened(&mut state, "");
    let tabs = state.tabs().len();
    assert!(crate::app::on_key(&mut state, &ctrl('p'), &mut tx), "Ctrl+P is trapped");
    assert!(state.palette_is_open());
    assert_eq!(state.ui.last_command, None, "nothing ran behind the dialog");
    assert!(crate::app::on_key(&mut state, &ctrl('w'), &mut tx), "Ctrl+W is trapped");
    assert_eq!(state.tabs().len(), tabs);
    assert!(state.palette_is_open());
    assert!(crate::app::on_key(&mut state, &ctrl('f'), &mut tx), "Ctrl+F is trapped");
    assert!(!state.active_tab().is_some_and(|tab| tab.view.reader_find.open));
}

#[test]
fn f1_closes_the_palette_and_opens_help() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = opened(&mut state, "");
    assert!(crate::app::on_key(&mut state, &key_event(Key::Function(1)), &mut tx));
    assert!(!state.palette_is_open());
    assert_eq!(state.help_view(), Some(crate::window::HelpView::Full), "the full guide opens after the palette closes");
}

#[test]
fn the_text_box_keeps_its_editing_keys() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = opened(&mut state, "abc");
    for event in [
        ctrl('a'),
        ctrl('c'),
        ctrl('v'),
        ctrl('x'),
        ctrl('z'),
        ctrl('y'),
        key_event(Key::Backspace),
        key_event(Key::Left),
        key_event(Key::Home),
        key_event(Key::Text("d".to_owned())),
    ] {
        assert!(!state.palette_key(&event, &mut tx), "{event:?} stays the field's");
        assert!(state.palette_is_open());
    }
}

#[test]
fn focus_returns_to_where_it_was() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.update(|state, tx| {
        state.open_palette(None, tx);
        state.set_palette_query("read", tx);
    });
    runtime.frame(1.0 / 60.0);
    let input = runtime.elements().into_iter().find(|element| element.key.as_deref() == Some(PALETTE_INPUT_KEY));
    assert!(input.is_some_and(|field| field.focused), "the search box takes the keyboard");
    runtime.update(|state, _| state.close_palette());
    runtime.frame(2.0 / 60.0);
    let focused = runtime.elements().into_iter().find(|element| element.focused);
    assert!(
        focused.is_none_or(|element| element.key.as_deref() != Some(PALETTE_INPUT_KEY)),
        "the box lets the keyboard go"
    );
}

#[test]
fn closing_returns_the_keyboard_to_the_control_it_left() {
    let temp = TempDir::new().unwrap();
    let state = window(&temp);
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.focus("new-tab", false));
    runtime.frame(1.0 / 60.0);
    assert_eq!(focused_key(&runtime), Some("new-tab".to_owned()), "the strip's + has the keyboard");

    runtime.update(|state, tx| state.open_palette(None, tx));
    runtime.frame(2.0 / 60.0);
    assert_eq!(focused_key(&runtime), Some(PALETTE_INPUT_KEY.to_owned()), "the palette took it");

    runtime.update(|state, _| state.close_palette());
    runtime.frame(3.0 / 60.0);
    runtime.frame(4.0 / 60.0);
    assert!(!runtime.state.palette_is_open());
    assert_eq!(focused_key(&runtime), Some("new-tab".to_owned()), "C-10: the control has it back");
}

fn focused_key(runtime: &Runtime<WindowState>) -> Option<String> {
    runtime.elements().into_iter().find(|element| element.focused).and_then(|element| element.key)
}

#[test]
fn closing_restores_the_page_selection() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Reading notes", "# Reading notes\n\nBody text here.\n");
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.select_text("text", 0..4));
    runtime.frame(1.0 / 60.0);
    let selected = runtime.elements().into_iter().find_map(|element| element.selected);
    assert_eq!(selected.as_deref(), Some("Body"), "the paragraph's first word is selected");

    runtime.update(|state, tx| state.open_palette(None, tx));
    runtime.frame(2.0 / 60.0);
    runtime.update(|state, _| state.close_palette());
    runtime.frame(3.0 / 60.0);
    runtime.frame(4.0 / 60.0);
    let selected = runtime.elements().into_iter().find_map(|element| element.selected);
    assert_eq!(selected.as_deref(), Some("Body"), "C-10: the page has its selection back");
}

// ---- the middle button on a row (C-06/OI-147) ----------------------------

fn middle_click(runtime: &mut Runtime<WindowState>, key: &str) {
    let (x, y) = runtime.find(key).unwrap_or_else(|| panic!("no row {key}"));
    runtime.button_down(x, y, PointerButton::Middle);
    runtime.button_up(x, y, PointerButton::Middle);
}

#[test]
fn a_middle_click_on_a_file_row_opens_it_behind() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    // A document in front, so the background open cannot replace a start page.
    state.open_synthetic("Reading notes", "# Reading notes\n\nBody.\n");
    let path = temp.path().join("notes.md");
    std::fs::write(&path, "# Notes\n").unwrap();
    with_files(
        &mut state,
        temp.path().to_str().expect("a UTF-8 temp path"),
        &[(path.to_str().expect("a UTF-8 path"), "notes.md")],
    );
    let _tx = opened(&mut state, "notes");
    let key = format!("f:{}", path.display().to_string().to_lowercase());
    let row = row_of(&state, &key);

    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    middle_click(&mut runtime, &format!("{PALETTE_OPTION_PREFIX}{row}"));
    runtime.frame(2.0 / 60.0);

    let state = &runtime.state;
    assert!(!state.palette_is_open(), "picking a row closes the palette");
    assert_eq!(state.tabs().len(), 2, "the file opened");
    assert_eq!(
        state.active_tab().map(|tab| tab.full_title()),
        Some("Reading notes".to_owned()),
        "the front tab stayed in front"
    );
    assert_eq!(state.tabs()[1].full_title(), "notes.md");
}

#[test]
fn a_middle_click_on_a_heading_row_does_nothing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, "#");
    let row = row_of(&state, "h:0");
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    middle_click(&mut runtime, &format!("{PALETTE_OPTION_PREFIX}{row}"));
    runtime.frame(2.0 / 60.0);
    let state = &runtime.state;
    assert!(state.palette_is_open(), "nothing was chosen");
    assert_eq!(state.tabs().len(), 1);
    assert_eq!(state.palette.view.mode, PaletteMode::Outline);
}

// ---- closing triggers (C-13) ---------------------------------------------

#[test]
fn a_tab_switch_or_a_document_change_closes_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "# First\n");
    state.open_synthetic("Second", "# Second\n");
    opened(&mut state, "read");
    let mut tx = Tx::default();
    assert!(state.activate_tab(first));
    state.poll(0.1, &mut tx);
    assert!(!state.palette_is_open(), "a tab switch closes the palette");

    let mut state = window(&temp);
    with_notes(&mut state);
    opened(&mut state, "read");
    state.open_synthetic("Other", "# Other\n");
    let mut tx = Tx::default();
    state.poll(0.1, &mut tx);
    assert!(!state.palette_is_open(), "a document change closes it");
}

#[test]
fn another_modal_closes_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    opened(&mut state, "read");
    state.share.menu_open = true;
    let mut tx = Tx::default();
    state.poll(0.1, &mut tx);
    assert!(!state.palette_is_open(), "the Share menu closes the palette");

    let mut state = window(&temp);
    with_notes(&mut state);
    opened(&mut state, "read");
    state.ui.help = Some(crate::window::HelpView::Shortcuts);
    let mut tx = Tx::default();
    state.poll(0.1, &mut tx);
    assert!(!state.palette_is_open(), "Help closes the palette");
}

// ---- accessibility (C-12) ------------------------------------------------

#[test]
fn the_input_and_list_carry_the_combobox_roles() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, "read");
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    let elements = runtime.elements();
    let input = elements.iter().find(|element| element.key.as_deref() == Some(PALETTE_INPUT_KEY)).expect("the field");
    assert_eq!(input.role, "combobox");
    assert_eq!(input.label.as_deref(), Some("Command palette"));
    assert_eq!(input.expanded, Some(!runtime.state.palette.is_empty()));
    let list = elements.iter().find(|element| element.key.as_deref() == Some(PALETTE_LIST_KEY)).expect("the list");
    assert_eq!(list.role, "listbox");
    assert_eq!(list.label.as_deref(), Some("Results"));
    let options: Vec<_> = elements.iter().filter(|element| element.role == "option").collect();
    assert!(!options.is_empty());
    assert!(options.iter().all(|option| option.label.is_some()), "every option has a name");
    let active = runtime.state.palette.active.expect("a highlight");
    let selected = options.iter().filter(|option| option.item_selected).count();
    assert_eq!(selected, 1, "one option is selected");
    assert!(runtime.state.palette.entry(active).is_some());
    let groups: Vec<_> = elements
        .iter()
        .filter(|element| element.key.as_deref().is_some_and(|key| key.starts_with("palette-group-")))
        .collect();
    assert_eq!(groups.len(), runtime.state.palette.view.groups.len(), "every group of the list is drawn");
    assert!(groups.iter().all(|group| group.role == "group" && group.label.is_some()), "and is a labelled group");
}

#[test]
fn the_count_is_announced_after_the_typing_pauses() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let mut tx = opened(&mut state, "read");
    assert!(state.palette.announce.is_empty(), "nothing is announced while typing");
    state.poll(0.29, &mut tx);
    assert!(state.palette.announce.is_empty(), "the announcement waits 300 ms");
    state.poll(0.31, &mut tx);
    assert!(!state.palette.announce.is_empty(), "then the count is announced: {}", state.palette.announce);

    let mut tx = Tx::default();
    state.set_palette_query("zzzzzz", &mut tx);
    state.poll(0.70, &mut tx);
    assert_eq!(state.palette.announce, "Nothing matches “zzzzzz”");
    let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
    runtime.frame(0.0);
    let status = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(PALETTE_STATUS_KEY))
        .expect("the status line");
    assert_eq!(status.role, "status");
    assert_eq!(status.text.as_deref(), Some("Nothing matches “zzzzzz”"));
}

#[test]
fn an_empty_result_says_what_was_asked() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    let _tx = opened(&mut state, "zzzzzz");
    assert_eq!(state.palette.view.results, 0, "nothing matches");
    assert_eq!(state.palette.view.empty, "Nothing matches “zzzzzz”");
    assert_eq!(group_titles(&state), ["Search"], "the two search rows are all that is left");
    assert_eq!(state.palette.active, Some(0), "the first search row is highlighted");
}

#[test]
fn the_best_groups_top_row_starts_the_highlight() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    with_notes(&mut state);
    with_files(&mut state, "/docs", &[("/docs/install.md", "install.md")]);
    let _tx = opened(&mut state, "install");
    // “Install” and install.md both match at a word start with no command in
    // the way; the earlier group of equals wins, so the heading is highlighted.
    let best = state.palette.view.best;
    let entry = state.palette.entry(best).expect("the best row");
    assert_eq!(entry.label, "Install");
    assert_eq!(state.palette.active, Some(best));
    let headings = state.palette.view.groups.iter().find(|group| group.title == "In this document").expect("headings");
    assert_eq!(headings.first, best, "the heading group starts there");
    let files = state.palette.view.groups.iter().find(|group| group.title == "Files").expect("files");
    assert!(
        state.palette.entry(files.first).is_some_and(|file| file.label == "install.md"),
        "the file group's top ties on score and keeps the later place"
    );
}
