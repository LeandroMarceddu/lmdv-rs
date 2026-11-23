//! Localized strings for lmdv.
//!
//! The six catalogs are the original app's dictionaries plus the document
//! labels the native reader and exporters draw, in `locales/app/<id>.json`
//! (636 keys each, `en` is the source locale). The build
//! script turns them into a [`rune_i18n::Catalog`] embedded in the binary;
//! nothing is parsed or allocated at launch.
//!
//! ```
//! use lmdv_i18n::{key, text};
//!
//! const SAVE: lmdv_i18n::Key = key("toolbar.save");
//! assert_eq!(text(SAVE), "Save (Ctrl+S)");
//! ```
//!
//! Keys are checked at compile time: [`key`] is a `const fn`, and the [`t!`]
//! macro resolves its name in a `const`, so a typo fails the build. Lookups
//! fall back to the English source catalog when the current locale has no
//! message for a key (the generated catalog does this, see [`text_in`]).
//!
//! The catalogs use the original single-brace placeholders (`{name}`), so the
//! [`format()`]/[`format_in`] helpers and [`t!`] fill those, not rune-i18n's
//! `{{name}}` templates. The original `t()` replaced only the arguments it was
//! given and left every other placeholder literal; this port does the same.
//!
//! # The app's entry points
//!
//! * On launch: [`negotiate_system`] (unless a saved preference exists), then
//!   [`set_locale`]; the returned index is the locale for every later lookup.
//! * Per frame: `t!` for constant strings; [`text`]/[`format()`] when the key is a
//!   value; [`number`], [`decimal`] and [`list`] for locale-aware formatting.
//! * The language picker: [`locales`] (id, BCP 47 tag, native label), then
//!   [`set_locale`] with the chosen id.
//!
//! The six full help pages live in `locales/help/<id>.html`; [`help_page`]
//! embeds them at build time and falls back to English for an unknown id.

#![forbid(unsafe_code)]

use core::fmt::Display;

use rune_i18n::I18n;
pub use rune_i18n::{Key, LocaleInfo, PluralCategory, PluralKey};

// Defines KEYS, PLURALS and CATALOG; see rune_i18n::codegen.
include!(concat!(env!("OUT_DIR"), "/i18n.rs"));

/// The process-wide translator. Every window shares it; the current locale is
/// an atomic, so a change is visible to the next lookup in every tab.
pub static I18N: I18n = I18n::new(&CATALOG);

/// Resolves an English key name to a [`Key`], for use in a `const` initializer.
///
/// A name missing from the catalog makes the `const` fail to compile:
///
/// ```
/// const SAVE: lmdv_i18n::Key = lmdv_i18n::key("toolbar.save");
/// ```
///
/// Calling it at runtime with an unknown name panics; prefer the [`t!`] macro
/// or a `const` so the check happens at build time.
pub const fn key(name: &str) -> Key {
    Key::named(KEYS, name)
}

/// Resolves a plural base name to a [`PluralKey`], for use in a `const`.
///
/// A plural base is a family of `base.one`/`base.other` messages in the source
/// catalog. The relative-time units (`rel.minutes`, `rel.hours`, `rel.days`)
/// are the first set; a name without those messages fails to compile when it
/// is resolved in a `const`.
pub const fn plural_key(name: &str) -> PluralKey {
    PluralKey::named(PLURALS, name)
}

/// The current locale's message for `key`, or English when it has none.
pub fn text(key: Key) -> &'static str {
    I18N.text(key)
}

/// `key`'s message in `locale` (an index into [`locales`]), or English when
/// that locale has none or `locale` is out of range.
pub fn text_in(locale: usize, key: Key) -> &'static str {
    I18N.text_in(locale, key)
}

/// The current locale's message with its `{name}` placeholders filled from
/// `args`; unknown placeholders stay as written.
pub fn format(key: Key, args: &[(&str, &dyn Display)]) -> String {
    render(I18N.text(key), args)
}

/// [`format()`] in an explicit locale (an index into [`locales`]).
pub fn format_in(locale: usize, key: Key, args: &[(&str, &dyn Display)]) -> String {
    render(I18N.text_in(locale, key), args)
}

/// The current locale's message for plural base `key` and `count`'s CLDR
/// category, with `other` as the fallback. Placeholders are not filled.
pub fn plural(key: PluralKey, count: u64) -> &'static str {
    I18N.plural(key, count)
}

