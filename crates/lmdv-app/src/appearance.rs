//! The reading appearance: one `Kit`, `DocTheme` and `MarkdownStyle` per
//! scheme and reader preference set.
//!
//! The kit is the window's semantic palette: chrome, panes, controls, menus and
//! dialogs all draw from it. The reader's [`DocTheme`] and the Mermaid
//! [`MarkdownStyle`] are derived from the same kit and then adjusted by the
//! reader preferences, so a frame reuses them instead of rebuilding syntax
//! colours and text styles (R-02, J-01, OI-019).
//!
//! Light, system and dark use `rune-kit`'s neutral palettes. Sepia and black
//! have palettes of their own, built from the original app's tokens
//! (`src/styles/base.css:60-140`) and mapped onto the kit's
//! semantic roles in [`themed`].

use std::rc::Rc;

use lmdv_core::{LineHeight, Prefs, ReaderFont, ThemePref};
use lmdv_render::{CalloutPaint, DocLabels, DocTheme};
use rune::{Color, Family, oklch};
use rune_kit::Kit;
use rune_kit::kit::StateColors;
use rune_kit::markdown::MarkdownStyle;

/// The serif stack the reading font preference asks for.
///
/// The generic `serif` family is not a concrete face, so the stack names the
/// usual Linux serifs first; a machine without them falls back to the sans slot
/// through `DocTheme::from_kit`.
pub const SERIF_STACK: &str = "Liberation Serif, DejaVu Serif, Noto Serif";

/// Everything one appearance costs to derive.
#[derive(Clone, Debug)]
pub struct Appearance {
    /// Whether this is the dark scheme (chooses theme images, M-29).
    pub dark: bool,
    /// The Markdown theme.
    pub theme: DocTheme,
    /// The kit style Mermaid diagrams draw with.
    pub style: MarkdownStyle,
    /// The reading size in points after the reader preference.
    pub size: f32,
}

/// The inputs that change the derived appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppearanceKey {
    /// The stored theme preference.
    pub theme: ThemePref,
    /// The OS scheme, which only `System` reads.
    pub system_dark: bool,
    /// The resolved scheme.
    pub dark: bool,
    /// The reader size in points.
    pub size: u16,
    /// The reader line spacing.
    pub line_height: LineHeight,
    /// The reading font family.
    pub font: ReaderFont,
}

impl AppearanceKey {
    /// The key a preference set resolves to against the system scheme.
    pub fn for_prefs(prefs: &Prefs, system_dark: bool) -> Self {
        Self {
            theme: prefs.theme,
            system_dark,
            dark: is_dark(prefs.theme, system_dark),
            size: prefs.reader.size,
            line_height: prefs.reader.line_height,
            font: prefs.reader.font,
        }
    }
}

/// The scheme a theme preference asks for.
///
/// Sepia is always the light scheme and black always the dark one; `System`
/// follows the OS. This also chooses the image scheme a document draws (M-29).
pub fn is_dark(theme: ThemePref, system_dark: bool) -> bool {
    match theme {
        ThemePref::Light | ThemePref::Sepia => false,
        ThemePref::Dark | ThemePref::Black => true,
        ThemePref::System => system_dark,
    }
}

/// The window kit for `theme`, resolved against the system scheme.
///
/// Light, system and dark use `rune-kit`'s neutral light/dark palettes; sepia
/// and black override the semantic roles from the original app's tokens. The
/// neutral light accent is darkened to clear AA for link ink and primary
/// buttons (A-01).
pub(crate) fn palette(theme: ThemePref, system_dark: bool) -> Kit {
    match theme {
        ThemePref::Sepia => themed(false, &sepia()),
        ThemePref::Black => themed(true, &black()),
        ThemePref::Light | ThemePref::System | ThemePref::Dark => {
            let mut kit = Kit::neutral(is_dark(theme, system_dark));
            if !kit.dark {
                // A-01: the neutral light accent (L 0.60, #2f7fe3) is
                // 3.99:1 on the white card and 3.54:1 on the page — below AA
                // for links, accent icons and primary-button ink. The same
                // hue at L 0.52 is 5.60:1 on white and 4.96:1 on the page.
                let accent = oklch(0.52, 0.17, 256.0);
                let c = &mut kit.c;
                c.bg_accent = accent;
                c.text_accent = accent;
                c.icon_accent = accent;
                c.border_focus = accent;
                c.border_active = accent;
            }
            kit
        }
    }
}

