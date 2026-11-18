//! T-08/T-09/T-10: the tab keyboard.
//!
//! * `Ctrl+Tab` / `Ctrl+Shift+Tab` and `Ctrl+PgDn` / `Ctrl+PgUp` move the front
//!   tab one place, wrapping at the ends (T-08).
//! * `Ctrl+1`…`Ctrl+8` select that tab and `Ctrl+9` the last (T-09). The digit
//!   is read from the physical `Digit` key, as the original reads
//!   `KeyboardEvent.code`, so AZERTY's Ctrl+& selects the first tab; a numpad
//!   digit counts only while Num Lock makes it type one (Rune then reports a
//!   `Command` digit), and Rune has no `keyCode` fallback.
//! * `Ctrl+Shift+PgUp` / `Ctrl+Shift+PgDn` move the front tab one slot
//!   left/right, clamped at the ends (T-10).
//!
//! The editor takes `Ctrl+1`–`Ctrl+3` for its headings first: `app::on_key`
//! runs the editor's own chords before this module. Dialogs, menus, the
//! palette, the previews and a running close sequence keep the tab keys from
//! acting on the window behind them, as the original's `handleTabKey` never
//! runs with one open.
//!
//! Reference: upstream `src/main.ts:2590-2667`.

use rune::{Key, KeyEvent, Tx};

use crate::tab_list::physical_digit;
use crate::window::WindowState;

/// The [`TabKey::Select`] value for `Ctrl+9`: the last tab.
pub const LAST_TAB: usize = usize::MAX;

/// What one tab key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabKey {
    /// One tab forward, wrapping.
    Next,
    /// One tab back, wrapping.
    Previous,
    /// The tab at this 0-based index, or [`LAST_TAB`].
    Select(usize),
    /// One slot towards the start, clamped.
    MoveLeft,
    /// One slot towards the end, clamped.
    MoveRight,
}

/// The tab key `event` is, with the physical key that produced it for T-09's
/// layout rule (none in a test or a script that names no key code).
///
/// A plain Ctrl chord only: Alt (AltGr types) and the Super key are not tab
/// keys, and digits never take Shift (as in the original).
pub(crate) fn tab_key_of(event: &KeyEvent, physical: Option<rune::PhysicalKeyCode>) -> Option<TabKey> {
    let mods = event.mods;
    if !mods.primary() || mods.alt || mods.logo {
        return None;
    }
    match &event.key {
        Key::Tab if mods.shift => Some(TabKey::Previous),
        Key::Tab => Some(TabKey::Next),
        Key::PageDown if mods.shift => Some(TabKey::MoveRight),
        Key::PageUp if mods.shift => Some(TabKey::MoveLeft),
        Key::PageDown => Some(TabKey::Next),
        Key::PageUp => Some(TabKey::Previous),
        _ if !mods.shift => {
            digit_of(event, physical).map(|digit| TabKey::Select(if digit == 9 { LAST_TAB } else { digit - 1 }))
        }
        _ => None,
    }
}

/// The tab number `event` selects, from the character Ctrl types or the
/// physical key behind it (AZERTY's Ctrl+& arrives as `Command('&')`).
fn digit_of(event: &KeyEvent, physical: Option<rune::PhysicalKeyCode>) -> Option<usize> {
    if let Key::Command(character) = &event.key
        && let Some(digit) = character.to_digit(10)
        && (1..=9).contains(&digit)
    {
        return Some(digit as usize);
    }
    physical.and_then(physical_digit)
}

impl WindowState {
    /// Runs one tab key (T-08/T-09/T-10). False when the event is not a tab
    /// key or a dialog/menu is in front.
    ///
    /// A tab key the window can act on is consumed even when there is nothing
    /// to do (one tab, an out-of-range digit), as the original consumes it.
    pub(crate) fn tab_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.tab_keys_active() {
            return false;
        }
        let Some(key) = tab_key_of(event, tx.physical_key()) else { return false };
        // A key cancels a drag in hand, as the original's strip does before it
        // steps (`src/main.ts:2658`).
        self.drag.cancel();
        match key {
            TabKey::Next => {
                let _ = self.run_command("tab.next", tx);
            }
            TabKey::Previous => {
                let _ = self.run_command("tab.previous", tx);
            }
            TabKey::Select(index) => {
                let target = if index == LAST_TAB { self.tabs().len().saturating_sub(1) } else { index };
                self.activate_index_with_tx(target, tx);
                self.reveal_active_tab(tx);
            }
            TabKey::MoveLeft => {
                if self.move_active_tab(-1) {
                    self.reveal_active_tab(tx);
                }
            }
            TabKey::MoveRight => {
                if self.move_active_tab(1) {
                    self.reveal_active_tab(tx);
                }
            }
        }
        true
    }

    /// Whether the tab keys may act: a window surface with no dialog, menu,
    /// palette, preview or close sequence in front (C-09's rule).
    fn tab_keys_active(&self) -> bool {
        !(self.prompt().is_some()
            || self.help_view().is_some()
            || self.share.menu_open
            || self.palette_is_open()
            || self.settings_is_open()
            || self.table_preview().is_some()
            || self.lightbox.is_some()
            || self.block_menu().is_some()
            || self.browser_menu().is_some()
            || self.tab_menu().is_some()
            || self.tab_list_is_open()
            || self.tab_actions.closing_batch())
    }
}
