//! P6-M: the tab hover card (T-16) and the pointer-frozen reading time (T-35).

use lmdv_core::TabId;
use rune::{PointerPress, Tx};
use tempfile::TempDir;

use crate::reading_meta::ReadingStats;
use crate::tab_card::{TAB_CARD_DELAY, card_content};
use crate::test_support;
use crate::view::view;
use crate::window::{Mode, Tab, WindowState, tab_key};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn hover(state: &mut WindowState, tab: TabId, inside: bool, tx: &mut Tx) {
    state.tab_hover_changed(&tab_key(tab), inside, tx);
}

#[test]
fn a_resting_pointer_waits_550_ms_for_the_card() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Notes", "# Notes\n");
    let mut tx = Tx::default();

    hover(&mut state, tab, true, &mut tx);
    assert_eq!(state.tab_card().hovered(), Some(tab));
    assert_eq!(state.tab_card().shown(), None, "nothing appears at once");
    assert!(!state.poll_tab_card(TAB_CARD_DELAY - 0.01));
    assert_eq!(state.tab_card().shown(), None, "the delay has not passed");

    assert!(state.poll_tab_card(TAB_CARD_DELAY));
    assert_eq!(state.tab_card().shown(), Some(tab), "after 550 ms the card is up");
}

#[test]
fn moving_from_tab_to_tab_keeps_the_card_up_at_once() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "one\n");
    let second = state.open_synthetic("Second", "two\n");
    let third = state.open_synthetic("Third", "three\n");
    let mut tx = Tx::default();

    hover(&mut state, first, true, &mut tx);
    assert!(state.poll(TAB_CARD_DELAY, &mut tx));
    assert_eq!(state.tab_card().shown(), Some(first));

    // Straight to the next tab: shown in the same change, with no new delay.
    hover(&mut state, second, true, &mut tx);
    assert_eq!(state.tab_card().shown(), Some(second), "the card follows at once");
    hover(&mut state, first, false, &mut tx);
    assert_eq!(state.tab_card().shown(), Some(second), "the tab left is not the one hovered");

    // Leaving the strip closes it; coming back inside the warm window is at once.
    hover(&mut state, second, false, &mut tx);
    assert_eq!(state.tab_card().shown(), None, "leaving hides the card");
    hover(&mut state, third, true, &mut tx);
    assert_eq!(state.tab_card().shown(), Some(third), "a pointer crossing the gap keeps the card warm");
    assert!(!state.poll(TAB_CARD_DELAY, &mut tx), "it was shown immediately, without a timer");
}

#[test]
fn the_card_names_the_file_its_folder_and_the_time_or_unsaved_changes() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let folder = temp.path().join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("A note.md");
    std::fs::write(&file, "# A note\n").unwrap();
    state.open_path(file.clone());
    let stats = ReadingStats { words: 440, cjk: 0, minutes: 2 };
    {
        let tab = state.active_tab_mut().expect("the file's tab");
        tab.view.reading = Some(stats);
    }
    let tab = state.active_tab().expect("the file's tab");
    let content = card_content(tab);
    assert_eq!(content.name, "A note.md", "the full file name, extension included");
    assert_eq!(content.location, folder.display().to_string());
    assert_eq!(content.meta.as_deref(), Some("2 min read \u{b7} 440 words"));

    let tab = state.active_tab_mut().expect("the file's tab");
    tab.view.dirty = true;
    assert_eq!(
        card_content(state.active_tab().expect("tab")).meta.as_deref(),
        Some("Unsaved changes"),
        "a dirty tab says so instead of the time (T-16)"
    );
}

#[test]
fn pasted_and_start_tabs_say_what_they_are() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let pasted = state.open_synthetic("Pasted notes", "words\n");
    state.activate_index(0);
    let _ = pasted;
    let tab = state.tab(pasted).expect("the pasted tab");
    let content = card_content(tab);
    assert_eq!(content.name, "Pasted notes", "the title the text was given");
    assert_eq!(content.location, "Pasted text, in no file yet");
    assert_eq!(content.meta, None, "no reading time until the model has been counted");

    let start = Tab::start(99);
    let content = card_content(&start);
    assert_eq!(content.name, "New tab");
    assert_eq!(content.location, "Start page with the recent files");
    assert_eq!(content.meta, None, "the start page has nothing to read");
}

