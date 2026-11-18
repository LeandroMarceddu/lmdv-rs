//! The reading theme: kit tokens plus the Markdown-specific sizes, colours and
//! labels the renderer draws with.
//!
//! [`DocTheme::from_kit`] derives every colour from the kit's semantic palette,
//! so a new app theme needs no Markdown work; [`DocTheme::neutral`] builds the
//! kit's neutral light or dark theme for tests, exports and other crates.
//!
//! Sizes follow the original page (`src/styles/markdown.css`)
//! at a 16 pt reading size, and R-02's 1.7 line height.

use rune::tok::TextStyle;
use rune::{Color, oklch};
use rune_kit::Kit;
use rune_kit::syntax::SyntaxTheme;

const FRONT_MATTER: lmdv_i18n::Key = lmdv_i18n::key("render.frontMatter");
const TOC: lmdv_i18n::Key = lmdv_i18n::key("render.toc");
const INVALID_MERMAID: lmdv_i18n::Key = lmdv_i18n::key("render.invalidMermaid");
const COPY: lmdv_i18n::Key = lmdv_i18n::key("render.copy");
const CALLOUTS: [lmdv_i18n::Key; 5] = [
    lmdv_i18n::key("render.calloutNote"),
    lmdv_i18n::key("render.calloutTip"),
    lmdv_i18n::key("render.calloutImportant"),
    lmdv_i18n::key("render.calloutWarning"),
    lmdv_i18n::key("render.calloutCaution"),
];
const IMAGE: lmdv_i18n::Key = lmdv_i18n::key("render.image");
const FOOTNOTES: lmdv_i18n::Key = lmdv_i18n::key("render.footnotes");
const FOOTNOTE_N: lmdv_i18n::Key = lmdv_i18n::key("render.footnoteN");
const MATH: lmdv_i18n::Key = lmdv_i18n::key("render.math");
const OPEN_EXTERNALLY: lmdv_i18n::Key = lmdv_i18n::key("render.openExternally");
const MERMAID: lmdv_i18n::Key = lmdv_i18n::key("render.mermaid");
const DETAILS: lmdv_i18n::Key = lmdv_i18n::key("render.details");
const TABLE: lmdv_i18n::Key = lmdv_i18n::key("render.table");

/// The five GitHub alert kinds' paint, in the order of
/// [`lmdv_md::CalloutKind`]: note, tip, important, warning, caution.
#[derive(Clone, Copy, Debug)]
pub struct CalloutPaint {
    /// The title, icon and left rule.
    pub accent: Color,
    /// The card's fill, a 6 % tint of `accent` over the page.
    pub fill: Color,
}

/// Localized strings the renderer draws itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DocLabels {
    /// The front-matter card's title (M-12).
    pub front_matter: String,
    /// A `[TOC]` navigation's accessible name (M-20).
    pub toc: String,
    /// The notice above a Mermaid source that could not be drawn (M-14).
    pub invalid_mermaid: String,
    /// The options menu's “copy” action on a code block (M-13).
    pub copy: String,
    /// The default title of each callout kind (M-19).
    pub callouts: [String; 5],
    /// The backlink on a footnote entry (M-11).
    pub footnote_back: String,
    /// A footnote entry's accessible name; `{n}` is its number (M-11).
    pub footnote: String,
    /// The label of a video/audio placeholder's external-open action.
    pub open_externally: String,
    /// A display formula's accessible name; `{tex}` is its TeX source.
    pub math: String,
    /// The alt text used for an image with no alt of its own.
    pub image: String,
    /// The accessible name of the generated footnotes section.
    pub footnotes: String,
    /// The accessible name of a rendered Mermaid diagram.
    pub mermaid: String,
    /// An HTML `<details>` card without a summary.
    pub details: String,
    /// A table's accessible name when it has no readable header.
    pub table: String,
}

impl Default for DocLabels {
    fn default() -> Self {
        Self::english()
    }
}

