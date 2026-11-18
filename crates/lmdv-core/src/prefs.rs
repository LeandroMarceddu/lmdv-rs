//! Persisted preferences: reader, theme, panels and editor.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The current preferences schema version.
pub const PREFS_SCHEMA: u32 = 1;

macro_rules! pref_enum {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $wire:literal,
            )*
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$variant_meta])* $variant, )*
        }

        impl $name {
            /// The wire spelling stored in JSON.
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $wire, )* }
            }

            /// Parses a wire spelling; unknown values are `None`.
            pub fn from_wire(value: &str) -> Option<Self> {
                match value { $( $wire => Some(Self::$variant), )* _ => None }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = Option::<String>::deserialize(deserializer)?;
                Ok(value.as_deref().and_then(Self::from_wire).unwrap_or_default())
            }
        }
    };
}

pref_enum! {
    /// The palette preference; `System` follows the OS scheme live.
    pub enum ThemePref {
        /// Follow the system light/dark setting.
        #[default]
        System => "system",
        Light => "light",
        Sepia => "sepia",
        Dark => "dark",
        Black => "black",
    }
}

pref_enum! {
    /// The reading font family.
    pub enum ReaderFont {
        #[default]
        Sans => "sans",
        Serif => "serif",
    }
}

pref_enum! {
    /// Reading line spacing.
    pub enum LineHeight {
        Compact => "compact",
        #[default]
        Normal => "normal",
        Relaxed => "relaxed",
    }
}

pref_enum! {
    /// Reading column width.
    pub enum ColumnWidth {
        Narrow => "narrow",
        #[default]
        Normal => "normal",
        Wide => "wide",
        Full => "full",
    }
}

pref_enum! {
    /// Side panel text size.
    pub enum SideSize {
        Small => "small",
        #[default]
        Normal => "normal",
        Large => "large",
    }
}

pref_enum! {
    /// Which editor layout to use, independent of the measured threshold.
    pub enum LargeFileMode {
        /// Switch to block mode only above the measured size threshold.
        #[default]
        Auto => "auto",
        /// Always use block mode.
        Block => "block",
    }
}

/// The allowed reader text sizes, in CSS pixels.
pub const SIZE_STEPS: [u16; 8] = [13, 14, 15, 16, 17, 18, 20, 22];

/// The reader text size used when no preference is stored.
pub const DEFAULT_READER_SIZE: u16 = 16;

/// The default file-browser pane width, also its minimum.
pub const LEFT_WIDTH_DEFAULT: u16 = 260;

/// The maximum file-browser pane width.
pub const LEFT_WIDTH_MAX: u16 = 560;

/// The default outline pane width, also its minimum.
pub const RIGHT_WIDTH_DEFAULT: u16 = 240;

/// The maximum outline pane width.
pub const RIGHT_WIDTH_MAX: u16 = 520;

/// Snaps a text size to the nearest allowed step. Ties choose the smaller step,
/// matching the original app.
pub fn snap_size(size: u16) -> u16 {
    SIZE_STEPS.iter().copied().min_by_key(|step| step.abs_diff(size)).unwrap_or(DEFAULT_READER_SIZE)
}

fn snap_size_f64(size: f64) -> u16 {
    if !size.is_finite() {
        return DEFAULT_READER_SIZE;
    }
    let mut best = SIZE_STEPS[0];
    for &step in &SIZE_STEPS {
        if (f64::from(step) - size).abs() < (f64::from(best) - size).abs() {
            best = step;
        }
    }
    best
}

fn deserialize_size<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u16, D::Error> {
    Ok(Option::<f64>::deserialize(deserializer)?.map_or(DEFAULT_READER_SIZE, snap_size_f64))
}

fn deserialize_left_width<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u16, D::Error> {
    let width = Option::<f64>::deserialize(deserializer)?;
    Ok(width.map_or(LEFT_WIDTH_DEFAULT, |width| clamp_width(width, LEFT_WIDTH_DEFAULT, LEFT_WIDTH_MAX)))
}

fn deserialize_right_width<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u16, D::Error> {
    let width = Option::<f64>::deserialize(deserializer)?;
    Ok(width.map_or(RIGHT_WIDTH_DEFAULT, |width| clamp_width(width, RIGHT_WIDTH_DEFAULT, RIGHT_WIDTH_MAX)))
}

fn clamp_width(width: f64, min: u16, max: u16) -> u16 {
    if !width.is_finite() {
        return min;
    }
    width.round().clamp(f64::from(min), f64::from(max)) as u16
}

fn deserialize_locale<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") { None } else { Some(trimmed.to_owned()) }
    }))
}

/// Reader typography, from the `Aa` popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReaderPrefs {
    #[serde(deserialize_with = "deserialize_size")]
    pub size: u16,
    pub font: ReaderFont,
    pub line_height: LineHeight,
    pub width: ColumnWidth,
    pub side_size: SideSize,
}

