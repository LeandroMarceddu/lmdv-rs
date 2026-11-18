//! The settings dialog's view: one modal over the window with an Appearance,
//! an Editor and a General section, a reset confirmation and a close button.
//!
//! Every control reads the live [`lmdv_core::Prefs`] and every change calls one
//! of [`crate::settings`]'s actions, so the dialog, the window behind it and
//! every other open window show the same values after the next frame. The
//! modal is Rune's: the scrim, `role=dialog`, focus trap and focus return are
//! the same pieces the command palette uses, and Escape backs out one layer at
//! a time.

use rune::{Align, Color, Cursor, Cx, El, ScrollSpec, Shadow, TextStyle, Tx, col, el, row, text};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit, Select, Switch};

use crate::icons;
use crate::settings::{
    COLUMN_WIDTHS, LARGE_FILE_MODES, LINE_HEIGHTS, READER_FONTS, SETTINGS_APPEARANCE, SETTINGS_BODY, SETTINGS_BREAK_H1,
    SETTINGS_CLOSE, SETTINGS_CLOSE_FOOTER, SETTINGS_EDITOR, SETTINGS_FONT, SETTINGS_FORMAT_BAR, SETTINGS_GENERAL,
    SETTINGS_KEY, SETTINGS_LANGUAGE, SETTINGS_LANGUAGE_TRIGGER, SETTINGS_LARGE_FILE, SETTINGS_PANEL, SETTINGS_PANELS,
    SETTINGS_RESET, SETTINGS_RESET_CANCEL, SETTINGS_RESET_CONFIRM, SETTINGS_RESTORE, SETTINGS_SCRIM, SETTINGS_SIZE,
    SETTINGS_SPACING, SETTINGS_SPELLCHECK, SETTINGS_THEME, SETTINGS_TITLE, SETTINGS_WIDTH, SIDE_SIZES, THEMES,
    column_width_label, font_label, large_file_label, line_height_label, side_size_label, theme_index, theme_label,
};
use crate::window::WindowState;

/// The panel's widest.
pub const SETTINGS_WIDTH_PTS: f32 = 600.0;
/// The narrowest a panel may get when the window is small.
pub const SETTINGS_MIN_WIDTH: f32 = 380.0;
/// The body's tallest before it scrolls.
pub const SETTINGS_BODY_MAX: f32 = 680.0;

/// The modal layer.
pub(crate) fn overlay(state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit, layer: &mut Vec<El>) {
    if !state.settings_is_open() {
        return;
    }
    let [width, height] = cx.layout_size("window-body").unwrap_or([1100.0, 760.0]);
    let panel_width = (width - 48.0).clamp(SETTINGS_MIN_WIDTH, SETTINGS_WIDTH_PTS);
    let body_max = (height - 200.0).clamp(120.0, SETTINGS_BODY_MAX);

    let scrim = el()
        .key(SETTINGS_SCRIM)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(kit.c.scrim.mix(Color::BLACK, 0.45))
        .a11y_hidden()
        .on_press(|state: &mut WindowState| state.close_settings());

    // The language listbox is a Rune modal of its own and must draw above the
    // panel, so it collects into its own layer appended after the root.
    let mut popups = Vec::new();
    let panel = panel(state, kit, panel_width, body_max, &mut popups);

    let centred =
        col().key("settings-layer").absolute(0.0, 0.0).w_full().h_full().py(40.0).px(24.0).items_center().child(panel);
    let root = el()
        .key(SETTINGS_KEY)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .label(lmdv_i18n::t!("set.title"))
        .focusable(true)
        .no_focus_ring()
        .modal_tx(false, |state: &mut WindowState, _: &mut Tx| state.close_settings())
        .on_key(|state: &mut WindowState, event, tx: &mut Tx| state.settings_key(event, tx))
        .child(scrim)
        .child(centred);
    layer.push(root);
    layer.extend(popups);
}

