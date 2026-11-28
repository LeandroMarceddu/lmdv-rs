//! The command palette's modal view (C-12) and its headless scene.
//!
//! A scrim over the window and a panel near the top: a search field
//! (`role=combobox`, `aria-expanded`, `aria-activedescendant`,
//! `aria-describedby`), the list (`role=listbox`) of labelled groups
//! (`role=group`) whose rows are options (`role=option`, `aria-selected`), and a
//! footer with the keys' help and the live result count (`role=status`,
//! announced 300 ms after the last key). Choosing a row closes the palette
//! first, so what the action focuses keeps the keyboard.
//!
//! C-12's `aria-autocomplete="list"` and `aria-controls` are set on the field
//! with Rune's `El::auto_complete(AutoComplete::List)` and
//! `El::controls(PALETTE_LIST_KEY)`, so the combobox says it filters a popup
//! list and names that list. The field still carries `aria-expanded` and
//! `aria-activedescendant`; controls follow the list's presence, as expanded
//! does.

use rune::{
    AutoComplete, Color, Cursor, Cx, El, LiveRegion, Span, TextStyle, col, el, icon, rich, row, text, text_input,
};
use rune_kit::{Keybind, Kit};

use crate::fuzzy;
use crate::icons;
use crate::palette::{
    PALETTE_GROUP_HEIGHT, PALETTE_GROUP_PREFIX, PALETTE_HELP_KEY, PALETTE_INPUT_KEY, PALETTE_KEY, PALETTE_LIST_KEY,
    PALETTE_OPTION_PREFIX, PALETTE_PANEL, PALETTE_ROW_HEIGHT, PALETTE_STATUS_KEY, PaletteMode, PaletteState,
};
use crate::window::WindowState;

/// The panel's width, at most.
pub const PALETTE_WIDTH: f32 = 640.0;
/// The panel's distance from the window's top.
pub const PALETTE_TOP: f32 = 64.0;
/// The `#foo` outline's row indent per level, in points.
pub const PALETTE_INDENT: f32 = 14.0;