impl DocLabels {
    /// English defaults, with no translator needed.
    pub fn english() -> Self {
        Self {
            front_matter: "Front matter".into(),
            toc: "Table of contents".into(),
            invalid_mermaid: "Invalid Mermaid diagram".into(),
            copy: "Copy".into(),
            callouts: ["Note", "Tip", "Important", "Warning", "Caution"].map(str::to_owned),
            footnote_back: "↩".into(),
            footnote: "Footnote {n}".into(),
            open_externally: "Open externally".into(),
            math: "Math: {tex}".into(),
            image: "Image".into(),
            footnotes: "Footnotes".into(),
            mermaid: "Mermaid diagram".into(),
            details: "Details".into(),
            table: "Table".into(),
        }
    }

    /// The process locale's strings, when the catalog has them.
    pub fn localized() -> Self {
        Self::localized_in(lmdv_i18n::locale())
    }

    /// `locale`'s strings (an index into [`lmdv_i18n::locales`]).
    ///
    /// [`localized`](Self::localized) uses the process-wide current locale;
    /// tests and callers that must not switch it (the exports run beside the
    /// reader) use this instead. An out-of-range index falls back to English,
    /// like [`lmdv_i18n::text_in`].
    pub(crate) fn localized_in(locale: usize) -> Self {
        use lmdv_i18n::text_in;
        let mut labels = Self::english();
        labels.front_matter = text_in(locale, FRONT_MATTER).to_owned();
        labels.toc = text_in(locale, TOC).to_owned();
        labels.invalid_mermaid =
            lmdv_i18n::format_in(locale, INVALID_MERMAID, &[("err", &"")]).trim().trim_end_matches(':').to_owned();
        labels.copy = text_in(locale, COPY).to_owned();
        labels.callouts = CALLOUTS.map(|key| text_in(locale, key).to_owned());
        labels.image = text_in(locale, IMAGE).to_owned();
        labels.footnotes = text_in(locale, FOOTNOTES).to_owned();
        labels.footnote = text_in(locale, FOOTNOTE_N).to_owned();
        labels.math = text_in(locale, MATH).to_owned();
        labels.open_externally = text_in(locale, OPEN_EXTERNALLY).to_owned();
        labels.mermaid = text_in(locale, MERMAID).to_owned();
        labels.details = text_in(locale, DETAILS).to_owned();
        labels.table = text_in(locale, TABLE).to_owned();
        labels
    }
}

/// How a document is drawn: colours, fonts, spacing and labels.
///
/// Build one per appearance, not per frame; it is cheap to clone and walk but
/// the app keeps one alive while its theme is unchanged.
#[derive(Clone, Debug)]
pub struct DocTheme {
    /// Bumped by the app whenever a token changes outside the fields below; it
    /// joins the per-block memo version.
    pub version: u64,
    /// Whether the appearance is dark (chooses GitHub theme images, M-29).
    pub dark: bool,
    /// Body text.
    pub text: Color,
    /// Secondary text: list markers, code labels, footnote numbers.
    pub muted: Color,
    /// Tertiary text: missing-image alt text, fenced-code language.
    pub faint: Color,
    /// Links and the reading accent.
    pub accent: Color,
    /// A link's resting underline.
    pub link_underline: Color,
    /// The thematic-break and heading-underline rule.
    pub rule: Color,
    /// Block quotes' left rule.
    pub quote_rule: Color,
    /// Inline code's chip fill.
    pub code_bg: Color,
    /// Code blocks' frame and fill.
    pub code_border: Color,
    /// Code blocks' fill.
    pub code_fill: Color,
    /// The language label above a code block.
    pub code_label: Color,
    /// Table cell borders and the outline.
    pub table_border: Color,
    /// Table header fill.
    pub table_head_fill: Color,
    /// Table header text.
    pub table_head_text: Color,
    /// Text selection highlight (kit `Kit::selection`).
    pub selection: Color,
    /// `<mark>`'s fill.
    pub mark: Color,
    /// A find match's paint-only highlight, a translucent yellow.
    pub highlight_find: Color,
    /// The current find match, the same yellow painted stronger than the others.
    pub highlight_current: Color,
    /// A `--line` source-line highlight: a neutral tint with no colour of its own.
    pub highlight_line: Color,
    /// The heading a jump landed on (O-10): the accent at low alpha, a static
    /// tint in both motion modes (A-15).
    pub highlight_landing: Color,
    /// The callout cards' paint.
    pub callouts: [CalloutPaint; 5],
    /// The reading body: 16 pt, 1.7 line height (R-02).
    pub body: TextStyle,
    /// Heading styles for levels 1 to 6.
    pub headings: [TextStyle; 6],
    /// Fenced code.
    pub code: TextStyle,
    /// Small labels (code language, callout title, table header).
    pub small: TextStyle,
    /// Corner radius of cards, chips and tables.
    pub radius: f32,
    /// Syntax colours for fenced code.
    pub syntax: SyntaxTheme,
    /// Localized strings.
    pub labels: DocLabels,
    /// The tallest an inline image (a formula, an inline picture) may draw, in
    /// points; wider/taller pictures scale down to it.
    pub inline_image_max_height: f32,
    /// Space above a heading, per level.
    pub heading_margins: [(f32, f32); 6],
    /// Space between top-level blocks of prose, and below a paragraph.
    pub block_gap: f32,
    /// Indentation of one list level.
    pub list_indent: f32,
    /// Space between list items.
    pub list_gap: f32,
    /// Maximum width of a document image, when the app does not pass one.
    pub image_max_width: f32,
}

