//! The settings dialog (P6-PREF): opening, every control, persistence, the
//! reset confirmation, the cross-window broadcast, the language picker and the
//! headless scene.

use std::time::Duration;

use lmdv_core::{ColumnWidth, LargeFileMode, LineHeight, Prefs, ReaderFont, SideSize, ThemePref};
use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime, Tx};
use tempfile::TempDir;

use crate::settings::*;
use crate::test_support;
use crate::window::WindowState;

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn runtime(state: WindowState) -> Runtime<WindowState> {
    Runtime::new(state, crate::view::view, (1280.0, 900.0), Env::new(true, 2.0))
}

fn open(state: &mut WindowState) {
    let mut tx = Tx::default();
    state.open_settings(&mut tx);
}

fn ctrl(letter: char) -> KeyEvent {
    KeyEvent { key: Key::Command(letter), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } }
}

fn escape() -> KeyEvent {
    KeyEvent { key: Key::Escape, mods: Mods::default() }
}

fn by_key<'a>(elements: &'a [ElementInfo], key: &str) -> &'a ElementInfo {
    elements
        .iter()
        .find(|element| element.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no element keyed {key}"))
}

fn keys(elements: &[ElementInfo]) -> Vec<&str> {
    elements.iter().filter_map(|element| element.key.as_deref()).collect()
}

fn focused_key(elements: &[ElementInfo]) -> Option<String> {
    elements.iter().find(|element| element.focused).and_then(|element| element.key.clone())
}

fn click(runtime: &mut Runtime<WindowState>, key: &str) {
    let rect = by_key(&runtime.elements(), key).rect;
    let (x, y) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
}

/// One node of the accessibility JSON, by element key.
fn a11y<'a>(node: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    if node.get("key").and_then(|value| value.as_str()) == Some(key) {
        return Some(node);
    }
    node.get("children")?.as_array()?.iter().find_map(|child| a11y(child, key))
}

fn states(node: &serde_json::Value) -> Vec<&str> {
    node.get("states")
        .and_then(|value| value.as_array())
        .map(|states| states.iter().filter_map(|state| state.as_str()).collect())
        .unwrap_or_default()
}

fn stored(state: &WindowState) -> Prefs {
    lmdv_io::store::load_prefs(state.shared().dirs())
}

fn flush(state: &WindowState) {
    assert!(state.shared().flush_store(Duration::from_secs(2)), "the store writer answered");
}

// ---- opening and closing --------------------------------------------------

#[test]
fn the_command_and_ctrl_comma_open_the_dialog() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = Tx::default();

    assert_eq!(
        crate::commands::command_for_key(&ctrl(',')).map(|command| command.id),
        Some("app.settings"),
        "Ctrl+, is the settings chord"
    );
    assert!(!state.settings_is_open());
    assert!(crate::app::on_key(&mut state, &ctrl(','), &mut tx));
    assert!(state.settings_is_open());

    state.close_settings();
    assert!(state.run_command("app.settings", &mut tx), "the palette command opens it too");
    assert!(state.settings_is_open());
}

#[test]
fn escape_backs_out_of_the_language_list_then_the_confirmation_then_the_dialog() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open(&mut state);
    state.settings.language_open = true;
    state.settings.confirm_reset = true;

    let mut tx = Tx::default();
    assert!(crate::app::on_key(&mut state, &escape(), &mut tx));
    assert!(!state.settings.language_open, "Escape closes the list first");
    assert!(state.settings_is_open());

    assert!(crate::app::on_key(&mut state, &escape(), &mut tx));
    assert!(!state.settings.confirm_reset, "then the reset question");
    assert!(state.settings_is_open());

    assert!(crate::app::on_key(&mut state, &escape(), &mut tx));
    assert!(!state.settings_is_open(), "then the dialog");
}

#[test]
fn the_gear_opens_the_dialog_and_focus_returns_to_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "# One\n");
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    click(&mut runtime, SETTINGS_TOOLBAR);
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.settings_is_open());
    let focused = focused_key(&runtime.elements());
    assert!(
        focused.as_deref().is_some_and(|key| key.starts_with(SETTINGS_THEME)),
        "the selected theme segment takes the keyboard: {focused:?}"
    );

    runtime.update(|state, _| state.close_settings());
    runtime.frame(2.0 / 60.0);
    assert_eq!(
        focused_key(&runtime.elements()).as_deref(),
        Some(SETTINGS_TOOLBAR),
        "closing hands focus back to the gear that opened it"
    );
}

#[test]
fn clicking_the_scrim_outside_the_panel_closes_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    assert!(runtime.state.settings_is_open());

    runtime.pointer_move(4.0, 4.0);
    runtime.pointer_down(4.0, 4.0);
    runtime.pointer_up(4.0, 4.0);
    runtime.frame(1.0 / 60.0);
    assert!(!runtime.state.settings_is_open(), "the scrim closes the modal");
}