/// The palette's modal layer, over the whole window (C-12).
pub(crate) fn overlay(state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit, layer: &mut Vec<El>) {
    if !state.palette_is_open() {
        return;
    }
    let [width, height] = cx.layout_size("window-body").unwrap_or([1100.0, 760.0]);
    state.palette.viewport.set([width, height]);
    let panel_width = (width - 48.0).clamp(320.0, PALETTE_WIDTH);
    let list_max = (height - PALETTE_TOP - 170.0).clamp(120.0, 520.0);

    let scrim = el()
        .key("palette-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(kit.c.scrim.mix(Color::BLACK, 0.45))
        .a11y_hidden()
        .on_press(|state: &mut WindowState| state.close_palette());

    let panel = palette_panel(state, kit, panel_width, list_max);
    let centred = col()
        .key("palette-layer")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .pt(PALETTE_TOP)
        .px(24.0)
        .items_center()
        .child(panel);
    let root = el()
        .key(PALETTE_KEY)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .label(lmdv_i18n::t!("pal.title"))
        .focusable(true)
        .no_focus_ring()
        .modal_tx(false, |state: &mut WindowState, _: &mut rune::Tx| state.close_palette())
        .on_key(|state: &mut WindowState, event, tx| state.palette_key(event, tx))
        .child(scrim)
        .child(centred);
    layer.push(root);
}

/// The panel: the search field, the list and the footer.
fn palette_panel(state: &WindowState, kit: &Kit, width: f32, list_max: f32) -> El {
    let c = &kit.c;
    let palette = &state.palette;
    let active_key = palette.active.filter(|active| palette.entry(*active).is_some()).map(option_key);

    let field = text_input(&palette.query, kit.ty.ui)
        .key(PALETTE_INPUT_KEY)
        .single_line()
        .role("combobox")
        .label(lmdv_i18n::t!("pal.title"))
        .description(lmdv_i18n::t!("pal.help"))
        .described_by(PALETTE_HELP_KEY)
        .auto_complete(AutoComplete::List)
        .expanded(!palette.is_empty())
        .controls(if palette.is_empty() { "" } else { PALETTE_LIST_KEY })
        .placeholder(lmdv_i18n::t!("pal.placeholder"), c.text_faint)
        .caret(c.text_base, kit.selection())
        .w_full()
        .on_edit(|state: &mut WindowState| &mut state.palette.query)
        .on_input_change(|state: &mut WindowState, tx| state.palette_query_changed(tx))
        .on_input_key(|state: &mut WindowState, event, tx| state.palette_key(event, tx));
    let field = match active_key {
        Some(key) => field.active_descendant(key),
        None => field,
    };

    let head = row()
        .key("palette-head")
        .w_full()
        .px(14.0)
        .py(10.0)
        .gap(10.0)
        .center_y()
        .child(icon(&icons::SEARCH, 16.0).color(c.icon_muted))
        .child(field);

    let rows_height = palette
        .view
        .groups
        .iter()
        .map(|group| group.len as f32 * PALETTE_ROW_HEIGHT + PALETTE_GROUP_HEIGHT)
        .sum::<f32>()
        .max(PALETTE_ROW_HEIGHT);
    let body_height = rows_height.min(list_max);

    let body: El = if palette.is_empty() {
        col().key("palette-empty-row").w_full().h(body_height).px(14.0).py(10.0).child(
            text(if palette.view.empty.is_empty() {
                lmdv_i18n::t!("pal.results")
            } else {
                palette.view.empty.as_str()
            })
            .key("palette-empty")
            .font(kit.ty.ui)
            .color(c.text_faint),
        )
    } else {
        palette_list(palette, kit, body_height)
    };

    // The footer spells the keys as keycaps (the kit's Keybind), not as a
    // sentence; the full sentence stays the field's accessible description.
    let foot = |keys: &[&str], label: String, kit: &Kit| {
        row()
            .gap(4.0)
            .center_y()
            .shrink()
            .child(Keybind::new(keys).build(kit))
            .child(text(label).font(kit.ty.small).color(c.text_faint).shrink())
    };
    let dot = || text("·").font(kit.ty.small).color(c.text_faint);
    let help = row()
        .key(PALETTE_HELP_KEY)
        .role("group")
        .label(lmdv_i18n::t!("pal.help"))
        .gap(6.0)
        .center_y()
        .grow(1.0)
        .min_w(0.0)
        .child(foot(&["↑", "↓"], lmdv_i18n::t!("pal.footChoose").to_owned(), kit))
        .child(dot())
        .child(foot(&["Enter"], lmdv_i18n::t!("pal.footOpen").to_owned(), kit))
        .child(dot())
        .child(foot(&["Ctrl", "Enter"], lmdv_i18n::t!("pal.footNewTab").to_owned(), kit))
        .child(dot())
        .child(foot(&[">"], lmdv_i18n::t!("pal.footCommands").to_owned(), kit))
        .child(dot())
        .child(foot(&["#"], lmdv_i18n::t!("pal.footHeadings").to_owned(), kit));
    let status = text(palette.announce.clone())
        .key(PALETTE_STATUS_KEY)
        .role("status")
        .live_region(LiveRegion::Polite)
        .font(kit.ty.small)
        .color(c.text_muted)
        .shrink();
    let footer = row().key("palette-foot").w_full().px(14.0).py(6.0).gap(10.0).items_start().child(help).child(status);

    col()
        .key(PALETTE_PANEL)
        .role("dialog")
        .label(lmdv_i18n::t!("pal.title"))
        .w(width)
        .radius(10.0)
        .fill(c.layer_01)
        .border(1.0, c.border_strong)
        .shadow(12.0, 40.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.30 })
        .preserve_focus_on_press(true)
        .child(head)
        .child(el().key("palette-rule").w_full().h(1.0).fill(c.border_muted))
        .child(body)
        .child(el().key("palette-rule-2").w_full().h(1.0).fill(c.border_muted))
        .child(footer)
}

/// The scrolling list: a labelled group of options each (C-02/C-12).
fn palette_list(palette: &PaletteState, kit: &Kit, height: f32) -> El {
    let mut list = col()
        .key(PALETTE_LIST_KEY)
        .role("listbox")
        .label(lmdv_i18n::t!("pal.results"))
        .w_full()
        .h(height)
        .min_h(0.0)
        .py(4.0)
        .scroll(rune::ScrollSpec { stick_bottom: false });
    for (group_index, group) in palette.view.groups.iter().enumerate() {
        let title_key = format!("{PALETTE_GROUP_PREFIX}{group_index}");
        let group_el = col()
            .key(title_key.clone())
            .role("group")
            .label(group.title.clone())
            .w_full()
            .child(
                text(group.title.clone())
                    .font(kit.ty.small_medium)
                    .color(kit.c.text_faint)
                    .px(14.0)
                    .py(4.0)
                    .w_full()
                    .role("none"),
            )
            .children(group.rows().map(|row| palette_row(palette, kit, row)));
        list = list.child(group_el);
    }
    list
}

