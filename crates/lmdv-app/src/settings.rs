//! The settings dialog's model: the modal's state, every preference action and
//! the one funnel that persists a change.
//!
//! `WindowState` handlers call these methods; [`crate::settings_ui`] draws the
//! controls from the live [`Prefs`]. Every change goes through
//! [`WindowState::patch_prefs`], which stores the whole preference set through
//! [`AppShared::set_prefs`] (one coalesced `prefs.json` patch) and broadcasts
//! [`UiMsg::PrefsChanged`](crate::shared::UiMsg::PrefsChanged) so every open
//! window redraws with the new values.
//!
//! Nothing here touches the filesystem: persistence is the store writer's job
//! (ADR-017).

use lmdv_core::{
    ColumnWidth, DEFAULT_READER_SIZE, LargeFileMode, LineHeight, Prefs, ReaderFont, SIZE_STEPS, SideSize, ThemePref,
};
use lmdv_io::assoc::{AssociationError, Confirmation};
use rune::{Key, KeyEvent, TextStyle, Tx};
use rune_kit::Kit;

use crate::window::{NoticeKind, WindowState};

/// The settings modal's root.
pub const SETTINGS_KEY: &str = "settings";
/// The settings panel (`role=dialog`).
pub const SETTINGS_PANEL: &str = "settings-panel";
/// The scrim behind the panel; pressing it closes the dialog.
pub const SETTINGS_SCRIM: &str = "settings-scrim";
/// The dialog's title text.
pub const SETTINGS_TITLE: &str = "settings-title";
/// The scrolling body.
pub const SETTINGS_BODY: &str = "settings-body";
/// The header's close button.
pub const SETTINGS_CLOSE: &str = "settings-close";
/// The footer's close button.
pub const SETTINGS_CLOSE_FOOTER: &str = "settings-close-footer";
/// Reset to defaults.
pub const SETTINGS_RESET: &str = "settings-reset";
/// The reset confirmation's own Reset button.
pub const SETTINGS_RESET_CONFIRM: &str = "settings-reset-confirm";
/// The reset confirmation's Cancel button.
pub const SETTINGS_RESET_CANCEL: &str = "settings-reset-cancel";
/// The toolbar's gear.
pub const SETTINGS_TOOLBAR: &str = "settings-toolbar";

/// The Appearance section.
pub const SETTINGS_APPEARANCE: &str = "settings-appearance";
/// The Editor section.
pub const SETTINGS_EDITOR: &str = "settings-editor";
/// The General section.
pub const SETTINGS_GENERAL: &str = "settings-general";

/// The theme choice's key prefix.
pub const SETTINGS_THEME: &str = "settings-theme";
/// The reader-size stepper.
pub const SETTINGS_SIZE: &str = "settings-size";
/// The font choice's key prefix.
pub const SETTINGS_FONT: &str = "settings-font";
/// The line-height choice's key prefix.
pub const SETTINGS_SPACING: &str = "settings-spacing";
/// The column-width choice's key prefix.
pub const SETTINGS_WIDTH: &str = "settings-width";
/// The side-panel size choice's key prefix.
pub const SETTINGS_PANELS: &str = "settings-panels";
/// The large-file mode choice's key prefix.
pub const SETTINGS_LARGE_FILE: &str = "settings-large-file";
/// The format bar switch.
pub const SETTINGS_FORMAT_BAR: &str = "settings-format-bar";
/// The spellcheck switch (disabled in this build).
pub const SETTINGS_SPELLCHECK: &str = "settings-spellcheck";
/// The language select's trigger.
pub const SETTINGS_LANGUAGE: &str = "settings-language";
/// The language select's trigger key, as the kit derives it from `-trigger`.
pub const SETTINGS_LANGUAGE_TRIGGER: &str = "settings-language-trigger";
/// The session-restore switch.
pub const SETTINGS_RESTORE: &str = "settings-restore";
/// The print/PDF H1 page-break switch.
pub const SETTINGS_BREAK_H1: &str = "settings-break-h1";
/// The “default for Markdown files” row (Windows and Linux).
pub const SETTINGS_DEFAULT_MD_ROW: &str = "settings-row-default-md";
/// Its button.
pub const SETTINGS_DEFAULT_MD: &str = "settings-default-md";
/// How long the report stays up; on Windows the user is in Settings when it
/// appears, so it outlives an ordinary confirmation.
const DEFAULT_MD_NOTICE_SECONDS: f64 = 8.0;

