//! T-17's all-tabs list: the strip's ⌄ control.
//!
//! The kit strip scrolls with a linear fade at each edge and keeps its own
//! wheel axis, but it does not say whether it overflowed. The app measures the
//! two rows the kit lays out — the horizontal scroll viewport (`tabs-scroll`)
//! and the tabs inside it (`tabs-list`) — from the previous frame, so no
//! threshold has to guess the kit's policy. The button appears when the tabs'
//! laid-out width passes the viewport's. The first frame has no layout yet and
//! shows no button; the next frame corrects that before it is read.
//!
//! The list itself is a kit `Menu` (`view::tab_list_button`): every tab with
//! the active one marked ●, a file's folder as its hint, then New tab. Choosing
//! a tab activates it and closes the list; New tab appends a start page, as the
//! original's list does (`src/tabs/strip.ts:796-815`).

use rune::{PhysicalKeyCode, Tx};

use crate::window::WindowState;

/// The ⌄ button's element key.
pub const TAB_LIST_BUTTON_KEY: &str = "tab-list-button";
/// The list menu's key.
pub const TAB_LIST_MENU_KEY: &str = "tab-list-menu";
/// The tab strip's scroll viewport, as the kit names it.
pub const TAB_STRIP_SCROLL_KEY: &str = "tabs-scroll";
/// The tab strip's list row, as the kit names it.
pub const TAB_STRIP_LIST_KEY: &str = "tabs-list";

/// Sub-pixel slack when comparing the tabs' width with their viewport.
pub const STRIP_OVERFLOW_SLACK: f32 = 0.5;

/// Whether the tabs overflow their strip, from the laid-out widths of the
/// previous frame (T-17).
///
/// The app never gives the kit a `viewport_width`, so the tabs keep their
/// 224-point natural width (the kit's maximum) and the strip scrolls instead of
/// shrinking below it; overflow therefore means the list's measured width is
/// wider than the scroll viewport's. Both widths include the reading time a tab
/// shows and the actions after the last tab.
pub fn strip_overflows(scroll: Option<[f32; 2]>, list: Option<[f32; 2]>) -> bool {
    match (scroll, list) {
        (Some(scroll), Some(list)) => list[0] > scroll[0] + STRIP_OVERFLOW_SLACK,
        // The first frame has no layout; the next one does.
        _ => false,
    }
}

/// The all-tabs list's open flag (T-17).
#[derive(Debug, Default)]
pub struct TabListState {
    open: bool,
}

impl WindowState {
    /// Whether the ⌄ list is showing.
    pub fn tab_list_is_open(&self) -> bool {
        self.tab_list.open
    }

    /// Opens the ⌄ list under its button.
    pub fn open_tab_list(&mut self) {
        self.tab_list.open = true;
    }

    /// Closes the ⌄ list (Escape, an outside press, or a choice).
    pub fn close_tab_list(&mut self) {
        self.tab_list.open = false;
    }

    /// The ⌄ button's press: shows the list, or puts it away.
    pub fn toggle_tab_list(&mut self) {
        self.tab_list.open = !self.tab_list.open;
    }

    /// Reveals the active tab in the strip, after a key or the list moved the
    /// front tab (T-08/T-10/T-17): the strip scrolls it into its viewport.
    pub(crate) fn reveal_active_tab(&self, tx: &mut Tx) {
        if let Some(tab) = self.active_tab() {
            tx.scroll_into_view_in(TAB_STRIP_SCROLL_KEY, format!("t-{}", tab.id));
        }
    }
}