/// The dialog panel: title, scrolling sections, footer.
fn panel(state: &WindowState, kit: &Kit, width: f32, body_max: f32, popups: &mut Vec<El>) -> El {
    let c = &kit.c;
    let header = row()
        .key("settings-header")
        .w_full()
        .h(52.0)
        .px(20.0)
        .gap(10.0)
        .center_y()
        .child(
            text(lmdv_i18n::t!("set.title"))
                .key(SETTINGS_TITLE)
                .role("heading")
                .level(1)
                .font(TextStyle { line_height: 22.0, ..kit.ty.ui_medium })
                .color(c.text_base)
                .grow(1.0),
        )
        .child(
            IconButton::new(kit.glyph.close)
                .variant(IconButtonVariant::GhostMuted)
                .label(lmdv_i18n::t!("set.close"))
                .help(lmdv_i18n::t!("set.closeButton"))
                .on_press(|state: &mut WindowState| state.close_settings())
                .build(kit)
                .key(SETTINGS_CLOSE),
        );

    let body = col()
        .key(SETTINGS_BODY)
        .w_full()
        .max_h(body_max)
        .min_h(0.0)
        .px(20.0)
        .pb(10.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(section_header(SETTINGS_APPEARANCE, lmdv_i18n::t!("set.appearance"), kit))
        .child(appearance_rows(state, kit))
        .child(section_header(SETTINGS_EDITOR, lmdv_i18n::t!("set.editor"), kit))
        .child(editor_rows(state, kit))
        .child(section_header(SETTINGS_GENERAL, lmdv_i18n::t!("set.general"), kit))
        .child(general_rows(state, kit, popups));

    col()
        .key(SETTINGS_PANEL)
        .role("dialog")
        .label(lmdv_i18n::t!("set.title"))
        .w(width)
        .radius(10.0)
        .fill(c.layer_01)
        .border(1.0, c.border_strong)
        .shadow(12.0, 40.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.30 })
        .preserve_focus_on_press(true)
        .child(header)
        .child(el().key("settings-rule").w_full().h(1.0).fill(c.border_muted))
        .child(body)
        .child(el().key("settings-rule-2").w_full().h(1.0).fill(c.border_muted))
        .child(footer(state, kit))
}

/// A section's heading.
fn section_header(key: &str, title: &str, kit: &Kit) -> El {
    text(title)
        .key(key)
        .role("heading")
        .level(2)
        .font(kit.ty.small_medium)
        .color(kit.c.text_faint)
        .w_full()
        .pt(12.0)
        .pb(2.0)
}