/// One choice in the theme control, in the original app's order.
pub const THEMES: [ThemePref; 5] =
    [ThemePref::System, ThemePref::Light, ThemePref::Sepia, ThemePref::Dark, ThemePref::Black];
/// The font choices.
pub const READER_FONTS: [ReaderFont; 2] = [ReaderFont::Sans, ReaderFont::Serif];
/// The line-spacing choices.
pub const LINE_HEIGHTS: [LineHeight; 3] = [LineHeight::Compact, LineHeight::Normal, LineHeight::Relaxed];
/// The column-width choices.
pub const COLUMN_WIDTHS: [ColumnWidth; 4] =
    [ColumnWidth::Narrow, ColumnWidth::Normal, ColumnWidth::Wide, ColumnWidth::Full];
/// The side-panel text-size choices.
pub const SIDE_SIZES: [SideSize; 3] = [SideSize::Small, SideSize::Normal, SideSize::Large];
/// The large-file mode choices.
pub const LARGE_FILE_MODES: [LargeFileMode; 2] = [LargeFileMode::Auto, LargeFileMode::Block];

/// The theme's label in the UI language.
pub fn theme_label(theme: ThemePref) -> &'static str {
    match theme {
        ThemePref::System => lmdv_i18n::t!("rs.auto"),
        ThemePref::Light => lmdv_i18n::t!("rs.light"),
        ThemePref::Sepia => lmdv_i18n::t!("rs.sepia"),
        ThemePref::Dark => lmdv_i18n::t!("rs.dark"),
        ThemePref::Black => lmdv_i18n::t!("rs.black"),
    }
}

/// The font's label.
pub fn font_label(font: ReaderFont) -> &'static str {
    match font {
        ReaderFont::Sans => lmdv_i18n::t!("rs.sans"),
        ReaderFont::Serif => lmdv_i18n::t!("rs.serif"),
    }
}

/// The line spacing's label.
pub fn line_height_label(line: LineHeight) -> &'static str {
    match line {
        LineHeight::Compact => lmdv_i18n::t!("rs.compact"),
        LineHeight::Normal => lmdv_i18n::t!("rs.normal"),
        LineHeight::Relaxed => lmdv_i18n::t!("rs.relaxed"),
    }
}

/// The column width's label.
pub fn column_width_label(width: ColumnWidth) -> &'static str {
    match width {
        ColumnWidth::Narrow => lmdv_i18n::t!("rs.narrow"),
        ColumnWidth::Normal => lmdv_i18n::t!("rs.normal"),
        ColumnWidth::Wide => lmdv_i18n::t!("rs.wide"),
        ColumnWidth::Full => lmdv_i18n::t!("rs.full"),
    }
}

/// The side-panel size's label.
pub fn side_size_label(size: SideSize) -> &'static str {
    match size {
        SideSize::Small => lmdv_i18n::t!("rs.small"),
        SideSize::Normal => lmdv_i18n::t!("rs.normal"),
        SideSize::Large => lmdv_i18n::t!("rs.large"),
    }
}

/// The large-file mode's label.
pub fn large_file_label(mode: LargeFileMode) -> &'static str {
    match mode {
        LargeFileMode::Auto => lmdv_i18n::t!("rs.auto"),
        LargeFileMode::Block => lmdv_i18n::t!("set.largeFileBlock"),
    }
}

/// A theme's index in [`THEMES`].
pub fn theme_index(theme: ThemePref) -> usize {
    THEMES.iter().position(|candidate| *candidate == theme).unwrap_or(0)
}

/// A model for the theme's anchored focus key.
pub fn theme_key(index: usize) -> String {
    format!("{SETTINGS_THEME}-{index}")
}

/// The reading column's maximum width in points, or `None` for the full pane
/// (P-01's narrow 640, normal 760, wide 920, full none).
pub fn column_max_width(width: ColumnWidth) -> Option<f32> {
    match width {
        ColumnWidth::Narrow => Some(640.0),
        ColumnWidth::Normal => Some(760.0),
        ColumnWidth::Wide => Some(920.0),
        ColumnWidth::Full => None,
    }
}

