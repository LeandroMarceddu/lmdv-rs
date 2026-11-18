//! T-08/T-09/T-10: the tab keys and their wrap/clamp/layout rules.

use rune::{Key, KeyEvent, Mods, PhysicalKeyCode, Tx};
use tempfile::TempDir;

use crate::tab_keys::{LAST_TAB, TabKey, tab_key_of};
use crate::window::{HelpView, Mode, WindowState};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(crate::test_support::shared(temp))
}

fn ctrl(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { cmd: true, ctrl: true, ..Mods::default() } }
}

fn ctrl_shift(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } }
}

/// Three documents, the third in front, titled One, Two, Three.
fn three_tabs(state: &mut WindowState) {
    state.open_synthetic("One", "# One\n\nBody.\n");
    state.open_synthetic("Two", "# Two\n\nBody.\n");
    state.open_synthetic("Three", "# Three\n\nBody.\n");
}

fn titles(state: &WindowState) -> Vec<String> {
    state.tabs().iter().map(|tab| tab.full_title()).collect()
}

// ---- reading the chords (T-08/T-09/T-10) --------------------------------

#[test]
fn the_page_keys_step_move_and_shift_only_moves() {
    assert_eq!(tab_key_of(&ctrl(Key::PageDown), None), Some(TabKey::Next));
    assert_eq!(tab_key_of(&ctrl(Key::PageUp), None), Some(TabKey::Previous));
    assert_eq!(tab_key_of(&ctrl_shift(Key::PageDown), None), Some(TabKey::MoveRight));
    assert_eq!(tab_key_of(&ctrl_shift(Key::PageUp), None), Some(TabKey::MoveLeft));
    assert_eq!(tab_key_of(&ctrl_shift(Key::Tab), None), Some(TabKey::Previous));
    assert_eq!(tab_key_of(&ctrl(Key::Tab), None), Some(TabKey::Next));
}

#[test]
fn the_digits_are_read_from_the_character_or_the_physical_key() {
    assert_eq!(tab_key_of(&ctrl(Key::Command('1')), None), Some(TabKey::Select(0)));
    assert_eq!(tab_key_of(&ctrl(Key::Command('8')), None), Some(TabKey::Select(7)));
    assert_eq!(tab_key_of(&ctrl(Key::Command('9')), None), Some(TabKey::Select(LAST_TAB)));
    assert_eq!(tab_key_of(&ctrl(Key::Command('0')), None), None, "Ctrl+0 is not a tab");
    // AZERTY: Ctrl+& (the Digit1 key) arrives as Command('&'); the physical
    // key still names tab 1, as the original's KeyboardEvent.code does.
    assert_eq!(tab_key_of(&ctrl(Key::Command('&')), Some(PhysicalKeyCode::Digit1)), Some(TabKey::Select(0)));
    // Shift never selects a tab (the original checks !e.shiftKey), so the
    // French Ctrl+Shift+1 that types "1" is not a tab key.
    assert_eq!(tab_key_of(&ctrl_shift(Key::Command('&')), Some(PhysicalKeyCode::Digit1)), None);
    // A numpad digit counts only while Num Lock makes it type one: Rune then
    // reports Command('1'); with Num Lock off the key is End, not a digit.
    assert_eq!(tab_key_of(&ctrl(Key::End), Some(PhysicalKeyCode::Numpad1)), None);
    assert_eq!(tab_key_of(&ctrl(Key::Command('1')), Some(PhysicalKeyCode::Numpad1)), Some(TabKey::Select(0)));
}

#[test]
fn alt_and_super_are_not_tab_chords() {
    let alt = Mods { cmd: true, ctrl: true, alt: true, ..Mods::default() };
    assert_eq!(tab_key_of(&KeyEvent { key: Key::PageDown, mods: alt }, None), None);
    let logo = Mods { cmd: true, ctrl: true, logo: true, ..Mods::default() };
    assert_eq!(tab_key_of(&KeyEvent { key: Key::PageDown, mods: logo }, None), None);
    assert_eq!(tab_key_of(&KeyEvent { key: Key::PageDown, mods: Mods::default() }, None), None);
}

// ---- stepping (T-08) -----------------------------------------------------

#[test]
fn pgdn_pgup_and_ctrl_tab_wrap_around_the_strip() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    three_tabs(&mut state);
    let mut tx = Tx::default();
    assert_eq!(state.active_index(), 2, "the start page was replaced, then Two and Three");

    assert!(state.tab_key(&ctrl(Key::PageDown), &mut tx));
    assert_eq!(state.active_index(), 0, "PgDn wraps to the first tab");
    assert!(state.tab_key(&ctrl(Key::PageUp), &mut tx));
    assert_eq!(state.active_index(), 2, "PgUp wraps to the last tab");
    assert!(state.tab_key(&ctrl(Key::Tab), &mut tx));
    assert_eq!(state.active_index(), 0, "Ctrl+Tab keeps stepping");
    assert!(state.tab_key(&ctrl_shift(Key::Tab), &mut tx));
    assert_eq!(state.active_index(), 2);
}