#[test]
fn space_and_enter_activate_the_focused_control() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    runtime.update(|_, tx| tx.focus("settings-theme-3", false));
    runtime.frame(1.0 / 60.0);
    runtime.key(KeyEvent { key: Key::Text(" ".to_owned()), mods: Mods::default() });
    runtime.frame(2.0 / 60.0);
    assert_eq!(runtime.state.shared().prefs().theme, ThemePref::Dark, "Space chose the focused segment");

    runtime.update(|_, tx| tx.focus(SETTINGS_FORMAT_BAR, false));
    runtime.frame(3.0 / 60.0);
    runtime.key(KeyEvent { key: Key::Text(" ".to_owned()), mods: Mods::default() });
    runtime.frame(4.0 / 60.0);
    assert!(!runtime.state.shared().prefs().editor.format_bar, "Space flipped the focused switch");

    runtime.update(|_, tx| tx.focus(SETTINGS_FORMAT_BAR, false));
    runtime.frame(5.0 / 60.0);
    runtime.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    runtime.frame(6.0 / 60.0);
    assert!(runtime.state.shared().prefs().editor.format_bar, "Enter flips it back");
}

#[test]
fn the_size_value_steps_with_arrows_and_jumps_home_and_end() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.focus("settings-size-value", false));
    runtime.frame(1.0 / 60.0);

    runtime.key(KeyEvent { key: Key::Up, mods: Mods::default() });
    runtime.frame(2.0 / 60.0);
    assert_eq!(runtime.state.shared().prefs().reader.size, 17, "Up steps the size up");
    runtime.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    runtime.frame(3.0 / 60.0);
    assert_eq!(runtime.state.shared().prefs().reader.size, 16, "Down steps it back");

    runtime.key(KeyEvent { key: Key::End, mods: Mods::default() });
    runtime.frame(4.0 / 60.0);
    assert_eq!(runtime.state.shared().prefs().reader.size, 22, "End goes to the largest");
    runtime.key(KeyEvent { key: Key::Home, mods: Mods::default() });
    runtime.frame(5.0 / 60.0);
    assert_eq!(runtime.state.shared().prefs().reader.size, 13, "Home goes to the smallest");
}

#[test]
fn tab_stays_inside_the_dialog_and_reaches_its_controls() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "# One\n");
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    let mut reached: Vec<String> = Vec::new();
    for _ in 0..24 {
        runtime.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        runtime.frame(1.0 / 60.0);
        let focused = focused_key(&runtime.elements());
        let key = focused.expect("the dialog always has a focused control");
        assert!(key == SETTINGS_KEY || key.starts_with("settings-"), "Tab never leaves the modal: {key}");
        reached.push(key);
    }
    for expected in [SETTINGS_THEME, SETTINGS_SIZE, SETTINGS_LANGUAGE_TRIGGER, SETTINGS_CLOSE] {
        assert!(reached.iter().any(|key| key.starts_with(expected)), "the Tab order reaches {expected}: {reached:?}");
    }
    assert!(
        !reached.iter().any(|key| key == "open-file-toolbar" || key == "settings-toolbar"),
        "the window behind the modal is not reachable: {reached:?}"
    );
}

#[test]
fn the_palette_finds_the_settings_command() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = Tx::default();
    state.open_palette(Some(crate::palette::PaletteChord::CtrlK), &mut tx);
    state.set_palette_query("settings", &mut tx);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let option = runtime
        .elements()
        .into_iter()
        .filter(|element| element.role == "option")
        .find(|element| element.label.as_deref().is_some_and(|label| label.contains("Settings")));
    assert!(option.is_some(), "the palette lists Settings… for the query");
}

#[test]
fn a_window_command_is_held_back_while_the_dialog_shows() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "# One\n");
    open(&mut state);
    let mut tx = Tx::default();
    assert!(crate::app::on_key(&mut state, &ctrl('t'), &mut tx), "the chord is swallowed");
    assert_eq!(state.tabs().len(), 1, "Ctrl+T never opened a tab behind the dialog");
    assert!(state.settings_is_open());
}

// ---- the controls ---------------------------------------------------------