/// The Appearance section: theme, text size, font, spacing, column width and
/// side-panel size (P-01/R-02).
fn appearance_rows(state: &WindowState, kit: &Kit) -> El {
    let prefs = state.shared().prefs();
    let theme_labels: Vec<String> = THEMES.iter().copied().map(theme_label).map(str::to_owned).collect();
    let theme = segmented(
        kit,
        SETTINGS_THEME,
        lmdv_i18n::t!("rs.theme"),
        &theme_labels,
        theme_index(prefs.theme),
        true,
        |state: &mut WindowState, index: usize| state.set_theme(THEMES[index]),
    );

    let size = row()
        .key(SETTINGS_SIZE)
        .role("group")
        .label(lmdv_i18n::t!("keys.textSize"))
        .gap(6.0)
        .center_y()
        .child(
            IconButton::new(&icons::MINUS)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .disabled(!state.can_shrink_reader())
                .label(lmdv_i18n::t!("rs.smaller"))
                .help(lmdv_i18n::t!("rs.smaller"))
                .on_press(|state: &mut WindowState| state.bump_reader_size(-1))
                .build(kit)
                .key(format!("{SETTINGS_SIZE}-down")),
        )
        .child(
            text(format!("{} px", lmdv_i18n::number(i64::from(prefs.reader.size))))
                .key(format!("{SETTINGS_SIZE}-value"))
                .role("spinbutton")
                .label(lmdv_i18n::t!("keys.textSize"))
                .range_value(f64::from(prefs.reader.size), 13.0, 22.0)
                .range_step(1.0)
                .focusable(true)
                .focus_ring(2.0, 1.0, kit.c.border_focus)
                .on_step(|state: &mut WindowState, delta: i32, _: &mut Tx| state.bump_reader_size(delta.signum()))
                .on_step_limit(|state: &mut WindowState, max: bool, _: &mut Tx| {
                    state.set_reader_size(if max { 22 } else { 13 });
                })
                .font(kit.ty.ui_medium)
                .color(kit.c.text_base)
                .min_w(58.0)
                .align(Align::Center),
        )
        .child(
            IconButton::new(&icons::PLUS)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .disabled(!state.can_grow_reader())
                .label(lmdv_i18n::t!("rs.larger"))
                .help(lmdv_i18n::t!("rs.larger"))
                .on_press(|state: &mut WindowState| state.bump_reader_size(1))
                .build(kit)
                .key(format!("{SETTINGS_SIZE}-up")),
        );

    let font_labels: Vec<String> = READER_FONTS.iter().copied().map(font_label).map(str::to_owned).collect();
    let font = segmented(
        kit,
        SETTINGS_FONT,
        lmdv_i18n::t!("rs.font"),
        &font_labels,
        READER_FONTS.iter().position(|font| *font == prefs.reader.font).unwrap_or(0),
        false,
        |state: &mut WindowState, index: usize| state.set_reader_font(READER_FONTS[index]),
    );

    let line_labels: Vec<String> = LINE_HEIGHTS.iter().copied().map(line_height_label).map(str::to_owned).collect();
    let spacing = segmented(
        kit,
        SETTINGS_SPACING,
        lmdv_i18n::t!("rs.spacing"),
        &line_labels,
        LINE_HEIGHTS.iter().position(|line| *line == prefs.reader.line_height).unwrap_or(1),
        false,
        |state: &mut WindowState, index: usize| state.set_line_height(LINE_HEIGHTS[index]),
    );

    let width_labels: Vec<String> = COLUMN_WIDTHS.iter().copied().map(column_width_label).map(str::to_owned).collect();
    let width = segmented(
        kit,
        SETTINGS_WIDTH,
        lmdv_i18n::t!("rs.width"),
        &width_labels,
        COLUMN_WIDTHS.iter().position(|width| *width == prefs.reader.width).unwrap_or(1),
        false,
        |state: &mut WindowState, index: usize| state.set_column_width(COLUMN_WIDTHS[index]),
    );

    let side_labels: Vec<String> = SIDE_SIZES.iter().copied().map(side_size_label).map(str::to_owned).collect();
    let panels = segmented(
        kit,
        SETTINGS_PANELS,
        lmdv_i18n::t!("rs.panels"),
        &side_labels,
        SIDE_SIZES.iter().position(|size| *size == prefs.reader.side_size).unwrap_or(1),
        false,
        |state: &mut WindowState, index: usize| state.set_side_size(SIDE_SIZES[index]),
    );

    col()
        .w_full()
        .child(setting_row("settings-row-theme", lmdv_i18n::t!("rs.theme"), None, theme, kit))
        .child(setting_row(
            "settings-row-size",
            lmdv_i18n::t!("keys.textSize"),
            Some(lmdv_i18n::t!("set.sizeDetail")),
            size,
            kit,
        ))
        .child(setting_row("settings-row-font", lmdv_i18n::t!("rs.font"), None, font, kit))
        .child(setting_row("settings-row-spacing", lmdv_i18n::t!("rs.spacing"), None, spacing, kit))
        .child(setting_row(
            "settings-row-width",
            lmdv_i18n::t!("rs.width"),
            Some(lmdv_i18n::t!("set.widthDetail")),
            width,
            kit,
        ))
        .child(setting_row(
            "settings-row-panels",
            lmdv_i18n::t!("rs.panels"),
            Some(lmdv_i18n::t!("set.panelsDetail")),
            panels,
            kit,
        ))
}

/// The Editor section: large-file mode, format bar and the spellcheck toggle.
fn editor_rows(state: &WindowState, kit: &Kit) -> El {
    let prefs = state.shared().prefs();
    let mode_labels: Vec<String> = LARGE_FILE_MODES.iter().copied().map(large_file_label).map(str::to_owned).collect();
    let mode = segmented(
        kit,
        SETTINGS_LARGE_FILE,
        lmdv_i18n::t!("set.largeFile"),
        &mode_labels,
        LARGE_FILE_MODES.iter().position(|mode| *mode == prefs.editor.large_file_mode).unwrap_or(0),
        false,
        |state: &mut WindowState, index: usize| state.set_large_file_mode(LARGE_FILE_MODES[index]),
    );
    let format_bar = Switch::new(prefs.editor.format_bar)
        .label(lmdv_i18n::t!("set.formatBar"))
        .hide_label()
        .on_toggle(|state: &mut WindowState| {
            let shown = state.shared().prefs().editor.format_bar;
            state.set_format_bar(!shown);
        })
        .build(kit)
        .key(SETTINGS_FORMAT_BAR);

    let mut rows = col()
        .w_full()
        .child(setting_row(
            "settings-row-large-file",
            lmdv_i18n::t!("set.largeFile"),
            Some(lmdv_i18n::t!("set.largeFileDetail")),
            mode,
            kit,
        ))
        .child(setting_row(
            "settings-row-format-bar",
            lmdv_i18n::t!("set.formatBar"),
            Some(lmdv_i18n::t!("set.formatBarDetail")),
            format_bar,
            kit,
        ));
    // E-SPELL: without a dictionary for the launch's locale the row does not
    // exist at all — no disabled switch, no explanation string.
    if let Some(availability) = state.spell_availability() {
        let language = availability.language();
        let detail = if availability.fallback {
            lmdv_i18n::t!("set.spellcheckFallback", "dictionary" => language)
        } else {
            lmdv_i18n::t!("set.spellcheckDetail", "dictionary" => language)
        };
        let spellcheck = Switch::new(prefs.editor.spellcheck)
            .label(lmdv_i18n::t!("cmd.spell"))
            .hide_label()
            .on_toggle(|state: &mut WindowState| {
                let on = state.shared().prefs().editor.spellcheck;
                state.set_spellcheck(!on);
            })
            .build(kit)
            .key(SETTINGS_SPELLCHECK);
        rows = rows.child(setting_row(
            "settings-row-spellcheck",
            lmdv_i18n::t!("cmd.spell"),
            Some(&detail),
            spellcheck,
            kit,
        ));
    }
    rows
}