#[test]
fn the_card_hides_on_leave_switch_menus_palette_and_edit_mode() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "one\n");
    let second = state.open_synthetic("Second", "two\n");
    let mut tx = Tx::default();

    assert!(state.show_tab_card_now(first));
    assert_eq!(state.tab_card().shown(), Some(first));
    hover(&mut state, first, false, &mut tx);
    assert_eq!(state.tab_card().shown(), None, "leaving the tab hides it");

    assert!(state.show_tab_card_now(first));
    state.activate_index(1);
    assert_eq!(state.tab_card().shown(), Some(first), "opening a second tab did not move it");
    assert!(state.activate_index(0));
    assert_eq!(state.tab_card().shown(), None, "a tab switch hides it");

    assert!(state.show_tab_card_now(first));
    state.show_tab_menu(second, PointerPress { x: 40.0, y: 30.0 });
    assert!(state.poll(1.0, &mut tx));
    assert_eq!(state.tab_card().shown(), None, "the tab menu hides it");
    state.close_tab_menu();

    assert!(state.show_tab_card_now(first));
    state.show_browser_menu(std::path::PathBuf::from("/notes/a.md"), false, PointerPress { x: 40.0, y: 30.0 });
    assert!(state.poll(1.0, &mut tx));
    assert_eq!(state.tab_card().shown(), None, "the file menu hides it");
    state.close_browser_menu();

    assert!(state.show_tab_card_now(first));
    state.open_palette(None, &mut tx);
    assert!(state.poll(1.0, &mut tx));
    assert_eq!(state.tab_card().shown(), None, "the palette hides it");
    state.close_palette();

    assert!(state.show_tab_card_now(first));
    state.activate_index(1);
    state.active_tab_mut().expect("active").view.mode = Mode::Editor;
    assert!(state.show_tab_card_now(first), "the scene can still ask for one");
    assert!(state.poll(1.0, &mut tx));
    assert_eq!(state.tab_card().shown(), None, "edit mode hides it");

    // A hidden card leaves nothing scheduled: no timer wakes an idle window.
    assert_eq!(state.tab_card().shown(), None);
}

#[test]
fn the_card_is_a_floating_layer_under_its_tab_with_its_text() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("Reading notes", "one\n");
    state.open_synthetic("Other", "two\n");
    {
        let tab = state.tabs.iter_mut().find(|tab| tab.id == first).expect("the tab");
        tab.view.reading = Some(ReadingStats { words: 240, cjk: 0, minutes: 2 });
    }
    assert!(state.show_tab_card_now(first));
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    let elements = runtime.elements();

    let card = elements
        .iter()
        .find(|element| element.key.as_deref() == Some(crate::tab_card::TAB_CARD_KEY))
        .expect("the card is drawn");
    assert!(card.rect[2] > 0.0 && card.rect[3] > 0.0, "it has a real surface: {card:?}");
    for expected in ["Reading notes", "Pasted text, in no file yet", "2 min read \u{b7} 240 words"] {
        assert!(
            elements.iter().any(|element| element.text.as_deref() == Some(expected)),
            "the card says {expected:?}: {elements:#?}"
        );
    }
    let (tab, _) = runtime.presented(&format!("t-{}", tab_key(first))).expect("the tab's slot");
    assert!(card.rect[1] >= tab[1] + tab[3], "the card floats below its tab: {card:?} under {tab:?}");
    assert!(card.rect[0] >= tab[0] - 0.5, "and starts at it, clamped inside the window");
}