#[test]
fn the_dialog_draws_its_sections_controls_and_modal_scrim() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    // A dictionary exists, so the editor section shows its spellcheck switch.
    state.shared().seed_spell(Some(spell_fixture("en_US")), None);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let elements = runtime.elements();

    let panel = by_key(&elements, SETTINGS_PANEL);
    assert_eq!(panel.role, "dialog");
    assert_eq!(panel.label.as_deref(), Some("Settings"));
    assert!(by_key(&elements, SETTINGS_SCRIM).rect[2] > 0.0, "the scrim covers the window");

    for section in [SETTINGS_APPEARANCE, SETTINGS_EDITOR, SETTINGS_GENERAL] {
        let header = by_key(&elements, section);
        assert_eq!(header.role, "heading");
        assert_eq!(header.level, Some(2));
    }

    for (prefix, count) in
        [(SETTINGS_THEME, 5), (SETTINGS_FONT, 2), (SETTINGS_SPACING, 3), (SETTINGS_WIDTH, 4), (SETTINGS_PANELS, 3)]
    {
        let group = by_key(&elements, prefix);
        assert_eq!(group.role, "radiogroup", "{prefix}");
        for index in 0..count {
            let segment = by_key(&elements, &format!("{prefix}-{index}"));
            assert_eq!(segment.role, "radio", "{prefix}-{index}");
            assert!(segment.focusable);
        }
        assert!(
            elements
                .iter()
                .all(|element| !element.key.as_deref().is_some_and(|key| key == format!("{prefix}-{count}")))
        );
    }
    assert_eq!(by_key(&elements, SETTINGS_LARGE_FILE).role, "radiogroup");

    for key in [SETTINGS_FORMAT_BAR, SETTINGS_SPELLCHECK, SETTINGS_RESTORE, SETTINGS_BREAK_H1] {
        assert_eq!(by_key(&elements, key).role, "switch", "{key}");
    }
    assert_eq!(by_key(&elements, SETTINGS_LANGUAGE_TRIGGER).role, "combobox");
    assert_eq!(by_key(&elements, SETTINGS_CLOSE).role, "button");
    assert_eq!(by_key(&elements, SETTINGS_CLOSE_FOOTER).role, "button");

    assert!(by_key(&elements, SETTINGS_TOOLBAR).inert, "the window behind the modal is inert");
    assert!(by_key(&elements, SETTINGS_SCRIM).rect[2] > panel.rect[2], "the scrim is the window-sized layer");
}

#[test]
fn the_spellcheck_row_is_hidden_without_a_dictionary_and_the_reset_starts_disabled() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    // Discovery ran and found no dictionary for this machine.
    state.shared().seed_spell(None, None);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    let tree = runtime.accessibility_json();
    assert!(a11y(&tree, SETTINGS_SPELLCHECK).is_none(), "no dictionary, no switch");
    assert!(a11y(&tree, "settings-row-spellcheck").is_none(), "no dictionary, no row");

    let reset = a11y(&tree, SETTINGS_RESET).expect("the reset button");
    assert!(states(reset).contains(&"disabled"), "defaults need no reset: {:?}", states(reset));
    let close = a11y(&tree, SETTINGS_CLOSE_FOOTER).expect("the close button");
    assert!(!states(close).contains(&"disabled"));
}

/// The fixture dictionary as discovery would report it.
fn spell_fixture(language: &str) -> lmdv_edit::spell::Availability {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/spell/system");
    lmdv_edit::spell::Availability {
        files: lmdv_edit::spell::DictionaryFiles {
            language: language.to_owned(),
            aff: root.join(format!("{language}.aff")),
            dic: root.join(format!("{language}.dic")),
        },
        fallback: false,
    }
}

#[test]
fn the_spellcheck_switch_is_enabled_with_a_dictionary_and_persists() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.shared().seed_spell(Some(spell_fixture("en_US")), None);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    let tree = runtime.accessibility_json();
    let switch = a11y(&tree, SETTINGS_SPELLCHECK).expect("the spellcheck switch");
    assert_eq!(switch.get("role").and_then(|value| value.as_str()), Some("switch"));
    assert!(!states(switch).contains(&"disabled"), "a dictionary is present: {:?}", states(switch));
    let row = a11y(&tree, "settings-row-spellcheck").expect("the spellcheck row");
    assert_eq!(
        row.get("description").and_then(|value| value.as_str()),
        Some("Underlines misspelled words with the en_US dictionary.")
    );
    assert!(runtime.state.shared().prefs().editor.spellcheck, "on by default (E-32)");

    assert!(runtime.state.settings_activate(SETTINGS_SPELLCHECK));
    assert!(!runtime.state.shared().prefs().editor.spellcheck, "the switch turns it off");
    flush(&runtime.state);
    assert!(!stored(&runtime.state).editor.spellcheck, "editor.spellcheck is the stored key (P-11)");

    assert!(runtime.state.settings_activate(SETTINGS_SPELLCHECK));
    assert!(runtime.state.shared().prefs().editor.spellcheck, "and back on");
    flush(&runtime.state);
    assert!(stored(&runtime.state).editor.spellcheck);

    // Reset returns the editor preference to its default (on).
    runtime.update(|state, _| {
        state.set_spellcheck(false);
        state.reset_settings();
    });
    assert!(runtime.state.shared().prefs().editor.spellcheck);
}