/// One option: its icon, label with the matched letters bold, tag, detail and
/// shortcut; the highlighted one is selected for assistive technology (C-12).
fn palette_row(palette: &PaletteState, kit: &Kit, index: usize) -> El {
    let Some(entry) = palette.entry(index) else {
        return el().key(format!("{PALETTE_OPTION_PREFIX}{index}")).h(PALETTE_ROW_HEIGHT);
    };
    let c = &kit.c;
    let active = palette.active == Some(index);
    let outline = palette.view.mode == PaletteMode::Outline;
    let mut accessible = entry.label.clone();
    if !entry.detail.is_empty() && !outline {
        accessible.push_str(", ");
        accessible.push_str(&entry.detail);
    }
    let mut option = row()
        .key(option_key(index))
        .role("option")
        .label(accessible)
        .w_full()
        .h(PALETTE_ROW_HEIGHT)
        .px(14.0)
        .gap(8.0)
        .center_y()
        .radius(kit.radius.sm)
        .focus_selected(active)
        .preserve_focus_on_press(true)
        .cursor(Cursor::Pointer)
        .hit()
        .hover(c.hover)
        .on_press_tx(move |state: &mut WindowState, tx: &mut rune::Tx| state.palette_row_pressed(index, tx))
        .on_pointer_move(move |state: &mut WindowState, point| state.palette_pointer_moved(index, point.x, point.y));
    // C-06/OI-147: a middle click on a file opens it in a tab behind, as the
    // original's `auxclick` does (`src/palette.ts:960-965`);
    // headings, tabs and commands have no middle-click action there.
    if entry.kind == crate::palette::EntryKind::File {
        option = option
            .on_middle_press(move |state: &mut WindowState, tx: &mut rune::Tx| state.palette_row_middle(index, tx));
    }
    if let Some(checked) = entry.checked {
        option = option.checked(checked);
    }
    if active {
        option = option.fill(c.layer_02);
    }
    if outline {
        option = option.pl(PALETTE_INDENT * f32::from(entry.depth));
    }

    // The kind's mark: a glyph for commands and headings, an icon for the rest.
    let mark: El = match entry.kind {
        crate::palette::EntryKind::Command => text("›").font(kit.ty.ui).color(c.icon_faint),
        crate::palette::EntryKind::Heading => text("#").font(kit.ty.mono).color(c.icon_faint),
        crate::palette::EntryKind::File => icon(&icons::DOCUMENT, 14.0).color(c.icon_muted),
        crate::palette::EntryKind::Tab => icon(&icons::DOCUMENT, 14.0).color(c.icon_muted),
        crate::palette::EntryKind::FindInDocument | crate::palette::EntryKind::SearchFolder => {
            icon(&icons::SEARCH, 14.0).color(c.icon_muted)
        }
    };

    let runs = fuzzy::runs(&entry.label, &entry.hits);
    let mut label: El = if runs.iter().any(|(_, hit)| *hit) {
        let spans: Vec<Span> =
            runs.iter().filter(|(_, hit)| *hit).map(|(range, _)| Span::new(range.start, range.end).bold()).collect();
        rich(entry.label.clone(), spans)
    } else {
        text(entry.label.clone())
    };
    label = label.font(kit.ty.ui).color(if active { c.text_base } else { c.text_muted }).truncate().shrink();

    let mut inner = row().key("palette-row-text").gap(6.0).center_y().grow(1.0).min_w(0.0).child(mark).child(label);
    if !entry.tag.is_empty() {
        inner =
            inner.child(text(entry.tag.clone()).key("palette-row-tag").font(kit.ty.small).color(c.text_faint).shrink());
    }
    if !entry.detail.is_empty() && !outline {
        inner = inner.child(
            text(entry.detail.clone())
                .key("palette-row-detail")
                .font(kit.ty.small)
                .color(c.text_faint)
                .truncate()
                .shrink(),
        );
    }
    option = option.child(inner);
    if entry.checked.is_some() {
        option = option.child(
            text(if entry.checked == Some(true) { "☑" } else { "☐" })
                .key("palette-row-check")
                .font(kit.ty.ui)
                .color(if entry.checked == Some(true) { c.text_accent } else { c.text_faint })
                .shrink(),
        );
    }
    if !entry.keys.is_empty() {
        option = option.child(
            text(crate::commands::chord_text(&entry.keys))
                .key("palette-row-keys")
                .font(TextStyle { size: 11.0, ..kit.ty.mono })
                .color(c.text_faint)
                .shrink(),
        );
    }
    option
}