impl Default for ReaderPrefs {
    fn default() -> Self {
        Self {
            size: DEFAULT_READER_SIZE,
            font: ReaderFont::default(),
            line_height: LineHeight::default(),
            width: ColumnWidth::default(),
            side_size: SideSize::default(),
        }
    }
}

impl ReaderPrefs {
    /// Enforces the size steps and valid enums after a programmatic change.
    pub fn sanitize(&mut self) {
        self.size = snap_size(self.size);
    }

    /// Whether every value still equals the classic default look.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Global side-panel geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelPrefs {
    #[serde(deserialize_with = "deserialize_left_width")]
    pub left_width: u16,
    #[serde(deserialize_with = "deserialize_right_width")]
    pub right_width: u16,
    pub left_hidden: bool,
    pub right_hidden: bool,
}

impl Default for PanelPrefs {
    fn default() -> Self {
        Self {
            left_width: LEFT_WIDTH_DEFAULT,
            right_width: RIGHT_WIDTH_DEFAULT,
            left_hidden: false,
            right_hidden: false,
        }
    }
}

impl PanelPrefs {
    /// Clamps stored widths to the range the UI can actually use.
    pub fn sanitize(&mut self) {
        self.left_width = self.left_width.clamp(LEFT_WIDTH_DEFAULT, LEFT_WIDTH_MAX);
        self.right_width = self.right_width.clamp(RIGHT_WIDTH_DEFAULT, RIGHT_WIDTH_MAX);
    }
}

/// Editor behavior preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorPrefs {
    /// Spellcheck is on unless explicitly disabled.
    pub spellcheck: bool,
    pub large_file_mode: LargeFileMode,
    pub format_bar: bool,
}

impl Default for EditorPrefs {
    fn default() -> Self {
        Self { spellcheck: true, large_file_mode: LargeFileMode::Auto, format_bar: true }
    }
}

/// The application preferences stored in `prefs.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Schema version; unknown fields are ignored by serde.
    pub schema: u32,
    pub theme: ThemePref,
    pub reader: ReaderPrefs,
    pub panels: PanelPrefs,
    /// Printing starts a new page at every heading 1.
    pub print_break_h1: bool,
    /// `None` follows the system locale; `sanitize` also maps `auto`/blank to
    /// `None`, so persisted values never mean "auto" as a string.
    #[serde(deserialize_with = "deserialize_locale")]
    pub locale: Option<String>,
    /// Session restore is an intentional extension and on by default.
    pub restore_session: bool,
    pub editor: EditorPrefs,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            schema: PREFS_SCHEMA,
            theme: ThemePref::default(),
            reader: ReaderPrefs::default(),
            panels: PanelPrefs::default(),
            print_break_h1: false,
            locale: None,
            restore_session: true,
            editor: EditorPrefs::default(),
        }
    }
}