/// One scheme's tokens, as the original CSS custom properties name them.
///
/// Only the tokens the original app defines are here: `warning` and `info`
/// keep the kit's neutral roles (the original has no palette variables for
/// them), and the callout inks are the per-scheme GitHub alert colours from
/// `markdown.css`, applied by [`apply_scheme`].
struct Scheme {
    /// `--bg`: the window behind the reading card.
    page: Color,
    /// `--surface`: the reading card and toolbar.
    surface: Color,
    /// `--surface-2`: the table header, control tracks and chips.
    surface_2: Color,
    /// `--text`.
    text: Color,
    /// `--text-muted`.
    muted: Color,
    /// `--border`: rules, table and code frames.
    border: Color,
    /// `--accent`: fills, the progress hairline and quote rules.
    accent: Color,
    /// `--link`: link ink and accent text.
    link: Color,
    /// `--code-bg`: fenced code fill.
    code_bg: Color,
    /// `--code-chip-bg`: inline code fill.
    chip_bg: Color,
    /// `--code-chip-border`.
    chip_border: Color,
    /// `--danger`.
    danger: Color,
    /// `--success`.
    success: Color,
    /// The ink that clears AA on `accent` (`--on-accent`).
    on_accent: Color,
    /// The accent tint's alpha (`--accent-soft`).
    accent_soft: f32,
    /// The five GitHub alert inks, in note/tip/important/warning/caution order.
    alerts: [Color; 5],
}

/// The original sepia palette (`:root[data-theme="sepia"]`).
fn sepia() -> Scheme {
    Scheme {
        page: token("#f3e9d6"),
        surface: token("#faf3e4"),
        surface_2: token("#ebdfc8"),
        text: token("#3b2f22"),
        muted: token("#6f5d49"),
        border: token("#dccdb2"),
        accent: token("#b2552f"),
        link: token("#9a4322"),
        code_bg: token("#eee3cd"),
        chip_bg: token("#e6d9bf"),
        chip_border: token("#cfbf9f"),
        danger: token("#a2271c"),
        success: token("#2a6e40"),
        on_accent: token("#ffffff"),
        accent_soft: 0.12,
        alerts: [token("#085fc5"), token("#177131"), token("#7447c8"), token("#865a00"), token("#bf1f2a")],
    }
}

/// The original black palette (`:root[data-theme="black"]`).
fn black() -> Scheme {
    Scheme {
        page: token("#000000"),
        surface: token("#121212"),
        surface_2: token("#1a1a1a"),
        text: token("#e6e6e3"),
        muted: token("#9c9a93"),
        border: token("#262626"),
        accent: token("#d97757"),
        link: token("#df8768"),
        code_bg: token("#0c0c0c"),
        chip_bg: token("#1f1f1f"),
        chip_border: token("#333333"),
        danger: token("#ff8f80"),
        success: token("#7ccf98"),
        on_accent: token("#1a1915"),
        accent_soft: 0.16,
        alerts: [token("#4493f8"), token("#3fb950"), token("#ab7df8"), token("#d29922"), token("#f9645d")],
    }
}

/// A palette token as the original CSS writes it.
fn token(hex: &str) -> Color {
    rune::color::hex(hex).expect("a palette token is a valid #rrggbb colour")
}