/// The pane text-size multiplier: the original's 12/13/14.5 px over 13.
pub fn pane_scale(size: SideSize) -> f32 {
    match size {
        SideSize::Small => 12.0 / 13.0,
        SideSize::Normal => 1.0,
        SideSize::Large => 14.5 / 13.0,
    }
}

/// The kit a side pane draws with: the window's colours at the side-panel size.
///
/// The browser and outline build exclusively from `kit.ty`, so scaling the
/// type tokens scales every label, row and hint in them without touching the
/// palette, radii or motion.
pub fn pane_kit(kit: &Kit, size: SideSize) -> Kit {
    let scale = pane_scale(size);
    if (scale - 1.0).abs() <= f32::EPSILON {
        return kit.clone();
    }
    let mut pane = kit.clone();
    let type_scale = |style: &mut TextStyle| {
        style.size *= scale;
        style.line_height *= scale;
    };
    type_scale(&mut pane.ty.ui);
    type_scale(&mut pane.ty.ui_medium);
    type_scale(&mut pane.ty.ui_compact);
    type_scale(&mut pane.ty.small);
    type_scale(&mut pane.ty.small_medium);
    type_scale(&mut pane.ty.body);
    type_scale(&mut pane.ty.body_medium);
    type_scale(&mut pane.ty.text_small);
    type_scale(&mut pane.ty.text_small_medium);
    type_scale(&mut pane.ty.text_xsmall);
    type_scale(&mut pane.ty.text_xsmall_medium);
    pane
}

/// Whether this build offers the “default for Markdown files” action.
///
/// Windows registers lmdv and hands the confirmation to Default apps; Linux
/// writes the XDG entry and the user's `mimeapps.list` and applies it at once.
/// The row is not drawn anywhere else, although the action still answers with
/// the unsupported notice if reached.
pub const fn default_markdown_supported() -> bool {
    lmdv_io::assoc::default_supported()
}

/// The row's explanation: what the press actually does on this platform.
///
/// Windows needs the user's confirmation in Default apps, so the detail says so;
/// Linux applies the change here and now.
pub fn default_markdown_detail() -> &'static str {
    if cfg!(windows) { lmdv_i18n::t!("set.defaultMdDetail") } else { lmdv_i18n::t!("set.defaultMdDetailLinux") }
}

/// The notice a “make default” result shows, and how it reads.
///
/// Pure, so the mapping is tested without writing a registry or a desktop entry:
/// Linux's [`Confirmation::Applied`] is a completed action, the Windows pages
/// are informational hand-offs, and a refusal that still registered lmdv is a
/// warning.
pub fn default_markdown_notice(result: Result<Confirmation, AssociationError>) -> (String, NoticeKind) {
    match result {
        Ok(Confirmation::Applied) => (lmdv_i18n::t!("msg.defaultMdApplied").to_owned(), NoticeKind::Success),
        Ok(Confirmation::FilteredSettingsPage | Confirmation::SettingsPage) => {
            (lmdv_i18n::t!("msg.defaultMdRegistered").to_owned(), NoticeKind::Info)
        }
        Ok(Confirmation::OpenWithDialog) => (lmdv_i18n::t!("msg.defaultMdOpenWith").to_owned(), NoticeKind::Info),
        Ok(Confirmation::Unavailable) => (lmdv_i18n::t!("msg.defaultMdManual").to_owned(), NoticeKind::Warning),
        Err(AssociationError::Unsupported) => (lmdv_i18n::t!("msg.defaultMdUnsupported").to_owned(), NoticeKind::Info),
        Err(error) => (lmdv_i18n::t!("msg.defaultMdFailed", "err" => error.to_string()), NoticeKind::Warning),
    }
}

/// The locale index a preference set resolves to.
///
/// `None` is Auto: the OS's preferred languages. An unknown stored id falls
/// back to Auto rather than leaving the previous language in place.
pub fn locale_index(prefs: &Prefs) -> usize {
    match prefs.locale.as_deref() {
        Some(id) => lmdv_i18n::find_locale(id).unwrap_or_else(lmdv_i18n::negotiate_system),
        None => lmdv_i18n::negotiate_system(),
    }
}