#[test]
fn a_fallback_dictionary_says_it_stands_in_for_the_interface_language() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut availability = spell_fixture("en_US");
    availability.fallback = true;
    state.shared().seed_spell(Some(availability), None);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    let tree = runtime.accessibility_json();
    let row = a11y(&tree, "settings-row-spellcheck").expect("the spellcheck row");
    assert_eq!(
        row.get("description").and_then(|value| value.as_str()),
        Some("Using the en_US dictionary because no dictionary matches the interface language.")
    );
}

#[test]
fn a_language_change_keeps_the_dictionary_until_the_next_start() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.shared().seed_spell(Some(spell_fixture("it_IT")), None);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    assert_eq!(runtime.state.spell_availability().map(|found| found.language().to_owned()).as_deref(), Some("it_IT"));
    runtime.update(|state, _| state.set_language(Some("de".to_owned())));
    assert_eq!(
        runtime.state.spell_availability().map(|found| found.language().to_owned()).as_deref(),
        Some("it_IT"),
        "E-32: the checker follows the launch's locale, not a live language change"
    );
}

#[test]
fn every_appearance_control_changes_persists_and_reaches_the_appearance() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = Tx::default();
    assert!(state.run_command("app.settings", &mut tx));

    state.set_theme(ThemePref::Dark);
    state.set_reader_size(20);
    state.set_reader_font(ReaderFont::Serif);
    state.set_line_height(LineHeight::Relaxed);
    state.set_column_width(ColumnWidth::Wide);
    state.set_side_size(SideSize::Large);

    let prefs = state.shared().prefs().clone();
    assert_eq!(prefs.theme, ThemePref::Dark);
    assert_eq!(prefs.reader.size, 20);
    assert_eq!(prefs.reader.font, ReaderFont::Serif);
    assert_eq!(prefs.reader.line_height, LineHeight::Relaxed);
    assert_eq!(prefs.reader.width, ColumnWidth::Wide);
    assert_eq!(prefs.reader.side_size, SideSize::Large);

    flush(&state);
    let stored = stored(&state);
    assert_eq!(stored, prefs, "every control stored the same set the window reads");

    let look = state.current_appearance(false);
    assert!(look.dark);
    assert_eq!(look.theme.body.size, 20.0);
    assert!((look.theme.body.line_height - 38.0).abs() < 0.01, "relaxed at 20 pt");
    assert_eq!(look.theme.body.family, rune::Family::Named(crate::appearance::SERIF_STACK));
    assert_eq!(column_max_width(prefs.reader.width), Some(920.0));
    assert!((pane_scale(prefs.reader.side_size) - 14.5 / 13.0).abs() < 0.001);
}

#[test]
fn the_theme_segments_follow_the_order_and_resolve_like_the_appearance() {
    assert_eq!(theme_index(ThemePref::System), 0);
    assert_eq!(theme_label(ThemePref::Sepia), lmdv_i18n::t!("rs.sepia"));
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    for theme in THEMES {
        state.set_theme(theme);
        assert_eq!(state.shared().prefs().theme, theme);
        let look = state.current_appearance(false);
        assert_eq!(look.dark, crate::appearance::is_dark(theme, false), "{theme:?}");
    }
    state.set_theme(ThemePref::System);
    state.cycle_theme();
    assert_eq!(state.shared().prefs().theme, ThemePref::Light, "System → Light");
    state.cycle_theme();
    assert_eq!(state.shared().prefs().theme, ThemePref::Sepia);
    let sepia = state.current_appearance(false);
    state.set_theme(ThemePref::Light);
    let light = state.current_appearance(false);
    assert_ne!(
        sepia.theme.fingerprint(),
        light.theme.fingerprint(),
        "sepia and light resolve to the same scheme but different colours"
    );
    state.set_theme(ThemePref::Sepia);
    state.cycle_theme();
    state.cycle_theme();
    assert_eq!(state.shared().prefs().theme, ThemePref::Black);
    state.cycle_theme();
    assert_eq!(state.shared().prefs().theme, ThemePref::System, "Black → System");
}

#[test]
fn the_size_stepper_walks_the_eight_steps_and_stops_at_both_ends() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);

    state.set_reader_size(13);
    assert!(!state.can_shrink_reader());
    state.bump_reader_size(-1);
    assert_eq!(state.shared().prefs().reader.size, 13, "no step below 13");
    let mut tx = Tx::default();
    assert!(!state.run_command("app.reader-smaller", &mut tx), "the command is disabled at the end");

    state.set_reader_size(16);
    assert!(state.can_shrink_reader() && state.can_grow_reader());
    assert!(state.run_command("app.reader-larger", &mut tx));
    assert_eq!(state.shared().prefs().reader.size, 17);
    assert!(state.run_command("app.reader-smaller", &mut tx));
    assert_eq!(state.shared().prefs().reader.size, 16);
    assert!(state.run_command("app.reader-larger", &mut tx));
    assert!(state.run_command("app.reader-larger", &mut tx));
    assert_eq!(state.shared().prefs().reader.size, 18, "two steps from 16");

    state.set_reader_size(22);
    assert!(!state.can_grow_reader());
    state.bump_reader_size(1);
    assert_eq!(state.shared().prefs().reader.size, 22);
    assert!(!state.run_command("app.reader-larger", &mut tx));

    assert!(state.run_command("app.reader-size-reset", &mut tx));
    assert_eq!(state.shared().prefs().reader.size, 16);
}