impl Prefs {
    /// Normalizes everything a hand-edited or older file could get wrong.
    pub fn sanitize(&mut self) {
        if self.schema == 0 {
            self.schema = PREFS_SCHEMA;
        }
        self.reader.sanitize();
        self.panels.sanitize();
        if let Some(locale) = self.locale.take() {
            let trimmed = locale.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
                self.locale = None;
            } else {
                self.locale = Some(trimmed.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn defaults_match_the_classic_look() {
        let prefs = Prefs::default();
        assert_eq!(prefs.schema, PREFS_SCHEMA);
        assert_eq!(prefs.theme, ThemePref::System);
        assert_eq!(prefs.reader.size, 16);
        assert_eq!(prefs.reader.font, ReaderFont::Sans);
        assert_eq!(prefs.reader.line_height, LineHeight::Normal);
        assert_eq!(prefs.reader.width, ColumnWidth::Normal);
        assert_eq!(prefs.reader.side_size, SideSize::Normal);
        assert_eq!(prefs.panels.left_width, 260);
        assert_eq!(prefs.panels.right_width, 240);
        assert!(!prefs.panels.left_hidden);
        assert!(!prefs.panels.right_hidden);
        assert!(!prefs.print_break_h1);
        assert_eq!(prefs.locale, None);
        assert!(prefs.restore_session);
        assert!(prefs.editor.spellcheck);
        assert_eq!(prefs.editor.large_file_mode, LargeFileMode::Auto);
        assert!(prefs.editor.format_bar);
    }

    #[test]
    fn empty_json_uses_every_default() {
        let prefs: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs, Prefs::default());
    }

    #[test]
    fn prefs_round_trip_through_json() {
        let prefs = Prefs {
            theme: ThemePref::Sepia,
            reader: ReaderPrefs {
                size: 20,
                font: ReaderFont::Serif,
                line_height: LineHeight::Relaxed,
                width: ColumnWidth::Wide,
                side_size: SideSize::Large,
            },
            panels: PanelPrefs { left_width: 400, right_hidden: true, ..PanelPrefs::default() },
            print_break_h1: true,
            locale: Some("it".into()),
            restore_session: false,
            editor: EditorPrefs { spellcheck: false, format_bar: false, ..EditorPrefs::default() },
            ..Prefs::default()
        };

        let text = serde_json::to_string_pretty(&prefs).unwrap();
        let parsed: Prefs = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, prefs);

        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], json!(PREFS_SCHEMA));
        assert_eq!(value["theme"], json!("sepia"));
        assert_eq!(value["reader"]["size"], json!(20));
        assert_eq!(value["editor"]["large_file_mode"], json!("auto"));
    }

    #[test]
    fn unknown_enum_values_fall_back_to_defaults() {
        let prefs: Prefs = serde_json::from_str(
            r#"{
                "theme": "neon",
                "reader": {"font": "cursive", "line_height": "airy", "width": "enormous", "side_size": "huge"}
            }"#,
        )
        .unwrap();
        assert_eq!(prefs.theme, ThemePref::System);
        assert_eq!(prefs.reader.font, ReaderFont::Sans);
        assert_eq!(prefs.reader.line_height, LineHeight::Normal);
        assert_eq!(prefs.reader.width, ColumnWidth::Normal);
        assert_eq!(prefs.reader.side_size, SideSize::Normal);
    }

    #[test]
    fn null_values_fall_back_to_defaults() {
        let prefs: Prefs = serde_json::from_str(r#"{"theme": null, "reader": {"size": null, "font": null}}"#).unwrap();
        assert_eq!(prefs.theme, ThemePref::System);
        assert_eq!(prefs.reader.size, DEFAULT_READER_SIZE);
        assert_eq!(prefs.reader.font, ReaderFont::Sans);
    }

    #[test]
    fn reader_sizes_snap_on_deserialize_and_sanitize() {
        for (stored, snapped) in [
            (12.0, 13),
            (13.0, 13),
            (16.4, 16),
            (18.6, 18),
            (19.0, 18),
            (19.2, 20),
            (21.0, 20),
            (400.0, 22),
            (-2.0, 13),
        ] {
            let prefs: Prefs = serde_json::from_value(json!({"reader": {"size": stored}})).unwrap();
            assert_eq!(prefs.reader.size, snapped, "size {stored} should snap to {snapped}");
        }

        let mut reader = ReaderPrefs { size: 19, ..ReaderPrefs::default() };
        reader.sanitize();
        assert_eq!(reader.size, 18);
        reader.size = 2;
        reader.sanitize();
        assert_eq!(reader.size, 13);
        reader.size = 99;
        reader.sanitize();
        assert_eq!(reader.size, 22);
    }

    #[test]
    fn panel_widths_clamp_to_default_and_max() {
        let prefs: Prefs = serde_json::from_str(r#"{"panels": {"left_width": 9999, "right_width": 1}}"#).unwrap();
        assert_eq!(prefs.panels.left_width, LEFT_WIDTH_MAX);
        assert_eq!(prefs.panels.right_width, RIGHT_WIDTH_DEFAULT);

        let mut panels = PanelPrefs { left_width: 100, right_width: 900, ..PanelPrefs::default() };
        panels.sanitize();
        assert_eq!(panels.left_width, LEFT_WIDTH_DEFAULT);
        assert_eq!(panels.right_width, RIGHT_WIDTH_MAX);
    }

    #[test]
    fn missing_reader_fields_keep_their_defaults() {
        let prefs: Prefs = serde_json::from_str(r#"{"reader": {"size": 20}}"#).unwrap();
        assert_eq!(prefs.reader.size, 20);
        assert_eq!(prefs.reader.font, ReaderFont::Sans);
        assert_eq!(prefs.reader.line_height, LineHeight::Normal);
    }

    #[test]
    fn locale_auto_and_blank_normalize_to_none() {
        let mut prefs = Prefs { locale: Some(" auto ".into()), ..Prefs::default() };
        prefs.sanitize();
        assert_eq!(prefs.locale, None);

        prefs.locale = Some("  ".into());
        prefs.sanitize();
        assert_eq!(prefs.locale, None);

        prefs.locale = Some(" it ".into());
        prefs.sanitize();
        assert_eq!(prefs.locale.as_deref(), Some("it"));

        let parsed: Prefs = serde_json::from_str(r#"{"locale": "auto"}"#).unwrap();
        assert_eq!(parsed.locale, None);
        let parsed: Prefs = serde_json::from_str(r#"{"locale": " it "}"#).unwrap();
        assert_eq!(parsed.locale.as_deref(), Some("it"));
    }

    #[test]
    fn schema_zero_is_upgraded_by_sanitize() {
        let mut prefs: Prefs = serde_json::from_str(r#"{"schema": 0}"#).unwrap();
        prefs.sanitize();
        assert_eq!(prefs.schema, PREFS_SCHEMA);
    }

    #[test]
    fn enum_wire_names_round_trip() {
        for theme in [ThemePref::System, ThemePref::Light, ThemePref::Sepia, ThemePref::Dark, ThemePref::Black] {
            assert_eq!(ThemePref::from_wire(theme.as_str()), Some(theme));
        }
        assert_eq!(ThemePref::from_wire("neon"), None);
        assert_eq!(ThemePref::default().as_str(), "system");
    }
}