/// [`plural`] with `{count}` and `args` filled in.
pub fn plural_format(key: PluralKey, count: u64, args: &[(&str, &dyn Display)]) -> String {
    let mut all: Vec<(&str, &dyn Display)> = args.to_vec();
    all.push(("count", &count));
    render(I18N.plural(key, count), &all)
}

/// The CLDR plural category (`one`, `other`, …) of `count` in the current
/// locale; what [`plural`] selects on.
pub fn plural_category(count: u64) -> PluralCategory {
    I18N.plural_category(count)
}

/// Makes the locale with `id` (`"en"`, `"zh-CN"`, …) current.
///
/// Returns whether `id` is a supported locale; an unknown id leaves the current
/// locale unchanged and returns `false`.
pub fn set_locale(id: &str) -> bool {
    let Some(index) = I18N.find_locale(id) else {
        return false;
    };
    I18N.set_locale(index);
    true
}

/// The current locale's index into [`locales`]; `0` is English.
pub fn locale() -> usize {
    I18N.locale()
}

/// The current locale's id (`"en"`, `"zh-CN"`, …).
pub fn locale_id() -> &'static str {
    I18N.info().id()
}

/// The full help page for `locale_id` (an id such as `"en"` or `"zh-CN"`).
///
/// The six `locales/help/<id>.html` pages are embedded at build time. An
/// unknown id gets the English page, the source locale, so the Help dialog
/// always has something to draw.
pub fn help_page(locale_id: &str) -> &'static str {
    match locale_id {
        "it" => HELP_IT,
        "de" => HELP_DE,
        "fr" => HELP_FR,
        "es" => HELP_ES,
        "zh-CN" => HELP_ZH_CN,
        _ => HELP_EN,
    }
}

const HELP_EN: &str = include_str!("../../../locales/help/en.html");
const HELP_IT: &str = include_str!("../../../locales/help/it.html");
const HELP_DE: &str = include_str!("../../../locales/help/de.html");
const HELP_FR: &str = include_str!("../../../locales/help/fr.html");
const HELP_ES: &str = include_str!("../../../locales/help/es.html");
const HELP_ZH_CN: &str = include_str!("../../../locales/help/zh-CN.html");

/// The current locale: id, BCP 47 tag, native label and `rtl`.
pub fn locale_info() -> LocaleInfo {
    I18N.info()
}

/// Every supported locale, English first. The language picker iterates this.
pub fn locales() -> &'static [LocaleInfo] {
    I18N.locales()
}

/// The index of the locale whose id is `id`.
pub fn find_locale(id: &str) -> Option<usize> {
    I18N.find_locale(id)
}

/// The supported locale that best matches `preferred` (BCP 47 tags, most
/// wanted first), or English (`0`) when nothing matches.
///
/// Language and likely script are matched (`de-AT` picks `de`, `fr-FR` picks
/// `fr`). As in the original app, any Chinese tag picks `zh-CN`, the only
/// Chinese catalog: `zh-Hans`, `zh-TW` and `zh-Hant-HK` all resolve to it.
pub fn negotiate<'a>(preferred: impl IntoIterator<Item = &'a str>) -> usize {
    let preferred = preferred.into_iter().map(|tag| {
        let chinese = tag.get(..2).is_some_and(|prefix| prefix.eq_ignore_ascii_case("zh"));
        if chinese { "zh-CN" } else { tag }
    });
    I18N.negotiate(preferred)
}

/// [`negotiate`] over the OS's preferred languages (POSIX locale variables on
/// Linux), or English (`0`) when none match.
pub fn negotiate_system() -> usize {
    let languages = rune_i18n::system_languages();
    negotiate(languages.iter().map(String::as_str))
}

/// `n` with the current locale's grouping (`1,234`, `1.234`, `1 234`).
pub fn number(n: i64) -> String {
    I18N.integer(n)
}

/// `n` with at most `max_fraction` fraction digits in the current locale
/// (`3.5`, `3,5`).
pub fn decimal(n: f64, max_fraction: u8) -> String {
    I18N.decimal(n, max_fraction)
}

/// `items` joined as the current locale's long conjunction (`a, b, and c`,
/// `a, b und c`).
pub fn list<S: AsRef<str>>(items: &[S]) -> String {
    I18N.list(items)
}