/// The neutral kit with every semantic role filled from `scheme`.
///
/// `page`, `surface` and `surface-2` keep the original's elevation order:
/// `layer_01` is the page (cards, menus and diagram nodes one step below the
/// reading card), `layer_02` the second surface, and `layer_03` the accent tint
/// selected rows paint. Translucent roles are CSS-like alphas, solved over the
/// page for a window that blends in linear light (`blend_over`).
///
/// The sheen constants (`white`, `black`, `depth_top`, `depth_bottom`) and the
/// elevation shadows keep the kit's neutral values; the original's `--shadow`
/// has no per-scheme kit role. `warning` and `info` keep its status roles (see
/// [`Scheme`]); only `success` and `danger` have original palette variables.
fn themed(dark: bool, scheme: &Scheme) -> Kit {
    let mut kit = Kit::neutral(dark);
    kit.blend_over = Some(scheme.page);
    // Elevation rings (zero-blur hairlines on buttons and dialogs) follow the
    // scheme's border once the palette is overridden.
    kit.ring = Some(|c| c.border_base);
    let strong = scheme.border.mix(scheme.text, 0.25);
    let hover = scheme.text.alpha(0.05);
    let pressed = scheme.text.alpha(0.09);
    let c = &mut kit.c;
    c.page = scheme.page;
    c.bg_base = scheme.surface;
    c.bg_deep = scheme.page;
    c.layer_01 = scheme.page;
    c.layer_02 = scheme.surface_2;
    c.layer_03 = scheme.accent.alpha(scheme.accent_soft);
    // The kit's fourth depth is the switch's off track; the original's
    // strongest chip edge is the closest token (`--code-chip-border`).
    c.layer_04 = scheme.chip_border;
    c.bg_inverse = scheme.text;
    c.bg_contrast = scheme.text;
    c.bg_button_neutral = scheme.surface;
    c.bg_icon_button_contrast = scheme.text;
    c.bg_accent = scheme.accent;
    c.submit_highlight = Color::WHITE.alpha(0.2);
    c.submit_hover = Color::WHITE.alpha(0.12);
    c.submit_fg = scheme.on_accent;
    c.submit_icon = scheme.on_accent;
    c.text_base = scheme.text;
    c.text_muted = scheme.muted;
    c.text_faint = scheme.muted.alpha(0.8);
    c.text_inverse = scheme.surface;
    c.text_contrast = scheme.on_accent;
    c.text_accent = scheme.link;
    c.icon_base = scheme.text;
    c.icon_muted = scheme.muted;
    c.icon_faint = scheme.muted.alpha(0.7);
    c.icon_inverse = scheme.surface;
    c.icon_contrast = scheme.on_accent;
    c.icon_accent = scheme.accent;
    c.border_muted = scheme.border;
    c.border_base = scheme.border;
    c.border_strong = strong;
    c.border_focus = scheme.accent;
    c.hover = hover;
    c.pressed = pressed;
    c.contrast_hover = Color::WHITE.alpha(0.1);
    c.contrast_pressed = Color::WHITE.alpha(0.2);
    c.scrim = Color::BLACK.alpha(if dark { 0.5 } else { 0.3 });
    c.knob = scheme.surface;
    c.knob_on = scheme.surface;
    c.success = state(scheme.success, scheme.page);
    c.danger = state(scheme.danger, scheme.page);
    c.surface_base = scheme.surface_2;
    c.surface_weak = scheme.surface_2;
    c.surface_hover = hover;
    c.surface_raised = scheme.surface;
    c.surface_raised_base = scheme.surface;
    c.surface_raised_hover = hover;
    c.surface_float = scheme.text;
    c.surface_disabled = scheme.surface_2;
    c.border_weak = scheme.border;
    c.border_hover = scheme.border.mix(scheme.text, 0.15);
    c.border_active = scheme.accent;
    c.border_disabled = scheme.border;
    c.text_strong = scheme.text;
    c.text_weak = scheme.muted;
    c.text_invert_strong = scheme.surface;
    c.icon_weaker = scheme.muted;
    c.std_border = strong;
    c.std_icon = scheme.muted;
    c.std_icon_invert = scheme.surface;
    c.std_icon_strong = scheme.text;
    c.std_text = scheme.text;
    kit
}

/// One status role over the scheme's page.
fn state(ink: Color, page: Color) -> StateColors {
    StateColors { bg: page.mix(ink, 0.10), fg: ink, border: page.mix(ink, 0.35), icon: ink }
}

/// Derives the appearance for `key`.
pub fn appearance(key: AppearanceKey) -> Rc<Appearance> {
    let kit = palette(key.theme, key.system_dark);
    let mut theme = DocTheme::from_kit(&kit);
    theme.labels = DocLabels::localized();
    // Sepia and black carry the original's exact Markdown tokens: the kit's
    // roles are semantic, while the article needs the original's code, table
    // and alert colours (its CSS names them separately).
    match key.theme {
        ThemePref::Sepia => apply_scheme(&mut theme, &sepia()),
        ThemePref::Black => apply_scheme(&mut theme, &black()),
        ThemePref::Light | ThemePref::System | ThemePref::Dark => {}
    }
    apply_reader(&mut theme, key);
    let style = MarkdownStyle::from_kit(&kit);
    Rc::new(Appearance { dark: key.dark, theme, style, size: f32::from(key.size) })
}