#[test]
fn a_clicked_segment_changes_the_preference_and_the_dialog_shows_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    assert!(by_key(&runtime.elements(), "settings-theme-0").item_selected, "System is selected");

    click(&mut runtime, "settings-theme-3");
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.shared().prefs().theme, ThemePref::Dark);
    assert!(by_key(&runtime.elements(), "settings-theme-3").item_selected, "the dialog follows the live pref");
    assert!(!by_key(&runtime.elements(), "settings-theme-0").item_selected);
}

#[test]
fn a_clicked_switch_changes_its_preference() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    assert!(runtime.state.shared().prefs().editor.format_bar);

    click(&mut runtime, SETTINGS_FORMAT_BAR);
    runtime.frame(1.0 / 60.0);
    assert!(!runtime.state.shared().prefs().editor.format_bar, "the switch turned the bar off");
}

#[test]
fn the_format_bar_preference_hides_the_editor_bar() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "# One\n\nBody.\n");
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx));
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    let drawn = |runtime: &Runtime<WindowState>, key: &str| {
        runtime.elements().iter().any(|element| element.key.as_deref() == Some(key) && !element.exiting)
    };
    assert!(drawn(&runtime, "format-inline"), "the bar starts shown");

    runtime.update(|state, _| state.set_format_bar(false));
    runtime.frame(1.0 / 60.0);
    assert!(!drawn(&runtime, "format-inline"), "the bar is gone");
    assert!(!drawn(&runtime, "format-blocks"), "its second row too");
}

#[test]
fn the_editor_and_general_switches_change_their_preferences() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);

    state.set_large_file_mode(LargeFileMode::Block);
    state.set_restore_session(false);
    state.set_print_break_h1(true);

    let prefs = state.shared().prefs().clone();
    assert_eq!(prefs.editor.large_file_mode, LargeFileMode::Block);
    assert!(!prefs.restore_session);
    assert!(prefs.print_break_h1, "the Share menu's checkbox reads the same preference");
    let share = crate::commands::lookup_command("share.toggle-break-h1").expect("the share command");
    assert!((share.checked.expect("checked"))(&state));
}

#[test]
fn the_default_markdown_row_exists_where_the_action_does() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    let elements = runtime.elements();
    let row = elements.iter().find(|element| element.key.as_deref() == Some(SETTINGS_DEFAULT_MD_ROW));
    let button = elements.iter().find(|element| element.key.as_deref() == Some(SETTINGS_DEFAULT_MD));
    let supported = cfg!(any(windows, target_os = "linux"));
    assert_eq!(row.is_some(), supported, "the default-app row exists exactly where the action does");
    assert_eq!(button.is_some(), supported, "its button, too");
    assert_eq!(default_markdown_supported(), supported);

    if supported {
        assert_eq!(button.map(|button| button.role), Some("button"));
    }

    // The detail says what a press does here. The button itself is never pressed
    // in a test: on Linux that would rewrite the real ~/.config/mimeapps.list.
    let tree = runtime.accessibility_json();
    let description = a11y(&tree, SETTINGS_DEFAULT_MD_ROW)
        .and_then(|row| row.get("description"))
        .and_then(|description| description.as_str());
    if supported {
        assert_eq!(description, Some(default_markdown_detail()));
    } else {
        assert_eq!(description, None, "no row is drawn where the action does not exist");
    }
}

#[test]
fn the_default_detail_names_the_platforms_own_mechanism() {
    if cfg!(windows) {
        assert_eq!(default_markdown_detail(), lmdv_i18n::t!("set.defaultMdDetail"));
    } else {
        assert_eq!(default_markdown_detail(), lmdv_i18n::t!("set.defaultMdDetailLinux"));
        assert_ne!(
            default_markdown_detail(),
            lmdv_i18n::t!("set.defaultMdDetail"),
            "the Windows confirmation hand-off is not what Linux does"
        );
    }
}