/// Fills single-brace `{name}` placeholders in `template` from `args`.
///
/// Arguments fill every occurrence; a name with no argument is left literal,
/// matching the original `t()`. A `{` without a closing `}` stays literal.
pub fn render(template: &str, args: &[(&str, &dyn Display)]) -> String {
    if args.is_empty() {
        return template.to_owned();
    }
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let name = &after[..close];
        if name.is_empty() || name.contains('{') {
            out.push('{');
            rest = after;
            continue;
        }
        match args.iter().find(|(arg, _)| *arg == name) {
            Some((_, value)) => {
                out.push_str(&value.to_string());
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The current locale's message, with a key name checked at compile time.
///
/// ```
/// use lmdv_i18n::t;
///
/// assert_eq!(t!("toolbar.save"), "Save (Ctrl+S)");
/// let line = t!("pane.lineN", "n" => 12);
/// assert_eq!(line, "Line 12");
/// ```
#[macro_export]
macro_rules! t {
    ($name:literal $(,)?) => {{
        const KEY: $crate::Key = $crate::key($name);
        $crate::text(KEY)
    }};
    ($name:literal, $($arg:literal => $value:expr),+ $(,)?) => {{
        const KEY: $crate::Key = $crate::key($name);
        $crate::format(KEY, &[$(($arg, &$value as &dyn ::core::fmt::Display)),+])
    }};
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard};

    use serde_json::Value;

    use super::*;

    const EXPECTED_LOCALES: [(&str, &str, &str); 6] = [
        ("en", "en", "English"),
        ("it", "it", "Italiano"),
        ("de", "de", "Deutsch"),
        ("fr", "fr", "Français"),
        ("es", "es", "Español"),
        ("zh-CN", "zh-CN", "中文"),
    ];

    /// Tests that read or write the process-wide current locale take this lock;
    /// every one of them sets the locale it needs, so ordering does not matter.
    fn locale_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn locales_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales")
    }

    fn disk_catalog(id: &str) -> serde_json::Map<String, Value> {
        let path = locales_dir().join("app").join(format!("{id}.json"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        match serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display())) {
            Value::Object(map) => map,
            other => panic!("{}: not a JSON object: {other}", path.display()),
        }
    }

    #[test]
    fn locales_are_the_original_six_with_english_first() {
        let _guard = locale_lock();
        let ids: Vec<(&str, &str, &str)> =
            locales().iter().map(|locale| (locale.id(), locale.tag(), locale.label())).collect();
        assert_eq!(ids, EXPECTED_LOCALES);
        assert!(locales().iter().all(|locale| !locale.rtl()));
        assert_eq!(locale_info().id(), "en", "English is the source and starts current");
        assert_eq!(KEYS.len(), 636);
        assert_eq!(
            PLURALS,
            ["editor.blocks", "editor.lines", "rel.days", "rel.hours", "rel.minutes"],
            "the editor counts and relative-time units are CLDR plurals"
        );
    }

    #[test]
    fn every_disk_catalog_has_exactly_the_generated_keys() {
        let catalog_keys: BTreeSet<&str> = KEYS.iter().copied().collect();
        assert_eq!(catalog_keys.len(), KEYS.len(), "KEYS has no repeats");

        for (index, locale) in locales().iter().enumerate() {
            let catalog = disk_catalog(locale.id());
            let disk_keys: BTreeSet<&str> = catalog.keys().map(String::as_str).collect();
            assert_eq!(disk_keys.len(), catalog.len(), "{} has no repeated key", locale.id());
            assert_eq!(
                disk_keys,
                catalog_keys,
                "{}:{} must have the same key set as the generated catalog",
                locale.id(),
                index
            );
            for (key, message) in &catalog {
                let message = message.as_str().unwrap_or_else(|| panic!("{}: {key} is not a string", locale.id()));
                assert!(!message.is_empty(), "{}: {key} is empty", locale.id());
                assert_eq!(
                    text_in(index, Key::named(KEYS, key)),
                    message,
                    "{}: the generated catalog changed the value of {key}",
                    locale.id()
                );
            }
            for name in KEYS {
                let found = Key::named(KEYS, name);
                assert_eq!(found.index(), KEYS.binary_search(name).unwrap());
                assert!(
                    !text_in(index, found).is_empty(),
                    "{}: {name} has no message in the generated catalog",
                    locale.id()
                );
            }
        }
    }

    #[test]
    fn relative_time_units_are_plural_and_translated() {
        let _guard = locale_lock();
        let unit = |key: &str, n: u64| {
            let count = number(n as i64);
            let template = plural(plural_key(key), n);
            render(template, &[("n", &count)])
        };
        assert!(set_locale("en"));
        assert_eq!(unit("rel.minutes", 12), "12 min");
        assert_eq!(unit("rel.hours", 3), "3 h");
        assert_eq!(unit("rel.days", 5), "5 d");

        assert!(set_locale("de"));
        assert_eq!(unit("rel.days", 5), "5 T.");
        assert!(set_locale("it"));
        assert_eq!(unit("rel.days", 5), "5 g");
        assert!(set_locale("zh-CN"));
        assert_eq!(unit("rel.minutes", 12), "12 分钟", "Chinese uses its other form");
        assert!(set_locale("en"));
    }

    #[test]
    fn document_labels_are_translated_in_every_catalog() {
        const LABELS: [&str; 14] = [
            "render.calloutNote",
            "render.calloutTip",
            "render.calloutImportant",
            "render.calloutWarning",
            "render.calloutCaution",
            "render.footnotes",
            "render.footnoteN",
            "render.footnoteBack",
            "render.image",
            "render.mermaid",
            "render.math",
            "render.details",
            "render.table",
            "render.openExternally",
        ];
        for (index, locale) in locales().iter().enumerate() {
            for name in LABELS {
                assert!(!text_in(index, key(name)).is_empty(), "{}: {name} has no document label", locale.id());
            }
        }
        let italian = find_locale("it").unwrap();
        assert_eq!(text_in(italian, key("render.calloutNote")), "Nota");
        assert_eq!(text_in(italian, key("render.footnotes")), "Note a piè di pagina");
        assert_eq!(text_in(italian, key("render.footnoteN")), "Nota a piè di pagina {n}");
        assert_eq!(text_in(italian, key("render.footnoteBack")), "Torna al riferimento {n}");
        assert_eq!(text_in(italian, key("render.mermaid")), "Diagramma Mermaid");
        assert_eq!(text_in(italian, key("render.math")), "Formula: {tex}");
        assert_eq!(text_in(italian, key("render.openExternally")), "Apri all'esterno");
        let chinese = find_locale("zh-CN").unwrap();
        assert_eq!(text_in(chinese, key("render.calloutCaution")), "注意");
        assert_eq!(text_in(chinese, key("render.table")), "表格");
        assert_eq!(text_in(chinese, key("render.footnoteN")), "脚注 {n}");
        assert_eq!(text_in(chinese, key("render.footnoteBack")), "返回引用 {n}");
        assert_eq!(text_in(chinese, key("render.math")), "公式：{tex}");
        assert_eq!(text_in(chinese, key("render.openExternally")), "在外部打开");
        assert_eq!(text_in(0, key("render.image")), "Image");
    }

    #[test]
    fn document_label_templates_keep_their_placeholders() {
        const TEMPLATES: [(&str, &str); 3] =
            [("render.footnoteN", "{n}"), ("render.math", "{tex}"), ("render.footnoteBack", "{n}")];
        for (index, locale) in locales().iter().enumerate() {
            for (name, placeholder) in TEMPLATES {
                let message = text_in(index, key(name));
                assert_eq!(
                    message.matches('{').count(),
                    1,
                    "{}: {name} must carry exactly one placeholder: {message}",
                    locale.id()
                );
                assert!(message.contains(placeholder), "{}: {name} lacks {placeholder}: {message}", locale.id());
            }
        }
        assert_eq!(format_in(0, key("render.footnoteN"), &[("n", &3)]), "Footnote 3");
        assert_eq!(format_in(0, key("render.math"), &[("tex", &"x^2")]), "Math: x^2");
        assert_eq!(format_in(0, key("render.footnoteBack"), &[("n", &3)]), "Back to reference 3");
        assert_eq!(format_in(find_locale("it").unwrap(), key("render.math"), &[("tex", &"x^2")]), "Formula: x^2");
        assert_eq!(format_in(find_locale("zh-CN").unwrap(), key("render.footnoteBack"), &[("n", &3)]), "返回引用 3");
    }

    #[test]
    fn source_locale_is_english_and_fallback_is_english() {
        assert_eq!(text_in(0, key("toolbar.save")), "Save (Ctrl+S)");
        // An out-of-range locale has no messages, so rune-i18n falls back to the
        // source catalog rather than returning an empty string.
        assert_eq!(text_in(999, key("toolbar.save")), "Save (Ctrl+S)");
    }

    #[test]
    fn samples_in_italian_german_and_chinese() {
        assert_eq!(text_in(find_locale("it").unwrap(), key("toolbar.save")), "Salva (Ctrl+S)");
        assert_eq!(text_in(find_locale("it").unwrap(), key("pane.lineN")), "Riga {n}");
        assert_eq!(text_in(find_locale("de").unwrap(), key("toolbar.save")), "Speichern (Strg+S)");
        assert_eq!(text_in(find_locale("de").unwrap(), key("msg.save")), "Speichern");
        assert_eq!(text_in(find_locale("de").unwrap(), key("msg.dontSave")), "Nicht speichern");
        assert_eq!(text_in(find_locale("de").unwrap(), key("msg.discard")), "Verwerfen");
        assert_eq!(text_in(find_locale("zh-CN").unwrap(), key("toolbar.save")), "保存 (Ctrl+S)");
        assert_eq!(text_in(find_locale("zh-CN").unwrap(), key("pane.lineN")), "第 {n} 行");
        assert_eq!(text_in(find_locale("zh-CN").unwrap(), key("toolbar.windowTitle")), "{name} — lmdv");
    }

    #[test]
    fn switching_locale_changes_every_later_lookup() {
        let _guard = locale_lock();
        assert!(set_locale("en"));
        assert_eq!(text(key("toolbar.read")), "Back to reading (Ctrl+E)");

        assert!(set_locale("it"));
        assert_eq!(locale_id(), "it");
        assert_eq!(locale_info().label(), "Italiano");
        assert_eq!(text(key("toolbar.read")), "Torna alla lettura (Ctrl+E)");

        assert!(set_locale("zh-CN"));
        assert_eq!(locale_id(), "zh-CN");
        assert_eq!(text(key("msg.discardTitle")), "未保存的更改");

        assert!(set_locale("en"));
        assert_eq!(locale_id(), "en");
    }

    #[test]
    fn unknown_locale_is_rejected_and_changes_nothing() {
        let _guard = locale_lock();
        assert!(set_locale("fr"));
        assert!(!set_locale("xx"));
        assert!(!set_locale(""));
        assert_eq!(find_locale("xx"), None);
        assert_eq!(locale_id(), "fr", "a rejected id leaves the current locale alone");
        assert!(set_locale("en"));
    }

    #[test]
    fn negotiation_matches_language_and_falls_back_to_english() {
        assert_eq!(negotiate(["fr-FR", "en-US"]), find_locale("fr").unwrap());
        assert_eq!(negotiate(["de-AT"]), find_locale("de").unwrap());
        assert_eq!(negotiate(["it_IT"]), find_locale("it").unwrap());
        assert_eq!(negotiate(["zh-Hans-CN"]), find_locale("zh-CN").unwrap());
        assert_eq!(negotiate(["zh-TW"]), find_locale("zh-CN").unwrap(), "the original maps any zh* here");
        assert_eq!(negotiate(["zh-Hant-HK"]), find_locale("zh-CN").unwrap());
        assert_eq!(negotiate(["xx-YY"]), 0);
        assert_eq!(negotiate(std::iter::empty()), 0);
        assert!(negotiate_system() < locales().len());
    }

    #[test]
    fn posix_locale_variables_are_understood() {
        let values = [("LANGUAGE", "fr_FR.UTF-8:de_DE.UTF-8"), ("LC_ALL", ""), ("LC_MESSAGES", ""), ("LANG", "C")];
        let tags = rune_i18n::posix_languages(|name| {
            values.iter().find(|(variable, _)| *variable == name).map(|(_, value)| (*value).to_owned())
        });
        assert_eq!(tags, ["fr-FR", "de-DE"], "C and POSIX are dropped");
        assert_eq!(negotiate(tags.iter().map(String::as_str)), find_locale("fr").unwrap());
    }

    #[test]
    fn format_fills_single_brace_placeholders() {
        assert_eq!(format_in(0, key("toolbar.windowTitle"), &[("name", &"notes.md")]), "notes.md — lmdv");
        assert_eq!(format_in(0, key("pane.lineN"), &[("n", &12)]), "Line 12");
        assert_eq!(format_in(0, key("zoom.rowsOf"), &[("n", &3), ("total", &10)]), "3 of 10 rows");
        // Missing arguments stay literal, as in the original `t()`.
        assert_eq!(format_in(0, key("pane.lineN"), &[("other", &1)]), "Line {n}");
        assert_eq!(format_in(0, key("pane.lineN"), &[]), "Line {n}");
        // A repeated placeholder fills every occurrence.
        assert_eq!(
            format_in(0, key("msg.closeWindowMixedQ"), &[("changed", &"a.md"), ("pasted", &"draft")]),
            "Unsaved changes: a.md. Pasted text in no file yet: draft. Save everything before closing the window?"
        );
        // Italian keeps the same placeholder syntax and its own word order.
        assert_eq!(format_in(find_locale("it").unwrap(), key("pane.lineN"), &[("n", &7)]), "Riga 7");
    }

    #[test]
    fn format_leaves_lone_braces_alone() {
        assert_eq!(render("{", &[("n", &1)]), "{");
        assert_eq!(render("} and {", &[("n", &1)]), "} and {");
        assert_eq!(render("{}", &[("n", &1)]), "{}");
        assert_eq!(render("{ n }", &[("n", &1)]), "{ n }", "names are exact, spaces included");
    }

    #[test]
    fn numbers_and_lists_follow_the_locale() {
        let _guard = locale_lock();

        // CLDR's minimum-grouping-digits: `it` leaves four-digit numbers
        // ungrouped (`1234`) and groups five (`12.345`), like `es`; `de`, `en`
        // and `zh-CN` group at four.
        assert!(set_locale("en"));
        assert_eq!(number(1234), "1,234");
        assert_eq!(number(12345), "12,345");
        assert_eq!(decimal(3.5, 1), "3.5");
        assert_eq!(list(&["a", "b", "c"]), "a, b, and c");

        assert!(set_locale("it"));
        assert_eq!(number(1234), "1234");
        assert_eq!(number(12345), "12.345");
        assert_eq!(decimal(3.5, 1), "3,5");
        assert_eq!(list(&["a", "b", "c"]), "a, b e c");

        assert!(set_locale("de"));
        assert_eq!(number(1234), "1.234");
        assert_eq!(decimal(3.5, 1), "3,5");
        assert_eq!(list(&["a", "b", "c"]), "a, b und c");

        assert!(set_locale("zh-CN"));
        assert_eq!(number(1234), "1,234");

        assert!(set_locale("en"));
    }

    #[test]
    fn plural_categories_follow_cldr() {
        let _guard = locale_lock();

        assert!(set_locale("en"));
        assert_eq!(plural_category(1), PluralCategory::One);
        assert_eq!(plural_category(0), PluralCategory::Other);
        assert_eq!(plural_category(2), PluralCategory::Other);

        assert!(set_locale("zh-CN"));
        assert_eq!(plural_category(1), PluralCategory::Other, "Chinese has no singular form");

        assert!(set_locale("fr"));
        assert_eq!(plural_category(1), PluralCategory::One);

        assert!(set_locale("en"));
    }

    #[test]
    fn key_names_are_checked_at_compile_time() {
        const SAVE: Key = key("toolbar.save");
        assert_eq!(text_in(0, SAVE), "Save (Ctrl+S)");
        assert_eq!(key("toolbar.save"), SAVE);
        assert_eq!(key("caption.group").index(), KEYS.binary_search(&"caption.group").unwrap());
    }

    #[test]
    fn t_macro_reads_and_formats() {
        let _guard = locale_lock();
        assert!(set_locale("en"));
        assert_eq!(t!("toolbar.save"), "Save (Ctrl+S)");
        assert_eq!(t!("pane.lineN"), "Line {n}");
        assert_eq!(t!("pane.lineN", "n" => 12), "Line 12");
        assert_eq!(t!("zoom.rowsOf", "n" => 3, "total" => 10), "3 of 10 rows");

        assert!(set_locale("de"));
        assert_eq!(t!("toolbar.save"), "Speichern (Strg+S)");
        assert_eq!(t!("toolbar.windowTitle", "name" => "notizen.md"), "notizen.md — lmdv");
        assert!(set_locale("en"));
    }

    #[test]
    fn help_pages_are_embedded_for_every_locale() {
        assert!(help_page("en").len() > 1_000, "the English page looks empty");
        for (id, _, _) in EXPECTED_LOCALES {
            let page = help_page(id);
            assert!(page.len() > 1_000, "{id} looks empty");
            assert!(page.contains("<p"), "{id} is not a help page");
        }
        for (id, _, _) in EXPECTED_LOCALES.iter().filter(|(id, _, _)| *id != "en") {
            assert_ne!(help_page(id), help_page("en"), "{id} must not reuse the English page");
        }
        assert_eq!(help_page("xx"), help_page("en"), "unknown ids fall back to English");
        assert_eq!(help_page("en"), HELP_EN);
    }
}