impl DocTheme {
    /// Derives the Markdown theme from the kit's semantic tokens.
    pub fn from_kit(k: &Kit) -> Self {
        let c = &k.c;
        let family = k.ty.ui.family;
        let body = TextStyle { family, ..TextStyle::new(16.0, 400.0) }.lh(27.2);
        let heading = |size: f32, weight: f32| TextStyle { family, ..TextStyle::new(size, weight) }.lh(size * 1.3);
        let code_family = k.ty.mono.family;
        let page = c.bg_base;
        let tint = |accent: Color| CalloutPaint { accent, fill: page.mix(accent, 0.06) };
        // The find paint is the warning token's pale yellow with a little of its
        // ink for chroma; the current match takes more of the ink and a higher
        // alpha, so it reads as the stronger of the two in both schemes.
        let highlight_find = c.warning.bg.mix(c.warning.icon, 0.2).alpha(0.5);
        let highlight_current = c.warning.bg.mix(c.warning.icon, 0.55).alpha(0.7);
        let highlight_line = c.text_base.alpha(if k.dark { 0.1 } else { 0.07 });
        // The landing band is the accent at the original's `--landing` alpha
        // (`rgba(217, 119, 87, 0.3)` on paper, 0.34 on black).
        let highlight_landing = c.text_accent.alpha(if k.dark { 0.34 } else { 0.3 });
        let (note, tip, important, warning, caution) = if k.dark {
            (
                oklch(0.70, 0.13, 250.0),
                oklch(0.72, 0.15, 145.0),
                oklch(0.71, 0.14, 295.0),
                oklch(0.76, 0.13, 80.0),
                oklch(0.68, 0.17, 25.0),
            )
        } else {
            (
                oklch(0.52, 0.14, 250.0),
                oklch(0.50, 0.13, 145.0),
                oklch(0.51, 0.16, 295.0),
                oklch(0.52, 0.12, 80.0),
                oklch(0.53, 0.17, 25.0),
            )
        };
        Self {
            version: 0,
            dark: k.dark,
            text: c.text_base,
            muted: c.text_muted,
            faint: c.text_faint,
            accent: c.text_accent,
            link_underline: c.text_accent.alpha(0.4),
            rule: c.border_muted,
            quote_rule: c.border_strong,
            code_bg: c.layer_02,
            code_border: c.border_base,
            code_fill: c.layer_01,
            code_label: c.text_faint,
            table_border: c.border_base,
            table_head_fill: c.layer_02,
            table_head_text: c.text_base,
            selection: k.selection(),
            mark: c.warning.bg,
            highlight_find,
            highlight_current,
            highlight_line,
            highlight_landing,
            callouts: [tint(note), tint(tip), tint(important), tint(warning), tint(caution)],
            body,
            headings: [
                heading(30.4, 700.0),
                heading(22.4, 650.0),
                heading(18.7, 600.0),
                heading(16.0, 600.0),
                heading(16.0, 600.0),
                heading(16.0, 600.0),
            ],
            code: TextStyle { family: code_family, ..TextStyle::new(13.5, 400.0) }.lh(21.0),
            small: TextStyle { family, ..TextStyle::new(12.0, 400.0) }.lh(16.0),
            radius: 6.0,
            syntax: SyntaxTheme::from_kit(k),
            labels: DocLabels::localized(),
            inline_image_max_height: 40.0,
            heading_margins: [(38.0, 19.0), (35.0, 15.0), (28.0, 10.0), (22.0, 8.0), (20.0, 7.0), (18.0, 6.0)],
            block_gap: 12.0,
            list_indent: 24.0,
            list_gap: 4.0,
            image_max_width: 760.0,
        }
    }