#[test]
fn the_default_markdown_result_maps_to_its_notice() {
    use lmdv_io::assoc::{AssociationError, Confirmation};

    let applied = default_markdown_notice(Ok(Confirmation::Applied));
    assert_eq!(applied, (lmdv_i18n::t!("msg.defaultMdApplied").to_owned(), crate::window::NoticeKind::Success));
    assert_ne!(
        applied.0,
        lmdv_i18n::t!("msg.defaultMdUnsupported"),
        "Linux applies the change instead of reporting a platform limit"
    );

    for page in [Confirmation::FilteredSettingsPage, Confirmation::SettingsPage] {
        assert_eq!(
            default_markdown_notice(Ok(page)),
            (lmdv_i18n::t!("msg.defaultMdRegistered").to_owned(), crate::window::NoticeKind::Info),
            "{page:?} is the register-then-confirm hand-off"
        );
    }
    assert_eq!(
        default_markdown_notice(Ok(Confirmation::OpenWithDialog)),
        (lmdv_i18n::t!("msg.defaultMdOpenWith").to_owned(), crate::window::NoticeKind::Info)
    );
    assert_eq!(
        default_markdown_notice(Ok(Confirmation::Unavailable)),
        (lmdv_i18n::t!("msg.defaultMdManual").to_owned(), crate::window::NoticeKind::Warning)
    );
    assert_eq!(
        default_markdown_notice(Err(AssociationError::Unsupported)),
        (lmdv_i18n::t!("msg.defaultMdUnsupported").to_owned(), crate::window::NoticeKind::Info)
    );

    let (text, kind) = default_markdown_notice(Err(AssociationError::NoHome));
    assert_eq!(kind, crate::window::NoticeKind::Warning);
    assert!(text.contains(&AssociationError::NoHome.to_string()), "the reason reaches the user: {text}");
}

// ---- live layout ----------------------------------------------------------

#[test]
fn the_column_width_preference_resizes_the_article_live() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.shared().set_headless();
    state.open_synthetic("One", "# One\n\nSome prose to lay out in a column.\n");
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.frame(1.0 / 60.0);
    let article_width = |runtime: &Runtime<WindowState>| {
        runtime
            .elements()
            .into_iter()
            .find(|element| element.key.as_deref() == Some(crate::reader::ARTICLE_KEY))
            .map(|element| element.rect[2])
    };
    let normal = article_width(&runtime).expect("the article");
    assert!((normal - 760.0).abs() < 1.0, "the normal column is 760 points: {normal}");

    runtime.update(|state, _| state.set_column_width(ColumnWidth::Narrow));
    for frame in 0..30 {
        runtime.frame(2.0 + f64::from(frame) / 60.0);
    }
    let narrow = article_width(&runtime).expect("the article");
    assert!((narrow - 640.0).abs() < 1.0, "narrow is 640 points: {narrow}");

    runtime.update(|state, _| state.set_column_width(ColumnWidth::Full));
    for frame in 0..30 {
        runtime.frame(3.0 + f64::from(frame) / 60.0);
    }
    let full = article_width(&runtime).expect("the article");
    assert!(full > 1000.0, "full fills the pane: {full}");
    assert!(full > narrow && narrow < normal);
}

#[test]
fn the_side_panel_kit_scales_only_for_small_and_large() {
    let kit = rune_kit::Kit::neutral(false);
    let small = pane_kit(&kit, SideSize::Small);
    let normal = pane_kit(&kit, SideSize::Normal);
    let large = pane_kit(&kit, SideSize::Large);
    assert!(small.ty.ui.size < normal.ty.ui.size);
    assert!(large.ty.ui.size > normal.ty.ui.size);
    assert_eq!(normal.ty.ui.size, kit.ty.ui.size);
    assert_eq!(normal.ty.ui.line_height, kit.ty.ui.line_height);
    assert_eq!(small.ty.mono.size, kit.ty.mono.size, "mono keeps the window scale");
    assert_eq!(kit.ty.ui.size, 13.0, "the kit's UI size the original 13 px panel maps from");
}

#[test]
fn an_open_dialog_follows_another_windows_change() {
    let shared = test_support::shared(&TempDir::new().unwrap());
    let mut first = WindowState::new(shared.clone());
    let mut second = WindowState::new(shared.clone());
    open(&mut second);
    let mut runtime = runtime(second);
    runtime.frame(0.0);
    assert!(by_key(&runtime.elements(), "settings-theme-0").item_selected);

    first.set_theme(ThemePref::Dark);
    // The bare test runtime does not run the app's poll; drain the mailbox the
    // broadcast filled, as a real frame would.
    runtime.update(|state, tx| {
        assert_eq!(state.ui_inbox().len(), 1);
        assert!(state.poll(1.0, tx), "PrefsChanged asks for a frame");
    });
    runtime.frame(1.0 / 60.0);
    assert!(by_key(&runtime.elements(), "settings-theme-3").item_selected, "the open dialog shows the broadcast value");
    assert!(!by_key(&runtime.elements(), "settings-theme-0").item_selected);
}