/// Fills the article tokens the kit's semantic roles do not carry 1:1.
///
/// `DocTheme` draws fenced code with `code_fill`, inline code with `code_bg`,
/// the table header with `table_head_fill`, and quote rules with
/// `quote_rule`; the original names those separately (`--code-bg`,
/// `--code-chip-bg`, `--surface-2`, `--accent`). The alert inks are the
/// per-scheme GitHub colours the original overrides in `markdown.css`.
fn apply_scheme(theme: &mut DocTheme, scheme: &Scheme) {
    theme.text = scheme.text;
    theme.muted = scheme.muted;
    theme.faint = scheme.muted.alpha(0.8);
    theme.accent = scheme.link;
    theme.link_underline = scheme.link.alpha(0.4);
    theme.rule = scheme.border;
    theme.quote_rule = scheme.accent;
    theme.code_bg = scheme.chip_bg;
    theme.code_border = scheme.border;
    theme.code_fill = scheme.code_bg;
    theme.code_label = scheme.muted;
    theme.table_border = scheme.border;
    theme.table_head_fill = scheme.surface_2;
    theme.table_head_text = scheme.text;
    theme.callouts = scheme.alerts.map(|accent| CalloutPaint { accent, fill: scheme.surface.mix(accent, 0.06) });
}