/// The General section: language, session restore and the H1 page break.
fn general_rows(state: &WindowState, kit: &Kit, popups: &mut Vec<El>) -> El {
    let prefs = state.shared().prefs();
    let options = crate::settings::language_options();
    // Group the options into flat label strings for the kit's listbox.
    let labels: Vec<String> = options.iter().map(|option| option.label.clone()).collect();
    let current = state.language_current();
    let mut select = Select::new(SETTINGS_LANGUAGE, &labels)
        .label(lmdv_i18n::t!("rs.language"))
        .current(Some(current.as_str()))
        .width(200.0)
        // The row sits near the dialog's bottom; the list opens upward so it
        // stays inside a short window.
        .placement(rune_kit::Placement::TopEnd)
        .open(state.settings.language_open)
        .on_toggle(|state: &mut WindowState| {
            state.settings.language_open = !state.settings.language_open;
        })
        .on_close(|state: &mut WindowState| state.settings.language_open = false)
        .on_choose(|state: &mut WindowState, label: &str, _: &mut Tx| {
            state.set_language_label(label);
        });
    select = select.appearance(rune_kit::SelectAppearance::Field);
    let language = select.build(kit, popups).key(SETTINGS_LANGUAGE_TRIGGER);

    let restore = Switch::new(prefs.restore_session)
        .label(lmdv_i18n::t!("set.restore"))
        .hide_label()
        .on_toggle(|state: &mut WindowState| {
            let on = state.shared().prefs().restore_session;
            state.set_restore_session(!on);
        })
        .build(kit)
        .key(SETTINGS_RESTORE);

    let break_h1 = Switch::new(prefs.print_break_h1)
        .label(lmdv_i18n::t!("share.breakH1"))
        .hide_label()
        .on_toggle(|state: &mut WindowState| {
            let on = state.shared().prefs().print_break_h1;
            state.set_print_break_h1(!on);
        })
        .build(kit)
        .key(SETTINGS_BREAK_H1);

    col()
        .w_full()
        .child(setting_row(
            "settings-row-language",
            lmdv_i18n::t!("rs.language"),
            Some(lmdv_i18n::t!("set.languageDetail")),
            language,
            kit,
        ))
        .child(setting_row(
            "settings-row-restore",
            lmdv_i18n::t!("set.restore"),
            Some(lmdv_i18n::t!("set.restoreDetail")),
            restore,
            kit,
        ))
        .child(setting_row(
            "settings-row-break-h1",
            lmdv_i18n::t!("share.breakH1"),
            Some(lmdv_i18n::t!("set.breakH1Detail")),
            break_h1,
            kit,
        ))
}

/// The footer: reset (with its confirmation) and close.
fn footer(state: &WindowState, kit: &Kit) -> El {
    let c = &kit.c;
    let mut bar = row().key("settings-footer").w_full().px(16.0).py(10.0).gap(8.0).center_y();
    if state.settings.confirm_reset {
        bar = bar
            .child(
                text(lmdv_i18n::t!("set.resetQuestion"))
                    .key("settings-reset-question")
                    .font(kit.ty.small)
                    .color(c.text_muted)
                    .wrap()
                    .grow(1.0),
            )
            .child(
                rune_kit::Button::new(lmdv_i18n::t!("set.resetConfirm"))
                    .variant(rune_kit::ButtonVariant::Danger)
                    .on_press(|state: &mut WindowState| state.reset_settings())
                    .build(kit)
                    .key(SETTINGS_RESET_CONFIRM),
            )
            .child(
                rune_kit::Button::new(lmdv_i18n::t!("msg.cancel"))
                    .variant(rune_kit::ButtonVariant::Outline)
                    .on_press(|state: &mut WindowState| state.cancel_reset_settings())
                    .build(kit)
                    .key(SETTINGS_RESET_CANCEL),
            );
    } else {
        bar = bar
            .child(
                rune_kit::Button::new(lmdv_i18n::t!("set.reset"))
                    .variant(rune_kit::ButtonVariant::Outline)
                    .disabled(state.prefs_are_default())
                    .on_press(|state: &mut WindowState| state.request_reset_settings())
                    .build(kit)
                    .key(SETTINGS_RESET),
            )
            .child(row().grow(1.0))
            .child(
                rune_kit::Button::new(lmdv_i18n::t!("set.closeButton"))
                    .variant(rune_kit::ButtonVariant::Contrast)
                    .on_press(|state: &mut WindowState| state.close_settings())
                    .build(kit)
                    .key(SETTINGS_CLOSE_FOOTER),
            );
    }
    bar
}