    /// The kit's neutral appearance, for tests and standalone rendering.
    pub fn neutral(dark: bool) -> Self {
        Self::from_kit(&Kit::neutral(dark))
    }

    /// The reading size in points (R-02 defaults to 16).
    pub fn size(&self) -> f32 {
        self.body.size
    }

    /// The heading style for `level` (1 to 6, clamped).
    pub fn heading(&self, level: u8) -> TextStyle {
        self.headings[(level.clamp(1, 6) - 1) as usize]
    }

    /// A cheap fingerprint of everything that changes the drawn block tree, for
    /// per-block memo versions.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.version.hash(&mut h);
        self.dark.hash(&mut h);
        for c in [
            self.text,
            self.muted,
            self.faint,
            self.accent,
            self.link_underline,
            self.rule,
            self.quote_rule,
            self.code_bg,
            self.code_border,
            self.code_fill,
            self.code_label,
            self.table_border,
            self.table_head_fill,
            self.table_head_text,
            self.selection,
            self.mark,
            self.highlight_find,
            self.highlight_current,
            self.highlight_line,
            self.highlight_landing,
        ] {
            c.hash(&mut h);
        }
        for paint in &self.callouts {
            paint.accent.hash(&mut h);
            paint.fill.hash(&mut h);
        }
        for style in std::iter::once(&self.body).chain(self.headings.iter()).chain([&self.code, &self.small]) {
            style.hash(&mut h);
        }
        self.syntax.hash(&mut h);
        for f in [
            self.radius,
            self.inline_image_max_height,
            self.block_gap,
            self.list_indent,
            self.list_gap,
            self.image_max_width,
        ] {
            f.to_bits().hash(&mut h);
        }
        for (top, bottom) in self.heading_margins {
            top.to_bits().hash(&mut h);
            bottom.to_bits().hash(&mut h);
        }
        self.labels.hash(&mut h);
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::Color;

    #[test]
    fn neutral_themes_differ_between_schemes() {
        let light = DocTheme::neutral(false);
        let dark = DocTheme::neutral(true);
        assert!(!light.dark && dark.dark);
        assert_ne!(light.text, dark.text);
        assert_ne!(light.fingerprint(), dark.fingerprint());
        // The reading body is R-02's 16 pt at a 1.7 line.
        assert_eq!(light.body.size, 16.0);
        assert!((light.body.line_height - 27.2).abs() < 0.01);
    }

    #[test]
    fn headings_clamp_and_carry_the_sizes() {
        let theme = DocTheme::neutral(false);
        assert_eq!(theme.heading(0).size, theme.heading(1).size);
        assert_eq!(theme.heading(9).size, theme.heading(6).size);
        assert!(theme.heading(1).size > theme.heading(2).size);
        assert!(theme.heading(2).size > theme.heading(3).size);
    }

    #[test]
    fn fingerprints_follow_overrides() {
        let theme = DocTheme::neutral(false);
        let red = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
        let mut other = theme.clone();
        other.text = red;
        assert_ne!(theme.fingerprint(), other.fingerprint());
        let mut bumped = theme.clone();
        bumped.version = 7;
        assert_ne!(theme.fingerprint(), bumped.fingerprint());
        assert_eq!(theme.fingerprint(), theme.clone().fingerprint());
    }