/// Applies the reading size, spacing and font family (R-02, OI-019).
fn apply_reader(theme: &mut DocTheme, key: AppearanceKey) {
    let requested = f32::from(key.size).max(8.0);
    let base = theme.body.size.max(1.0);
    let ratio = requested / base;
    if (ratio - 1.0).abs() > f32::EPSILON {
        theme.body.size = requested;
        for heading in &mut theme.headings {
            heading.size *= ratio;
        }
        theme.code.size *= ratio;
        theme.small.size *= ratio;
    }
    let line = match key.line_height {
        LineHeight::Compact => 1.5,
        LineHeight::Normal => 1.7,
        LineHeight::Relaxed => 1.9,
    };
    theme.body.line_height = theme.body.size * line;
    for heading in &mut theme.headings {
        heading.line_height = heading.size * 1.3;
    }
    theme.code.line_height = theme.code.size * 1.55;
    // The prose (body and headings) follows the font preference; inline and
    // fenced code stay monospaced, as in the original.
    if key.font == ReaderFont::Serif {
        let serif = Family::Named(SERIF_STACK);
        theme.body.family = serif;
        for heading in &mut theme.headings {
            heading.family = serif;
        }
    }
    // The theme fingerprint carries every field, so the bump only documents
    // that the app changed tokens outside `DocTheme::from_kit`.
    theme.version = theme.version.wrapping_add(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Color, b: Color) -> f32 {
        let (a, b) = (a.luminance(), b.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn themes_follow_the_preferred_scheme() {
        assert!(!is_dark(ThemePref::Light, true));
        assert!(is_dark(ThemePref::Dark, false));
        assert!(!is_dark(ThemePref::Sepia, true), "sepia is always the light scheme");
        assert!(is_dark(ThemePref::Black, false), "black is always the dark scheme");
        assert!(is_dark(ThemePref::System, true));
        assert!(!is_dark(ThemePref::System, false));
    }

    #[test]
    fn the_sepia_palette_uses_the_original_tokens() {
        let kit = palette(ThemePref::Sepia, true);
        assert!(!kit.dark, "sepia stays light even when the system is dark");
        let c = &kit.c;
        assert_eq!(c.page.hex(), "#f3e9d6ff");
        assert_eq!(c.bg_base.hex(), "#faf3e4ff");
        assert_eq!(c.layer_02.hex(), "#ebdfc8ff");
        assert_eq!(c.text_base.hex(), "#3b2f22ff");
        assert_eq!(c.text_muted.hex(), "#6f5d49ff");
        assert_eq!(c.border_base.hex(), "#dccdb2ff");
        assert_eq!(c.bg_accent.hex(), "#b2552fff");
        assert_eq!(c.text_accent.hex(), "#9a4322ff");
        assert_eq!(c.danger.fg.hex(), "#a2271cff");
        assert_eq!(c.success.fg.hex(), "#2a6e40ff");
        assert_eq!(kit.ty.ui.size, 13.0, "typography is the kit's");
    }

    #[test]
    fn the_black_palette_uses_the_original_tokens() {
        let kit = palette(ThemePref::Black, false);
        assert!(kit.dark, "black stays dark even when the system is light");
        let c = &kit.c;
        assert_eq!(c.page.hex(), "#000000ff");
        assert_eq!(c.bg_base.hex(), "#121212ff");
        assert_eq!(c.layer_02.hex(), "#1a1a1aff");
        assert_eq!(c.text_base.hex(), "#e6e6e3ff");
        assert_eq!(c.text_muted.hex(), "#9c9a93ff");
        assert_eq!(c.border_base.hex(), "#262626ff");
        assert_eq!(c.bg_accent.hex(), "#d97757ff");
        assert_eq!(c.text_accent.hex(), "#df8768ff");
        assert_eq!(c.danger.fg.hex(), "#ff8f80ff");
        assert_eq!(c.success.fg.hex(), "#7ccf98ff");
    }

    #[test]
    fn light_dark_and_system_keep_the_kit_palettes() {
        let light = palette(ThemePref::Light, true);
        let neutral_light = Kit::neutral(false);
        assert_eq!(light.dark, neutral_light.dark);
        assert_eq!(light.c.page.hex(), neutral_light.c.page.hex());
        assert_eq!(light.c.bg_base.hex(), neutral_light.c.bg_base.hex());
        assert_eq!(light.c.text_base.hex(), neutral_light.c.text_base.hex());

        assert!(palette(ThemePref::Dark, false).dark);
        assert!(palette(ThemePref::System, true).dark);
        assert!(!palette(ThemePref::System, false).dark);
    }

    #[test]
    fn sepia_and_black_differ_from_their_neutral_neighbours() {
        let sepia = palette(ThemePref::Sepia, false);
        let light = palette(ThemePref::Light, false);
        assert_ne!(sepia.c.page.hex(), light.c.page.hex());
        assert_ne!(sepia.c.text_accent.hex(), light.c.text_accent.hex());
        assert_ne!(sepia.c.bg_accent.hex(), light.c.bg_accent.hex());

        let black = palette(ThemePref::Black, true);
        let dark = palette(ThemePref::Dark, true);
        assert_ne!(black.c.page.hex(), dark.c.page.hex());
        assert_ne!(black.c.bg_base.hex(), dark.c.bg_base.hex());
        assert_ne!(black.c.layer_02.hex(), dark.c.layer_02.hex());
    }

    #[test]
    fn every_theme_keeps_body_muted_and_link_ink_readable() {
        // A-01: body, muted and link ink at 4.5:1 on both the page and the
        // reading card, in all four themes; primary-button ink too.
        let themes = [
            (ThemePref::Light, palette(ThemePref::Light, false)),
            (ThemePref::Dark, palette(ThemePref::Dark, true)),
            (ThemePref::Sepia, palette(ThemePref::Sepia, false)),
            (ThemePref::Black, palette(ThemePref::Black, true)),
        ];
        for (theme, kit) in themes {
            let c = &kit.c;
            for (surface, backdrop) in [("page", c.page), ("card", c.bg_base)] {
                let body = contrast(c.text_base, backdrop);
                let muted = contrast(c.text_muted, backdrop);
                let link = contrast(c.text_accent, backdrop);
                assert!(body >= 4.5, "{theme:?}: body on {surface} {body:.2}:1");
                assert!(muted >= 4.5, "{theme:?}: muted on {surface} {muted:.2}:1");
                assert!(link >= 4.5, "{theme:?}: link on {surface} {link:.2}:1");
            }
            let button = contrast(c.submit_fg, c.bg_accent);
            assert!(button >= 4.5, "{theme:?}: primary-button ink {button:.2}:1");
            assert!(c.border_base.hex() != c.bg_base.hex(), "{theme:?}: a border is visible");
        }
    }

    #[test]
    fn the_reader_theme_carries_the_scheme_tokens() {
        let prefs = Prefs { theme: ThemePref::Sepia, ..Prefs::default() };
        let look = appearance(AppearanceKey::for_prefs(&prefs, false));
        assert!(!look.dark);
        assert_eq!(look.theme.text.hex(), "#3b2f22ff");
        assert_eq!(look.theme.code_bg.hex(), "#e6d9bfff", "inline code is the chip");
        assert_eq!(look.theme.code_fill.hex(), "#eee3cdff", "fenced code is --code-bg");
        assert_eq!(look.theme.code_border.hex(), "#dccdb2ff");
        assert_eq!(look.theme.table_head_fill.hex(), "#ebdfc8ff");
        assert_eq!(look.theme.rule.hex(), "#dccdb2ff");
        assert_eq!(look.theme.quote_rule.hex(), "#b2552fff", "quotes keep the accent rule");
        assert_eq!(look.theme.callouts[0].accent.hex(), "#085fc5ff", "the sepia alert inks");
        assert_eq!(look.theme.callouts[4].accent.hex(), "#bf1f2aff");

        let prefs = Prefs { theme: ThemePref::Black, ..Prefs::default() };
        let look = appearance(AppearanceKey::for_prefs(&prefs, false));
        assert!(look.dark);
        assert_eq!(look.theme.text.hex(), "#e6e6e3ff");
        assert_eq!(look.theme.code_bg.hex(), "#1f1f1fff");
        assert_eq!(look.theme.code_fill.hex(), "#0c0c0cff");
        assert_eq!(look.theme.table_head_fill.hex(), "#1a1a1aff");
        assert_eq!(look.theme.callouts[0].accent.hex(), "#4493f8ff");
        assert_eq!(look.theme.callouts[4].accent.hex(), "#f9645dff");
    }

    #[test]
    fn mermaid_draws_with_the_themed_kit() {
        // `MarkdownStyle::from_kit` reads the kit's text ink and its first
        // layer for diagram nodes; both come from the scheme.
        let sepia = palette(ThemePref::Sepia, false);
        assert_eq!(sepia.c.text_base.hex(), "#3b2f22ff");
        assert_eq!(sepia.c.layer_01.hex(), "#f3e9d6ff", "nodes sit on the page, not the card");
        assert_eq!(sepia.c.bg_base.hex(), "#faf3e4ff");

        let black = palette(ThemePref::Black, false);
        assert_eq!(black.c.text_base.hex(), "#e6e6e3ff");
        assert_eq!(black.c.layer_01.hex(), "#000000ff");
    }

    #[test]
    fn the_reader_size_and_line_height_reach_the_theme() {
        let mut prefs = Prefs::default();
        prefs.reader.size = 20;
        prefs.reader.line_height = LineHeight::Relaxed;
        let key = AppearanceKey::for_prefs(&prefs, false);
        let look = appearance(key);

        assert_eq!(look.theme.body.size, 20.0);
        assert!((look.theme.body.line_height - 38.0).abs() < 0.01, "1.9 lines at 20 pt");
        assert_eq!(look.size, 20.0);
        assert!(look.theme.heading(1).size > 30.4, "headings scale with the body");
    }

    #[test]
    fn the_default_size_keeps_the_classic_metrics() {
        let prefs = Prefs::default();
        let look = appearance(AppearanceKey::for_prefs(&prefs, false));
        assert_eq!(look.theme.body.size, 16.0);
        assert!((look.theme.body.line_height - 27.2).abs() < 0.01);
        assert!(!look.dark);
        assert_eq!(look.theme.code_bg.hex(), palette(ThemePref::Light, false).c.layer_02.hex());
    }

    #[test]
    fn two_calls_with_the_same_key_derive_equal_looks() {
        let key = AppearanceKey {
            theme: ThemePref::Sepia,
            system_dark: false,
            dark: false,
            size: 16,
            line_height: LineHeight::Normal,
            font: ReaderFont::Sans,
        };
        let first = appearance(key);
        let second = appearance(key);
        assert_eq!(first.theme.fingerprint(), second.theme.fingerprint());
    }

    #[test]
    fn the_serif_font_preference_reaches_the_prose_and_keeps_code_mono() {
        let mut prefs = Prefs::default();
        prefs.reader.font = ReaderFont::Serif;
        let look = appearance(AppearanceKey::for_prefs(&prefs, false));
        assert_eq!(look.theme.body.family, Family::Named(SERIF_STACK));
        for heading in &look.theme.headings {
            assert_eq!(heading.family, Family::Named(SERIF_STACK), "headings follow the reading font");
        }
        assert!(!matches!(look.theme.code.family, Family::Named(_)), "code keeps the kit's mono face");

        prefs.reader.font = ReaderFont::Sans;
        let sans = appearance(AppearanceKey::for_prefs(&prefs, false));
        assert!(!matches!(sans.theme.body.family, Family::Named(_)));
        assert_ne!(sans.theme.fingerprint(), look.theme.fingerprint(), "the font joins the memo version");
    }
}