/// The tab number a physical digit key names, for T-09's layout rule.
///
/// The original reads `KeyboardEvent.code` (`Digit1`…`Digit9`), so AZERTY's
/// Ctrl+& selects the first tab; the numpad counts only while Num Lock makes
/// it type a digit, which Rune reports as a `Command` character instead, so
/// it is not read here. Rune has no `keyCode` fallback: a backend that names
/// neither a `Command` digit nor a physical key cannot select a tab.
pub(crate) fn physical_digit(code: PhysicalKeyCode) -> Option<usize> {
    use PhysicalKeyCode as Key;
    Some(match code {
        Key::Digit1 => 1,
        Key::Digit2 => 2,
        Key::Digit3 => 3,
        Key::Digit4 => 4,
        Key::Digit5 => 5,
        Key::Digit6 => 6,
        Key::Digit7 => 7,
        Key::Digit8 => 8,
        Key::Digit9 => 9,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};
    use tempfile::TempDir;

    use super::*;
    use crate::window::WindowState;

    fn window(temp: &TempDir) -> WindowState {
        WindowState::new(crate::test_support::shared(temp))
    }

    /// Six tabs at 1280 points: 6×224 plus gaps passes the 1264-point strip.
    fn crowded(state: &mut WindowState) {
        for index in 1..=6 {
            state.open_synthetic(&format!("Note {index}"), &format!("# Note {index}\n\nBody.\n"));
        }
    }

    fn click(runtime: &mut Runtime<WindowState>, key: &str) {
        let (x, y) = runtime.find(key).unwrap_or_else(|| panic!("no element {key}"));
        runtime.pointer_move(x, y);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
    }

    #[test]
    fn the_list_overflows_only_past_the_viewport() {
        assert!(!strip_overflows(Some([600.0, 28.0]), Some([600.0, 28.0])), "the tabs fit");
        assert!(!strip_overflows(Some([600.0, 28.0]), Some([540.0, 28.0])), "a shorter list fits");
        assert!(strip_overflows(Some([600.0, 28.0]), Some([601.0, 28.0])), "a wider list scrolls");
        assert!(!strip_overflows(None, Some([601.0, 28.0])), "the first frame has no layout");
        assert!(!strip_overflows(Some([600.0, 28.0]), None));
    }

    #[test]
    fn the_button_toggles_the_list() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        assert!(!state.tab_list_is_open());
        state.toggle_tab_list();
        assert!(state.tab_list_is_open());
        state.close_tab_list();
        assert!(!state.tab_list_is_open());
        state.open_tab_list();
        assert!(state.tab_list_is_open());
    }

    #[test]
    fn physical_digits_name_one_to_nine_and_nothing_else() {
        assert_eq!(physical_digit(PhysicalKeyCode::Digit1), Some(1));
        assert_eq!(physical_digit(PhysicalKeyCode::Digit9), Some(9));
        assert_eq!(physical_digit(PhysicalKeyCode::Digit0), None);
        assert_eq!(physical_digit(PhysicalKeyCode::Numpad1), None, "Num Lock decides a numpad digit");
        assert_eq!(physical_digit(PhysicalKeyCode::KeyQ), None);
    }

    #[test]
    fn the_button_appears_only_when_the_tabs_overflow() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("One", "# One\n");
        let mut runtime = Runtime::new(state, crate::view::view, (1280.0, 900.0), Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        assert!(runtime.find(TAB_LIST_BUTTON_KEY).is_none(), "two tabs fit: no ⌄");

        runtime.update(|state, _| crowded(state));
        runtime.frame(2.0 / 60.0);
        runtime.frame(3.0 / 60.0);
        assert!(runtime.find(TAB_LIST_BUTTON_KEY).is_some(), "the crowded strip shows ⌄");
    }

    #[test]
    fn choosing_a_tab_from_the_list_activates_it_and_closes_the_list() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        crowded(&mut state);
        state.activate_index(2);
        let mut runtime = Runtime::new(state, crate::view::view, (1280.0, 900.0), Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        click(&mut runtime, TAB_LIST_BUTTON_KEY);
        runtime.frame(2.0 / 60.0);
        assert!(runtime.state.tab_list_is_open());
        assert!(runtime.find(&format!("{TAB_LIST_MENU_KEY}-item0")).is_some());

        let marks =
            runtime.elements().into_iter().filter(|element| element.text.as_deref() == Some("\u{25cf}")).count();
        assert_eq!(marks, 1, "exactly the front tab is marked ●");

        click(&mut runtime, &format!("{TAB_LIST_MENU_KEY}-item0"));
        assert_eq!(runtime.state.active_index(), 0);
        assert!(!runtime.state.tab_list_is_open(), "the choice closes the list");
        assert!(runtime.find(&format!("{TAB_LIST_MENU_KEY}-menu")).is_none());
    }

    #[test]
    fn escape_closes_the_list() {
        use rune::{Key, KeyEvent, Mods};
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        crowded(&mut state);
        state.activate_index(1);
        let mut runtime = Runtime::new(state, crate::view::view, (1280.0, 900.0), Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        click(&mut runtime, TAB_LIST_BUTTON_KEY);
        runtime.frame(2.0 / 60.0);
        assert!(runtime.state.tab_list_is_open());
        runtime.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        runtime.frame(3.0 / 60.0);
        assert!(!runtime.state.tab_list_is_open(), "Escape closes the list");
        assert!(runtime.find(&format!("{TAB_LIST_MENU_KEY}-menu")).is_none());
        assert_eq!(runtime.state.active_index(), 1, "closing changed no tab");
    }

    #[test]
    fn file_tabs_show_their_folder_and_new_tab_appends_a_start_page() {
        let temp = TempDir::new().unwrap();
        let folder = temp.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("one.md");
        std::fs::write(&path, "# One\n").unwrap();
        let mut state = window(&temp);
        state.open_path(path);
        for index in 1..=6 {
            state.open_synthetic(&format!("Note {index}"), &format!("# Note {index}\n\nBody.\n"));
        }
        state.activate_index(0);
        state.open_tab_list();
        let mut runtime = Runtime::new(state, crate::view::view, (1280.0, 900.0), Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        runtime.frame(2.0 / 60.0);

        let hint = folder.display().to_string();
        let elements = runtime.elements();
        assert!(
            elements.iter().any(|element| element.key.as_deref() == Some("description")
                && element.text.as_deref() == Some(hint.as_str())),
            "the file's folder is its hint"
        );
        let new_tab = elements
            .iter()
            .find(|element| element.role == "menuitem" && element.label.as_deref() == Some("New tab"))
            .and_then(|element| element.key.clone())
            .expect("the New tab item");

        let tabs = runtime.state.tabs().len();
        click(&mut runtime, &new_tab);
        assert_eq!(runtime.state.tabs().len(), tabs + 1, "New tab appended");
        assert!(runtime.state.active_tab().is_some_and(|tab| tab.is_start()));
        assert!(!runtime.state.tab_list_is_open());
    }
}