/// Makes `prefs`' language the process-wide UI language.
///
/// The translator is an atomic shared by every window, so the next frame of
/// every open window draws in the new language. There is no reload: the
/// original needed one only because its strings lived in the DOM.
pub fn apply_locale(prefs: &Prefs) {
    let index = locale_index(prefs);
    if let Some(info) = lmdv_i18n::locales().get(index) {
        lmdv_i18n::set_locale(info.id());
    }
}

/// A language row: what the picker shows and the stored id (`None` = Auto).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageOption {
    pub label: String,
    pub id: Option<String>,
}

/// Auto followed by the six catalogs, in the original's order (English first,
/// then the endonyms the catalog ships).
pub fn language_options() -> Vec<LanguageOption> {
    let mut options = vec![LanguageOption { label: lmdv_i18n::t!("rs.langAuto").to_owned(), id: None }];
    for info in lmdv_i18n::locales() {
        options.push(LanguageOption { label: info.label().to_owned(), id: Some(info.id().to_owned()) });
    }
    options
}

/// The label the language picker shows for `prefs`.
pub fn language_current(prefs: &Prefs) -> String {
    let Some(id) = prefs.locale.as_deref() else { return lmdv_i18n::t!("rs.langAuto").to_owned() };
    lmdv_i18n::locales()
        .iter()
        .find(|info| info.id() == id)
        .map_or_else(|| id.to_owned(), |info| info.label().to_owned())
}

/// The transient state of the settings modal. Not persisted.
#[derive(Debug, Default)]
pub struct SettingsState {
    /// The dialog is mounted.
    pub open: bool,
    /// The footer asks “Reset every setting?” instead of offering Close alone.
    pub confirm_reset: bool,
    /// The language listbox is open.
    pub language_open: bool,
}

impl WindowState {
    /// Whether the settings dialog is open.
    pub fn settings_is_open(&self) -> bool {
        self.settings.open
    }

    /// Opens the settings dialog, closing any other transient menu first.
    ///
    /// Focus moves to the current theme segment; Rune's modal stack traps it in
    /// the panel and returns it to whatever had it before on close.
    pub fn open_settings(&mut self, tx: &mut Tx) {
        self.open_settings_focused(tx, None);
    }

    /// `app.reading-settings`: the same dialog, with focus on the reader
    /// section's size stepper (the reading controls start there).
    pub fn open_reading_settings(&mut self, tx: &mut Tx) {
        self.open_settings_focused(tx, Some(format!("{SETTINGS_SIZE}-value")));
    }

    /// Opens the settings dialog, closing any other transient menu first.
    ///
    /// Focus moves to the requested control, or to the current theme segment;
    /// Rune's modal stack traps it in the panel and returns it to whatever had
    /// it before on close.
    fn open_settings_focused(&mut self, tx: &mut Tx, focus: Option<String>) {
        if self.palette_is_open() {
            self.close_palette();
        }
        if self.share.menu_open {
            self.close_share_menu();
        }
        self.close_help();
        self.close_block_menu();
        if self.browser_menu().is_some() {
            self.close_browser_menu();
        }
        self.settings.open = true;
        self.settings.confirm_reset = false;
        self.settings.language_open = false;
        let key = focus.unwrap_or_else(|| {
            let index = theme_index(self.shared().prefs().theme);
            theme_key(index)
        });
        tx.focus(key, false);
    }

    /// Closes the settings dialog and forgets its transient state.
    pub fn close_settings(&mut self) {
        if !self.settings.open {
            return;
        }
        self.settings.open = false;
        self.settings.confirm_reset = false;
        self.settings.language_open = false;
    }