#[test]
fn the_reset_confirmation_replaces_the_footer_buttons() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.set_reader_size(20);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    click(&mut runtime, SETTINGS_RESET);
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.settings.confirm_reset);
    let elements = runtime.elements();
    assert!(elements.iter().any(|element| element.key.as_deref() == Some(SETTINGS_RESET_CONFIRM) && !element.exiting));
    assert!(!elements.iter().any(|element| element.key.as_deref() == Some(SETTINGS_CLOSE_FOOTER) && !element.exiting));

    click(&mut runtime, SETTINGS_RESET_CANCEL);
    runtime.frame(2.0 / 60.0);
    assert!(!runtime.state.settings.confirm_reset);
    assert_eq!(runtime.state.shared().prefs().reader.size, 20, "cancel kept the change");
}

// ---- reset ----------------------------------------------------------------

#[test]
fn reset_asks_first_and_then_restores_every_default() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let defaults = Prefs::default();
    state.set_theme(ThemePref::Dark);
    state.set_reader_size(22);
    state.set_reader_font(ReaderFont::Serif);
    state.set_language(Some("it".to_owned()));
    state.set_print_break_h1(true);
    assert!(!state.prefs_are_default());

    open(&mut state);
    state.request_reset_settings();
    assert!(state.settings.confirm_reset, "the first press only asks");
    assert_eq!(state.shared().prefs().reader.size, 22, "nothing changed yet");

    state.cancel_reset_settings();
    assert!(!state.settings.confirm_reset);
    state.request_reset_settings();
    state.reset_settings();
    assert!(!state.settings.confirm_reset);
    let expected = Prefs { print_break_h1: true, ..defaults };
    assert_eq!(*state.shared().prefs(), expected, "every resettable section is back to its default");
    assert!(state.shared().prefs().print_break_h1, "the H1 page break survives the reset, as in the original");
    assert!(state.prefs_are_default());
    assert_eq!(state.language_current(), lmdv_i18n::t!("rs.langAuto"), "language is Auto again");
    assert!(!state.current_appearance(false).dark, "the theme is System again");
}

#[test]
fn a_no_op_change_does_not_store_or_wake_a_window() {
    let shared = test_support::shared(&TempDir::new().unwrap());
    let first = WindowState::new(shared.clone());
    let second = WindowState::new(shared.clone());
    second.ui_inbox().drain();

    let mut first = first;
    first.set_theme(ThemePref::Dark);
    let posted = second.ui_inbox().len();
    assert_eq!(posted, 1, "a real change broadcasts");
    second.ui_inbox().drain();

    first.set_theme(ThemePref::Dark);
    assert_eq!(second.ui_inbox().len(), 0, "the same value broadcasts nothing");
}

// ---- cross-window ---------------------------------------------------------

#[test]
fn a_preference_change_reaches_and_redraws_every_window() {
    let shared = test_support::shared(&TempDir::new().unwrap());
    let mut first = WindowState::new(shared.clone());
    let mut second = WindowState::new(shared.clone());
    second.open_synthetic("Note", "# Note\n");
    let mut tx = Tx::default();

    // Cache the second window's appearance under the default light theme.
    assert!(!second.current_appearance(false).dark);

    first.set_theme(ThemePref::Black);
    assert_eq!(second.ui_inbox().len(), 1, "the broadcast reached the other window");
    assert!(matches!(second.ui_inbox().drain().as_slice(), [crate::shared::UiMsg::PrefsChanged]));
    assert!(second.poll(0.0, &mut tx), "the message asks for a frame");
    assert_eq!(second.shared().prefs().theme, ThemePref::Black);
    assert!(second.current_appearance(false).dark, "the other window's cache was invalidated");

    second.set_reader_size(20);
    assert!(first.poll(0.0, &mut tx));
    assert_eq!(first.shared().prefs().reader.size, 20);
    assert_eq!(first.current_appearance(false).theme.body.size, 20.0);
}

// ---- language -------------------------------------------------------------

#[test]
fn the_language_picker_offers_auto_and_six_endonyms() {
    let options = language_options();
    assert_eq!(options.len(), 7, "Auto + six catalogs");
    assert_eq!(options[0].id, None);
    assert_eq!(options[0].label, lmdv_i18n::t!("rs.langAuto"));
    let ids: Vec<&str> = options.iter().filter_map(|option| option.id.as_deref()).collect();
    for expected in ["en", "it", "de", "fr", "es", "zh-CN"] {
        assert!(ids.contains(&expected), "{expected} is offered: {ids:?}");
    }
    assert_eq!(
        options.iter().find(|option| option.id.as_deref() == Some("it")).map(|option| option.label.as_str()),
        Some("Italiano")
    );
}