    #[test]
    fn highlight_paints_follow_the_scheme() {
        for dark in [false, true] {
            let theme = DocTheme::neutral(dark);
            // Find is a translucent yellow; the current match is stronger; a
            // source line is a faint neutral tint under both.
            assert!(theme.highlight_find.a > 0.0 && theme.highlight_find.a < 1.0);
            assert!(theme.highlight_current.a > theme.highlight_find.a);
            assert!(theme.highlight_line.a < theme.highlight_find.a);
            assert_ne!(theme.highlight_find, theme.highlight_line);
            // The landing band is the accent, visible but not as strong as a
            // find match (O-10/A-15).
            assert!(theme.highlight_landing.a > 0.0 && theme.highlight_landing.a < 1.0);
            assert_ne!(theme.highlight_landing, theme.highlight_line);
        }
        let theme = DocTheme::neutral(false);
        let mut other = theme.clone();
        other.highlight_current = Color { r: 1.0, g: 0.0, b: 0.0, a: 0.5 };
        assert_ne!(theme.fingerprint(), other.fingerprint(), "the highlight paints join the memo version");
    }

    #[test]
    fn labels_localize_without_a_catalog_error() {
        let labels = DocLabels::localized();
        assert!(!labels.front_matter.is_empty());
        assert_eq!(DocLabels::english().callouts[0], "Note");
    }

    #[test]
    fn localized_labels_follow_the_catalog_in_every_locale() {
        let italian = DocLabels::localized_in(lmdv_i18n::find_locale("it").unwrap());
        assert_eq!(italian.front_matter, "Front matter");
        assert_eq!(italian.toc, "Indice");
        assert_eq!(italian.copy, "Copia");
        assert_eq!(italian.callouts, ["Nota", "Suggerimento", "Importante", "Avviso", "Attenzione"]);
        assert_eq!(italian.footnotes, "Note a piè di pagina");
        assert_eq!(italian.image, "Immagine");
        assert_eq!(italian.mermaid, "Diagramma Mermaid");
        assert_eq!(italian.details, "Dettagli");
        assert_eq!(italian.table, "Tabella");
        // Strings without a catalog key stay as English defaults.
        assert_eq!(italian.footnote_back, "↩");
        assert_eq!(italian.open_externally, "Apri all'esterno");
        assert_eq!(italian.footnote, "Nota a piè di pagina {n}");
        assert_eq!(italian.math, "Formula: {tex}");

        let chinese = DocLabels::localized_in(lmdv_i18n::find_locale("zh-CN").unwrap());
        assert_eq!(chinese.callouts[0], "提示");
        assert_eq!(chinese.footnotes, "脚注");
        assert_eq!(chinese.footnote, "脚注 {n}");
        assert_eq!(chinese.math, "公式：{tex}");
        assert_eq!(chinese.open_externally, "在外部打开");

        // Every catalog fills every document label, and English never changes.
        for locale in 0..lmdv_i18n::locales().len() {
            let labels = DocLabels::localized_in(locale);
            assert!(labels.callouts.iter().all(|title| !title.is_empty()), "locale {locale}");
            assert!(!labels.footnotes.is_empty(), "locale {locale}");
            assert!(!labels.footnote.is_empty(), "locale {locale}");
            assert!(!labels.open_externally.is_empty(), "locale {locale}");
            assert!(!labels.math.is_empty(), "locale {locale}");
            assert!(!labels.image.is_empty(), "locale {locale}");
            assert!(!labels.mermaid.is_empty(), "locale {locale}");
            assert!(!labels.details.is_empty(), "locale {locale}");
            assert!(!labels.table.is_empty(), "locale {locale}");
        }
        let english = DocLabels::english();
        assert_eq!(english.footnote, "Footnote {n}");
        assert_eq!(english.math, "Math: {tex}");
        assert_eq!(english.open_externally, "Open externally");
        assert_eq!(DocLabels::localized_in(0), DocLabels::english());
        assert_eq!(DocLabels::default(), DocLabels::english());
    }
}