#[test]
fn a_drag_hides_the_card_and_the_time_while_a_pointer_close_freezes_the_widths() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Doc", "# Title\n\nwords here\n");
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    assert!(state.show_tab_card_now(tab));

    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    assert!(runtime.state.active_tab().is_some_and(|tab| tab.view.reading_meta.get()), "the time shows");

    // A pointer press on the active tab keeps the card; lifting the tab hides both.
    let (x, y) = runtime.find(&format!("t-{}", tab_key(tab))).expect("the tab");
    runtime.pointer_move(x, y);
    runtime.frame(0.0);
    runtime.pointer_down(x, y);
    runtime.frame(0.0);
    runtime.pointer_move(x + 48.0, y);
    runtime.frame(0.0);
    assert!(
        matches!(runtime.state.drag.phase(), rune_kit::shell::TabDragPhase::Dragging(_)),
        "the tab is in hand: {:?}",
        runtime.state.drag.phase()
    );
    assert!(runtime.state.poll(1.0, &mut tx));
    assert_eq!(runtime.state.tab_card().shown(), None, "no card while a tab is dragged (T-35)");
    runtime.frame(1.0);
    assert!(
        !runtime.state.active_tab().is_some_and(|tab| tab.view.reading_meta.get()),
        "and no inline time while it is in hand"
    );
    runtime.pointer_up(x + 48.0, y);
    runtime.frame(1.0);

    // A pointer close freezes the tabs' widths; the time waits for the pointer
    // to leave the strip, then comes back without a reflow (T-35).
    runtime.update(|state: &mut WindowState, _: &mut Tx| state.drag.width_lock = Some(180.0));
    runtime.frame(1.0);
    assert!(
        !runtime.state.active_tab().is_some_and(|tab| tab.view.reading_meta.get()),
        "the frozen width hides the time"
    );
    runtime.update(|state: &mut WindowState, _: &mut Tx| state.drag.width_lock = None);
    runtime.frame(1.0);
    runtime.frame(1.0);
    assert!(
        runtime.state.active_tab().is_some_and(|tab| tab.view.reading_meta.get()),
        "and it returns once the widths are free"
    );
}

#[test]
fn a_hover_does_not_change_the_tabs_width() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Doc", "# Title\n\nwords here\n");
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    let key = format!("t-{}", tab_key(tab));
    let before = runtime.presented(&key).expect("the tab").0;

    let (x, y) = runtime.find(&key).expect("the tab");
    runtime.pointer_move(x, y);
    runtime.frame(0.0);
    runtime.frame(0.0);
    let after = runtime.presented(&key).expect("the tab").0;
    assert_eq!(before, after, "hovering changes no width (T-35): {before:?} -> {after:?}");
    assert!(
        runtime.state.active_tab().is_some_and(|tab| tab.view.reading_meta.get()),
        "the time stays drawn while the pointer rests on the tab"
    );
}

#[test]
fn a_real_hover_sets_the_delay_and_a_leave_hides_the_card() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Hovered", "# Title\n\nwords here\n");
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.frame(0.0);

    let (x, y) = runtime.find(&format!("t-{}", tab_key(tab))).expect("the tab");
    runtime.pointer_move(x, y);
    runtime.frame(0.0);
    assert_eq!(runtime.state.tab_card().hovered(), Some(tab), "the strip's hover reached the card");
    assert_eq!(runtime.state.tab_card().shown(), None, "not before the pause");

    runtime.frame(0.3);
    assert_eq!(runtime.state.tab_card().shown(), None, "300 ms is not enough");
    runtime.frame(0.3);
    assert_eq!(runtime.state.tab_card().shown(), Some(tab), "550 ms of rest shows it");
    let elements = runtime.elements();
    assert!(
        elements.iter().any(|element| element.key.as_deref() == Some(crate::tab_card::TAB_CARD_KEY)),
        "the card is drawn"
    );

    runtime.pointer_move(20.0, 500.0);
    runtime.frame(0.0);
    assert_eq!(runtime.state.tab_card().shown(), None, "leaving the tab hides it");
    assert!(
        runtime
            .elements()
            .iter()
            .all(|element| element.key.as_deref() != Some(crate::tab_card::TAB_CARD_KEY) || element.exiting),
        "and the layer no longer shows it (a fading exit is allowed)"
    );
}