#[test]
fn choosing_a_language_persists_it_and_reports_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    assert_eq!(state.language_current(), lmdv_i18n::t!("rs.langAuto"));

    state.settings.language_open = true;
    state.set_language(Some("it".to_owned()));
    assert_eq!(state.shared().prefs().locale.as_deref(), Some("it"));
    assert_eq!(state.language_current(), "Italiano");
    assert!(!state.settings.language_open, "choosing closes the list");
    flush(&state);
    assert_eq!(stored(&state).locale.as_deref(), Some("it"));

    state.set_language_label("Deutsch");
    assert_eq!(state.shared().prefs().locale.as_deref(), Some("de"));

    state.set_language(None);
    assert_eq!(state.shared().prefs().locale, None, "Auto removes the stored id");
    flush(&state);
    assert_eq!(stored(&state).locale, None);
}

#[test]
fn choosing_a_language_switches_translated_labels_live_in_an_isolated_scope() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open(&mut state);

    // OI-176: the picker changes strings in-process. The scoped translator keeps
    // this thread's switch away from the process-wide locale other tests read.
    lmdv_i18n::with_locale("en", || {
        assert_eq!(lmdv_i18n::t!("rs.language"), "Language");
        state.set_language(Some("it".to_owned()));
        assert_eq!(lmdv_i18n::locale_id(), "it");
        assert_eq!(lmdv_i18n::t!("rs.language"), "Lingua");

        // A window built after the switch draws the translated row, not a
        // stale English snapshot.
        let mut rt = runtime(state);
        rt.frame(0.0);
        assert!(
            rt.elements().iter().any(|element| element.text.as_deref() == Some("Lingua")),
            "the language row follows the live locale: {:?}",
            rt.elements().iter().filter_map(|element| element.text.as_deref()).take(16).collect::<Vec<_>>()
        );
    });
    assert!(!lmdv_i18n::is_isolated(), "the scope restored the thread's translator on exit");
}

#[test]
fn the_locale_index_prefers_the_stored_id_and_falls_back_to_auto() {
    let mut prefs = Prefs::default();
    assert_eq!(locale_index(&prefs), lmdv_i18n::negotiate_system());
    prefs.locale = Some("it".to_owned());
    assert_eq!(locale_index(&prefs), lmdv_i18n::find_locale("it").expect("Italian"));
    prefs.locale = Some("klingon".to_owned());
    assert_eq!(locale_index(&prefs), lmdv_i18n::negotiate_system(), "an unknown id falls back to Auto");
}

// ---- scene ----------------------------------------------------------------

#[test]
fn the_palette_scenes_show_the_scheme_without_writing_prefs() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);

    crate::view::scene_reader_sepia(&mut state);
    assert_eq!(state.shared().prefs().theme, ThemePref::Sepia, "the scene previews sepia");
    let look = state.current_appearance(false);
    assert!(!look.dark, "sepia is a light scheme");
    assert_eq!(look.theme.text.hex(), "#3b2f22ff");
    flush(&state);
    assert_eq!(stored(&state), Prefs::default(), "a scene never writes prefs.json");

    crate::view::scene_reader_black(&mut state);
    assert_eq!(state.shared().prefs().theme, ThemePref::Black, "the scene previews black");
    let look = state.current_appearance(false);
    assert!(look.dark, "black is a dark scheme");
    assert_eq!(look.theme.text.hex(), "#e6e6e3ff");
    flush(&state);
    assert_eq!(stored(&state), Prefs::default());

    crate::view::scene_editor_size(&mut state);
    assert_eq!(state.shared().prefs().reader.size, 22);
    assert_eq!(state.editor_metrics().size, 19.25, "E-37 in the editor-size scene");
    assert!((state.editor_metrics().line_height - 30.8).abs() < 1e-4);
}

#[test]
fn the_settings_dialog_draws_with_the_selected_scheme() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.set_theme(ThemePref::Sepia);
    open(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);

    let sepia = crate::appearance::palette(ThemePref::Sepia, false);
    let elements = runtime.elements();
    let panel = by_key(&elements, SETTINGS_PANEL);
    assert_eq!(panel.fill.hex(), sepia.c.layer_01.hex(), "the dialog is a theme surface");
    let selected = by_key(&elements, "settings-theme-2");
    assert!(selected.item_selected);
    assert_eq!(selected.fill.hex(), sepia.c.bg_base.hex(), "the selected segment is the card");
}

#[test]
fn the_scene_shows_the_dialog_over_a_document() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::settings_ui::scene_settings(&mut state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    assert!(runtime.state.settings_is_open());
    let elements = runtime.elements();
    assert_eq!(by_key(&elements, SETTINGS_PANEL).role, "dialog");
    assert!(by_key(&elements, SETTINGS_SCRIM).rect[2] > 0.0);
    assert!(keys(&elements).contains(&"tab-band"), "the window's tabs stand behind the dialog");
}