#[test]
fn one_tab_consumes_the_chord_and_stays() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "# One\n");
    let mut tx = Tx::default();
    assert_eq!(state.tabs().len(), 1);
    assert!(state.tab_key(&ctrl(Key::PageDown), &mut tx), "the key is the window's even with nothing to step");
    assert_eq!(state.active_index(), 0);
}

// ---- numbered tabs (T-09) ------------------------------------------------

#[test]
fn numbered_digits_select_a_tab_and_nine_is_the_last() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    three_tabs(&mut state);
    let mut tx = Tx::default();

    assert!(state.tab_key(&ctrl(Key::Command('1')), &mut tx));
    assert_eq!(state.active_index(), 0);
    assert!(state.tab_key(&ctrl(Key::Command('2')), &mut tx));
    assert_eq!(state.active_index(), 1);
    assert!(state.tab_key(&ctrl(Key::Command('9')), &mut tx));
    assert_eq!(state.active_index(), 2, "Ctrl+9 is the last tab");
}

#[test]
fn an_out_of_range_digit_does_nothing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    three_tabs(&mut state);
    let mut tx = Tx::default();
    assert_eq!(state.active_index(), 2);
    assert!(state.tab_key(&ctrl(Key::Command('5')), &mut tx), "the chord is still the window's");
    assert_eq!(state.active_index(), 2, "there is no fifth tab");
}

// ---- moving the front tab (T-10) -----------------------------------------

#[test]
fn shift_page_keys_move_the_front_tab_and_clamp_at_the_ends() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    three_tabs(&mut state);
    state.activate_index(1);
    let mut tx = Tx::default();
    assert_eq!(titles(&state), ["One", "Two", "Three"]);

    assert!(state.tab_key(&ctrl_shift(Key::PageUp), &mut tx));
    assert_eq!(titles(&state), ["Two", "One", "Three"]);
    assert_eq!(state.active_index(), 0, "the moved tab stays in front");
    assert_eq!(state.active_tab().map(|tab| tab.full_title()), Some("Two".to_owned()));

    // Already first: the key is consumed, the order stays.
    assert!(state.tab_key(&ctrl_shift(Key::PageUp), &mut tx));
    assert_eq!(titles(&state), ["Two", "One", "Three"]);

    assert!(state.tab_key(&ctrl_shift(Key::PageDown), &mut tx));
    assert!(state.tab_key(&ctrl_shift(Key::PageDown), &mut tx));
    assert_eq!(titles(&state), ["One", "Three", "Two"]);
    assert_eq!(state.active_index(), 2);
    assert!(state.tab_key(&ctrl_shift(Key::PageDown), &mut tx));
    assert_eq!(titles(&state), ["One", "Three", "Two"], "clamped at the end");
}

// ---- who keeps the keys --------------------------------------------------

#[test]
fn a_dialog_keeps_the_tab_keys_off_the_window() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    three_tabs(&mut state);
    let mut tx = Tx::default();
    state.open_help(HelpView::Shortcuts);
    assert!(!state.tab_key(&ctrl(Key::PageDown), &mut tx), "Help is modal");
    assert!(!state.tab_key(&ctrl(Key::Command('1')), &mut tx));
    assert_eq!(state.active_index(), 2);
}

#[test]
fn the_editor_takes_ctrl_1_to_3_for_headings_first() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    three_tabs(&mut state);
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx));
    assert_eq!(state.active_tab().map(|tab| tab.view.mode), Some(Mode::Editor));
    let active = state.active_index();

    assert!(crate::app::on_key(&mut state, &ctrl(Key::Command('1')), &mut tx));
    assert_eq!(state.ui.last_command.as_deref(), Some("edit.heading-1"));
    assert_eq!(state.active_index(), active, "Ctrl+1 was the heading, not the first tab");

    // Ctrl+4 is not an editor heading: it falls through to the tab keys.
    assert!(crate::app::on_key(&mut state, &ctrl(Key::Command('4')), &mut tx));
    assert_eq!(state.active_index(), 2, "Ctrl+4 selected the fourth tab (none: stayed)");
    assert!(crate::app::on_key(&mut state, &ctrl(Key::Command('2')), &mut tx));
    assert_eq!(state.ui.last_command.as_deref(), Some("edit.heading-2"));
    assert_eq!(state.active_index(), 2);
}