    /// The dialog's own keys: Escape backs out of the language list, then the
    /// reset confirmation, then the dialog. Return activates the focused
    /// control (Enter is not a default button here, so the dialog gives it the
    /// same meaning Space has); other keys stay with the controls.
    pub fn settings_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.settings.open {
            return false;
        }
        if event.key == Key::Escape {
            if self.settings.language_open {
                self.settings.language_open = false;
                return true;
            }
            if self.settings.confirm_reset {
                self.settings.confirm_reset = false;
                return true;
            }
            self.close_settings();
            return true;
        }
        if event.key == Key::Enter && event.mods == rune::Mods::default() {
            let focused = tx.focused().map(str::to_owned);
            if let Some(key) = focused {
                return self.settings_activate(&key);
            }
        }
        false
    }

    /// Runs the action of the control keyed `key`, for Return on a focused
    /// control. Unknown keys do nothing.
    pub fn settings_activate(&mut self, key: &str) -> bool {
        if let Some(rest) = key.strip_prefix("settings-theme-") {
            let Some(theme) = rest.parse::<usize>().ok().and_then(|index| THEMES.get(index)).copied() else {
                return false;
            };
            self.set_theme(theme);
            return true;
        }
        if let Some(rest) = key.strip_prefix("settings-font-") {
            let Some(font) = rest.parse::<usize>().ok().and_then(|index| READER_FONTS.get(index)).copied() else {
                return false;
            };
            self.set_reader_font(font);
            return true;
        }
        if let Some(rest) = key.strip_prefix("settings-spacing-") {
            let Some(line) = rest.parse::<usize>().ok().and_then(|index| LINE_HEIGHTS.get(index)).copied() else {
                return false;
            };
            self.set_line_height(line);
            return true;
        }
        if let Some(rest) = key.strip_prefix("settings-width-") {
            let Some(width) = rest.parse::<usize>().ok().and_then(|index| COLUMN_WIDTHS.get(index)).copied() else {
                return false;
            };
            self.set_column_width(width);
            return true;
        }
        if let Some(rest) = key.strip_prefix("settings-panels-") {
            let Some(size) = rest.parse::<usize>().ok().and_then(|index| SIDE_SIZES.get(index)).copied() else {
                return false;
            };
            self.set_side_size(size);
            return true;
        }
        if let Some(rest) = key.strip_prefix("settings-large-file-") {
            let Some(mode) = rest.parse::<usize>().ok().and_then(|index| LARGE_FILE_MODES.get(index)).copied() else {
                return false;
            };
            self.set_large_file_mode(mode);
            return true;
        }
        match key {
            "settings-size-down" => {
                self.bump_reader_size(-1);
                true
            }
            "settings-size-up" => {
                self.bump_reader_size(1);
                true
            }
            "settings-format-bar" => {
                let shown = self.shared().prefs().editor.format_bar;
                self.set_format_bar(!shown);
                true
            }
            "settings-spellcheck" => {
                let on = self.shared().prefs().editor.spellcheck;
                self.set_spellcheck(!on);
                true
            }
            "settings-language-trigger" => {
                self.settings.language_open = !self.settings.language_open;
                true
            }
            "settings-restore" => {
                let on = self.shared().prefs().restore_session;
                self.set_restore_session(!on);
                true
            }
            "settings-break-h1" => {
                let on = self.shared().prefs().print_break_h1;
                self.set_print_break_h1(!on);
                true
            }
            SETTINGS_DEFAULT_MD => {
                self.make_default_markdown();
                true
            }
            "settings-close" | "settings-close-footer" => {
                self.close_settings();
                true
            }
            "settings-reset" => {
                self.request_reset_settings();
                true
            }
            "settings-reset-confirm" => {
                if self.settings.confirm_reset {
                    self.reset_settings();
                }
                true
            }
            "settings-reset-cancel" => {
                self.cancel_reset_settings();
                true
            }
            _ => false,
        }
    }

    /// Applies a preference patch to the shared, stored set and redraws every
    /// window.
    ///
    /// The patch runs on a clone, is sanitized (size steps, panel widths,
    /// locale), and goes through [`AppShared::set_prefs`] — the only writer.
    /// A patch that changes nothing does not touch the store or broadcast.
    pub fn patch_prefs(&mut self, patch: impl FnOnce(&mut Prefs)) {
        let mut prefs = self.shared().prefs().clone();
        patch(&mut prefs);
        prefs.sanitize();
        if self.shared().set_prefs(prefs) {
            self.invalidate_appearance();
        }
    }

    /// Applies a preference patch for a headless scene without writing
    /// `prefs.json`.
    ///
    /// A snapshot of `reader-sepia`, `reader-black` or `editor-size` must not
    /// change the user's saved settings; the scene still redraws through the
    /// same cache invalidation a real change uses.
    pub(crate) fn scene_prefs(&mut self, patch: impl FnOnce(&mut Prefs)) {
        let mut prefs = self.shared().prefs().clone();
        patch(&mut prefs);
        prefs.sanitize();
        if self.shared().preview_prefs(prefs) {
            self.invalidate_appearance();
        }
    }

    /// The current theme.
    pub fn settings_theme(&self) -> ThemePref {
        self.shared().prefs().theme
    }

    /// Chooses a theme. Sepia and black carry their own full palettes (J-01);
    /// the preference reaches the window chrome, the article and every overlay
    /// on the next frame.
    pub fn set_theme(&mut self, theme: ThemePref) {
        self.patch_prefs(|prefs| prefs.theme = theme);
    }

    /// Steps the theme through System → Light → Sepia → Dark → Black → System.
    pub fn cycle_theme(&mut self) {
        let current = self.settings_theme();
        let next = match current {
            ThemePref::System => ThemePref::Light,
            ThemePref::Light => ThemePref::Sepia,
            ThemePref::Sepia => ThemePref::Dark,
            ThemePref::Dark => ThemePref::Black,
            ThemePref::Black => ThemePref::System,
        };
        self.set_theme(next);
    }

    /// The index of the current reader size in [`SIZE_STEPS`].
    pub fn reader_size_index(&self) -> usize {
        let size = self.shared().prefs().reader.size;
        SIZE_STEPS.iter().position(|step| *step == size).unwrap_or(3)
    }

    /// Whether the size stepper can go down (it cannot at 13).
    pub fn can_shrink_reader(&self) -> bool {
        self.reader_size_index() > 0
    }

    /// Whether the size stepper can go up (it cannot at 22).
    pub fn can_grow_reader(&self) -> bool {
        self.reader_size_index() + 1 < SIZE_STEPS.len()
    }

    /// Sets a reader size, snapped to the allowed steps.
    pub fn set_reader_size(&mut self, size: u16) {
        let size = lmdv_core::snap_size(size);
        self.patch_prefs(|prefs| prefs.reader.size = size);
    }

    /// Steps the reader size one allowed step up or down.
    pub fn bump_reader_size(&mut self, direction: i32) {
        let index = self.reader_size_index() as i32 + direction;
        if let Some(size) = usize::try_from(index).ok().and_then(|index| SIZE_STEPS.get(index)).copied() {
            self.set_reader_size(size);
        }
    }

    /// `Ctrl`+wheel or a pinch over the article changes the reader size (F-21).
    ///
    /// One step per gesture notch: trackpads emit many small deltas, so the
    /// steps accumulate until they pass the same threshold the panes use.
    pub fn reader_magnify(&mut self, delta: f32) -> bool {
        if !delta.is_finite() || delta == 0.0 {
            return true;
        }
        self.reader_magnify += delta;
        if self.reader_magnify.abs() < 0.35 {
            return true;
        }
        let step = if self.reader_magnify > 0.0 { 1 } else { -1 };
        self.reader_magnify = 0.0;
        self.bump_reader_size(step);
        true
    }

    /// Restores the default reader size (16).
    pub fn reset_reader_size(&mut self) {
        self.set_reader_size(DEFAULT_READER_SIZE);
    }

    /// Sets the reading font.
    pub fn set_reader_font(&mut self, font: ReaderFont) {
        self.patch_prefs(|prefs| prefs.reader.font = font);
    }

    /// Sets the reading line spacing.
    pub fn set_line_height(&mut self, line: LineHeight) {
        self.patch_prefs(|prefs| prefs.reader.line_height = line);
    }

    /// Sets the reading column width.
    pub fn set_column_width(&mut self, width: ColumnWidth) {
        self.patch_prefs(|prefs| prefs.reader.width = width);
    }

    /// Sets the side panes' text size.
    pub fn set_side_size(&mut self, size: SideSize) {
        self.patch_prefs(|prefs| prefs.reader.side_size = size);
    }

    /// Sets the large-file editor mode (chosen when an edit session starts).
    pub fn set_large_file_mode(&mut self, mode: LargeFileMode) {
        self.patch_prefs(|prefs| prefs.editor.large_file_mode = mode);
    }

    /// Shows or hides the editor's format bar.
    pub fn set_format_bar(&mut self, shown: bool) {
        self.patch_prefs(|prefs| prefs.editor.format_bar = shown);
    }

    /// Turns spellcheck on or off (E-32).
    ///
    /// The switch is shown only while a dictionary exists for the launch's
    /// locale (E-SPELL); the preference is stored as `editor.spellcheck`
    /// (P-11) and survives the next start. Disabling it drops the current
    /// underlines at once.
    pub fn set_spellcheck(&mut self, on: bool) {
        self.patch_prefs(|prefs| prefs.editor.spellcheck = on);
        if !on {
            self.editor.spell.reset();
        }
    }

    /// The language label the picker shows.
    pub fn language_current(&self) -> String {
        language_current(&self.shared().prefs())
    }

    /// Stores a language (`None` = Auto) and applies it live.
    pub fn set_language(&mut self, id: Option<String>) {
        self.patch_prefs(|prefs| prefs.locale = id);
        self.settings.language_open = false;
    }

    /// Stores the language whose endonym is `label` (the picker's option
    /// string), or Auto when it names none.
    pub fn set_language_label(&mut self, label: &str) {
        let id = language_options().into_iter().find(|option| option.label == label).and_then(|option| option.id);
        self.set_language(id);
    }

    /// Turns session restore on or off (read at the next start).
    pub fn set_restore_session(&mut self, restore: bool) {
        self.patch_prefs(|prefs| prefs.restore_session = restore);
    }

    /// Turns the print/PDF H1 page break on or off; the Share menu reads the
    /// same preference.
    pub fn set_print_break_h1(&mut self, on: bool) {
        self.patch_prefs(|prefs| prefs.print_break_h1 = on);
    }

    /// `settings-default-md`: makes lmdv the Markdown handler.
    ///
    /// On Linux the XDG entry and the user's `mimeapps.list` are written here and
    /// the action is complete. Windows 10/11 protect `UserChoice`, so there the
    /// registry work lives in [`lmdv_io::assoc`] and the dialog closes (the
    /// Default apps page covers it) while lmdv reports which surface opened. A
    /// platform with neither reaches the same path and says the action is
    /// unavailable.
    pub fn make_default_markdown(&mut self) {
        self.close_settings();
        let active = self.active_path();
        let (text, kind) = default_markdown_notice(lmdv_io::assoc::make_default_for_markdown(active.as_deref()));
        // On Windows the Default apps page takes the user away from lmdv; let the
        // report outlive the usual copy confirmation (H-06's strip is the one
        // place it is announced).
        self.show_notice(text, kind, None, DEFAULT_MD_NOTICE_SECONDS);
    }

    /// Whether every preference this dialog resets still equals its default, so
    /// “Reset to defaults” has nothing to do.
    ///
    /// The print/PDF H1 page break is not part of the reset (it survives), so
    /// it does not decide whether the button is enabled.
    pub fn prefs_are_default(&self) -> bool {
        let prefs = self.shared().prefs();
        *prefs == Prefs { print_break_h1: prefs.print_break_h1, ..Prefs::default() }
    }

    /// Asks the footer to confirm a full reset.
    pub fn request_reset_settings(&mut self) {
        if self.prefs_are_default() {
            return;
        }
        self.settings.confirm_reset = true;
    }

    /// Cancels the reset confirmation.
    pub fn cancel_reset_settings(&mut self) {
        self.settings.confirm_reset = false;
    }

    /// Resets every preference, applies the defaults and closes the
    /// confirmation.
    ///
    /// As in the original's text reset, the print/PDF H1 page break is kept:
    /// it belongs to the Share menu's print options, not to this dialog's
    /// reading settings. Everything the dialog itself edits goes back to its
    /// default: theme, reader typography, editor, language and session restore.
    pub fn reset_settings(&mut self) {
        self.settings.confirm_reset = false;
        let keep_break_h1 = self.shared().prefs().print_break_h1;
        let prefs = Prefs { print_break_h1: keep_break_h1, ..Prefs::default() };
        self.shared().set_prefs(prefs);
        self.invalidate_appearance();
    }
}