/// One labelled setting: the name and its explanation on the left, the
/// control on the right.
fn setting_row(key: &str, label: &str, detail: Option<&str>, control: El, kit: &Kit) -> El {
    let mut name = col().grow(1.0).min_w(0.0).gap(1.0).child(
        text(label.to_owned())
            .key("settings-label")
            .font(TextStyle { line_height: 18.0, ..kit.ty.ui })
            .color(kit.c.text_base),
    );
    if let Some(detail) = detail {
        name = name.child(
            text(detail.to_owned())
                .key("settings-detail")
                .font(kit.ty.small.lh(15.0))
                .color(kit.c.text_faint)
                .wrap()
                .max_lines(2),
        );
    }
    let mut line = row().key(key).w_full().py(6.0).gap(16.0).center_y().child(name).child(control.shrink());
    if let Some(detail) = detail {
        line = line.description(detail.to_owned());
    }
    line
}

/// A local segmented control with unique keys, the kit's look and the radio
/// group semantics the kit's own control carries.
///
/// The kit hard-codes `segment{i}` keys inside every instance; a settings page
/// has six of them, and scripts address controls by key, so this page keys each
/// choice `{prefix}-{i}` instead.
fn segmented<S: 'static>(
    kit: &Kit,
    prefix: &str,
    name: &str,
    labels: &[String],
    selected: usize,
    autofocus: bool,
    pick: impl Fn(&mut S, usize) + Clone + 'static,
) -> El {
    let c = &kit.c;
    let count = labels.len().max(1);
    let font = TextStyle { line_height: 13.0, ..kit.ty.ui };
    let mut root = row()
        .key(prefix)
        .role("radiogroup")
        .label(name)
        .focus_group()
        .h(28.0)
        .center_y()
        .radius(6.0)
        .fill(c.layer_01)
        .box_shadow(Shadow { spread: 0.5, color: c.border_base, ..Default::default() })
        .padding(2.0)
        .gap(2.0);
    for (index, label) in labels.iter().enumerate() {
        let on = selected == index;
        let mut segment = row()
            .key(format!("{prefix}-{index}"))
            .role("radio")
            .label(label.clone())
            .position_in_set(index + 1, count)
            .focusable(true)
            .focus_selected(on)
            .h(24.0)
            .px(10.0)
            .center_y()
            .radius(4.0)
            .cursor(Cursor::Pointer)
            .hit();
        segment = if on {
            segment.fill(c.bg_base).box_shadow(Shadow { spread: 0.5, color: c.border_strong, ..Default::default() })
        } else {
            segment.hover(c.hover).press(c.pressed)
        };
        if autofocus && on {
            segment = segment.autofocus();
        }
        let pick = pick.clone();
        segment = segment
            .child(text(label.clone()).font(font).color(if on { c.text_base } else { c.text_muted }).a11y_hidden())
            .on_press_tx(move |state: &mut S, _: &mut Tx| pick(state, index));
        root = root.child(segment);
    }
    root
}

/// The `settings` headless scene: tabs behind an open dialog with the classic
/// defaults, so a snapshot and tree show every section and control.
pub fn scene_settings(state: &mut WindowState) {
    crate::view::scene_tabs(state);
    state.left.set_open(false);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    state.sync_outline();
    state.ui.notice = None;
    state.ui.help = None;
    let mut tx = Tx::default();
    state.open_settings(&mut tx);
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn the_scene_opens_the_dialog_over_a_document() {
        let temp = TempDir::new().unwrap();
        let mut state = WindowState::new(crate::test_support::shared(&temp));
        scene_settings(&mut state);
        assert!(state.settings_is_open());
        assert!(state.active_document_id().is_some(), "the tabs scene leaves a document in front");
        assert!(!state.left.is_open() && !state.right.is_open());
    }
}