/// A row's element key: its place in the drawn list. The highlight is an index
/// into that same list, so the key a change points at is the key it draws.
fn option_key(index: usize) -> String {
    format!("{PALETTE_OPTION_PREFIX}{index}")
}

// ---- the headless scene ---------------------------------------------------

/// The `palette` scene: three tabs, a folder of files and the palette open on a
/// typed query, so a snapshot shows every group, a matched label and the search
/// rows. `LMDV_BROWSER_ROOT` gives the files group its folder (as the browser
/// scene does).
pub fn scene_palette(state: &mut WindowState) {
    crate::view::scene_tabs(state);
    // One more document titled like the query, so the tabs group shows too.
    state.open_synthetic("Reading list", "# Reading list\n\nA second page.\n");
    if let Some(index) = state.tabs().iter().position(|tab| tab.full_title() == "Reading notes") {
        state.activate_index(index);
    }
    state.left.set_open(false);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    let mut tx = rune::Tx::default();
    if let Some(root) = std::env::var_os("LMDV_BROWSER_ROOT") {
        state.left.set_open(true);
        state.set_browser_root(std::path::PathBuf::from(root));
        // A headless scene lists its folder in one settled frame, so the Files
        // group is there when the palette builds its lists.
        state.poll_browser(0.0, &mut tx);
        state.left.set_open(false);
    }
    state.open_palette(Some(crate::palette::PaletteChord::CtrlK), &mut tx);
    state.set_palette_query("rea", &mut tx);
    state.ui.notice = None;
    state.ui.help = None;
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn the_scene_opens_the_palette_on_a_query() {
        let temp = TempDir::new().unwrap();
        let mut state = WindowState::new(crate::test_support::shared(&temp));
        scene_palette(&mut state);
        assert!(state.palette_is_open());
        assert_eq!(state.palette.query.text, "rea");
        assert!(!state.palette.is_empty(), "the query found something");
        assert_eq!(state.palette.view.mode, PaletteMode::All);
    }

    #[test]
    fn the_option_keys_follow_the_row_index() {
        assert_eq!(option_key(0), "palette-opt-0");
        assert_eq!(option_key(41), "palette-opt-41");
    }

    #[test]
    fn the_scene_draws_the_dialog_the_list_and_its_groups() {
        use rune::{Env, Runtime};
        let temp = TempDir::new().unwrap();
        let mut state = WindowState::new(crate::test_support::shared(&temp));
        scene_palette(&mut state);
        let mut runtime = Runtime::new(state, crate::view::view, (1100.0, 760.0), Env::new(true, 2.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        let elements = runtime.elements();
        let dialog =
            elements.iter().find(|element| element.role == "dialog" && element.key.as_deref() == Some(PALETTE_PANEL));
        assert!(dialog.is_some_and(|dialog| dialog.label.as_deref() == Some(lmdv_i18n::t!("pal.title"))));
        let list = elements.iter().find(|element| element.key.as_deref() == Some(PALETTE_LIST_KEY));
        assert_eq!(list.map(|list| list.role), Some("listbox"));
        let options = elements.iter().filter(|element| element.role == "option").count();
        assert!(options > 2, "the list drew its rows: {options}");
        let groups = elements.iter().filter(|element| element.role == "group").count();
        assert!(groups >= 2, "the list drew labelled groups: {groups}");
        assert_eq!(
            elements.iter().find(|element| element.key.as_deref() == Some(PALETTE_INPUT_KEY)).map(|field| field.role),
            Some("combobox")
        );
        let behind = elements.iter().find(|element| element.key.as_deref() == Some("open-file-toolbar"));
        assert!(behind.is_some_and(|button| button.inert), "the window behind the palette is inert");
    }
}
