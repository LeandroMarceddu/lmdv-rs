//! Linux spellchecking with Hunspell dictionaries (ADR-008, E-SPELL).
//!
//! Rune's checker answers on Windows and macOS only; on Linux it is a stub, so
//! the app owns the engine. This module is the pure model: [`discover`] finds a
//! `.aff`/`.dic` pair in the Hunspell folders, [`Spell::open`] parses it with
//! the pure-Rust `spellbook` crate, [`Spell::check`] returns misspelled byte
//! ranges and [`compose_underlines`] paints them over the syntax spans without
//! overlap. Nothing here knows about windows: the app runs discovery, loading
//! and checking on workers (ADR-017) and copies the ranges into
//! `Span::underline`.
//!
//! Discovery roots, in priority order: the app's own `dictionaries` folder,
//! `$XDG_DATA_HOME/hunspell`, `/usr/share/hunspell` and `/usr/share/myspell`.
//! The requested language comes from the UI locale when one is chosen, else
//! from `LANGUAGE`, `LANG` and `LC_ALL`; an English dictionary stands in only
//! when nothing matches the requested language. A chosen dictionary is used
//! until the next start, so changing the interface language does not swap the
//! checker mid-session (E-32).

use std::fmt;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use rune::{Color, Span};

/// The app's own dictionary folder, beside its XDG data (`dictionaries`).
pub const APP_DICTIONARY_DIR: &str = "dictionaries";

/// The most suggestions one word returns.
pub const SUGGESTION_LIMIT: usize = 8;

/// A Hunspell dictionary pair found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictionaryFiles {
    /// The file stem, e.g. `en_US`: the dictionary's language tag.
    pub language: String,
    /// The affix rules.
    pub aff: PathBuf,
    /// The word list.
    pub dic: PathBuf,
}

/// The dictionary the checker will use, and whether it had to fall back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Availability {
    /// The chosen pair.
    pub files: DictionaryFiles,
    /// The dictionary is English because no dictionary matched the requested
    /// language.
    pub fallback: bool,
}

impl Availability {
    /// The dictionary's language tag, e.g. `en_US`.
    pub fn language(&self) -> &str {
        &self.files.language
    }
}

/// Why a dictionary could not be loaded.
#[derive(Debug)]
pub enum SpellError {
    /// The `.aff` or `.dic` file could not be read.
    Io { path: PathBuf, error: std::io::Error },
    /// `spellbook` rejected the pair.
    Parse(String),
}

impl fmt::Display for SpellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Parse(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SpellError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            Self::Parse(_) => None,
        }
    }
}

/// The Hunspell folders for this machine, highest priority first.
///
/// `app_data` is the app's XDG data folder; its `dictionaries` child is where a
/// personal or bundled dictionary lives. Reading the environment is the only
/// work here; the folders themselves are scanned by [`discover`] on a worker.
pub fn dictionary_roots(app_data: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(data) = app_data {
        roots.push(data.join(APP_DICTIONARY_DIR));
    }
    if let Some(data_home) = xdg_data_home() {
        roots.push(data_home.join("hunspell"));
    }
    roots.push(PathBuf::from("/usr/share/hunspell"));
    roots.push(PathBuf::from("/usr/share/myspell"));
    roots.dedup();
    roots
}

fn xdg_data_home() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    std::env::var_os("HOME").filter(|value| !value.is_empty()).map(|home| PathBuf::from(home).join(".local/share"))
}

/// The language tags to look for, most specific first.
///
/// An explicit UI locale wins: its regional spelling and then its base
/// language (`it_IT.UTF-8` → `it_it`, `it`). With no locale chosen the
/// environment answers — `LANGUAGE`'s colon list, then `LANG`, then `LC_ALL`
/// (E-SPELL/I-05).
pub fn requested_tags(locale: Option<&str>) -> Vec<String> {
    let mut tags = Vec::new();
    match locale.map(str::trim).filter(|locale| !locale.is_empty()) {
        Some(locale) => push_tag(&mut tags, locale),
        None => {
            for variable in ["LANGUAGE", "LANG", "LC_ALL"] {
                let Ok(value) = std::env::var(variable) else { continue };
                for part in value.split(':') {
                    push_tag(&mut tags, part);
                }
            }
        }
    }
    tags
}

fn push_tag(tags: &mut Vec<String>, raw: &str) {
    let tag = normalize_tag(raw);
    if tag.is_empty() || tag == "c" || tag == "posix" {
        return;
    }
    if !tags.contains(&tag) {
        tags.push(tag.clone());
    }
    if let Some((base, _)) = tag.split_once('_')
        && !tags.iter().any(|existing| existing == base)
    {
        tags.push(base.to_owned());
    }
}

fn normalize_tag(tag: &str) -> String {
    let tag = tag.split(['.', '@']).next().unwrap_or(tag);
    tag.trim().replace('-', "_").to_ascii_lowercase()
}

/// Finds the dictionary for `tags`, with an English fallback only when the
/// requested language has none (E-SPELL).
///
/// `fallback` is true when English stood in for a requested language. Sorting
/// the folder keeps the choice deterministic when a language has several
/// regional dictionaries; an earlier root wins over a later one, so a
/// dictionary beside the app overrides the system's.
pub fn discover(roots: &[PathBuf], tags: &[String]) -> Option<Availability> {
    let index = index_roots(roots);
    if index.is_empty() {
        return None;
    }
    for tag in tags {
        if let Some(files) = pick(&index, tag) {
            return Some(Availability { files, fallback: false });
        }
    }
    pick(&index, "en").map(|files| Availability { files, fallback: !tags.is_empty() })
}

fn index_roots(roots: &[PathBuf]) -> Vec<(String, DictionaryFiles)> {
    let mut index: Vec<(String, DictionaryFiles)> = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else { continue };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let dic = entry.path();
            if !dic.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dic")) {
                continue;
            }
            let Some(stem) = dic.file_stem().and_then(|stem| stem.to_str()) else { continue };
            let aff = dic.with_extension("aff");
            if !aff.is_file() {
                continue;
            }
            let key = normalize_tag(stem);
            if key.is_empty() {
                continue;
            }
            found.push((key, DictionaryFiles { language: stem.to_owned(), aff, dic }));
        }
        found.sort_by(|left, right| left.0.cmp(&right.0));
        for (key, files) in found {
            if !index.iter().any(|(existing, _)| *existing == key) {
                index.push((key, files));
            }
        }
    }
    index
}

fn pick(index: &[(String, DictionaryFiles)], tag: &str) -> Option<DictionaryFiles> {
    let tag = normalize_tag(tag);
    if tag.is_empty() {
        return None;
    }
    if let Some((_, files)) = index.iter().find(|(stem, _)| *stem == tag) {
        return Some(files.clone());
    }
    let prefix = format!("{tag}_");
    index.iter().find(|(stem, _)| stem.starts_with(&prefix)).map(|(_, files)| files.clone())
}

/// One loaded Hunspell dictionary.
#[derive(Debug)]
pub struct Spell {
    dictionary: spellbook::Dictionary,
    language: String,
    source: Option<PathBuf>,
}

impl Spell {
    /// Parses a dictionary from the `.aff` and `.dic` text.
    pub fn new(aff: &str, dic: &str, language: &str) -> Result<Self, SpellError> {
        let dictionary = spellbook::Dictionary::new(aff, dic).map_err(|error| SpellError::Parse(error.to_string()))?;
        Ok(Self { dictionary, language: language.to_owned(), source: None })
    }

    /// Reads and parses a pair found by [`discover`]. File I/O belongs on a
    /// worker, never on the UI thread.
    pub fn open(files: &DictionaryFiles) -> Result<Self, SpellError> {
        let aff = fs::read_to_string(&files.aff).map_err(|error| SpellError::Io { path: files.aff.clone(), error })?;
        let dic = fs::read_to_string(&files.dic).map_err(|error| SpellError::Io { path: files.dic.clone(), error })?;
        let mut spell = Self::new(&aff, &dic, &files.language)?;
        spell.source = Some(files.aff.clone());
        Ok(spell)
    }

    /// The dictionary's language tag, e.g. `en_US`.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// The `.aff` file the dictionary came from, when it was opened from disk.
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    /// Whether the dictionary accepts `word`.
    pub fn is_correct(&self, word: &str) -> bool {
        self.dictionary.check(word)
    }

    /// Misspelled words of `text` as byte ranges, in text order.
    ///
    /// Fenced code, inline code, HTML tags, URLs and YAML front matter are not
    /// prose and are skipped. Words in non-Latin scripts and short all-caps
    /// acronyms are left alone: the dictionary for the UI language would flag
    /// every one of them.
    pub fn check(&self, text: &str) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let start = front_matter_end(text);
        let mut fence: Option<(char, usize)> = None;
        let mut offset = start;
        for line in text[start..].split_inclusive('\n') {
            let trimmed = line.trim_start();
            if let Some((marker, run)) = fence {
                if is_fence_close(trimmed, marker, run) {
                    fence = None;
                }
                offset += line.len();
                continue;
            }
            if let Some(open) = fence_open(trimmed) {
                fence = Some(open);
                offset += line.len();
                continue;
            }
            self.check_line(line, offset, &mut out);
            offset += line.len();
        }
        out
    }

    /// Up to [`SUGGESTION_LIMIT`] corrections for `word`.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut suggestions = Vec::new();
        self.dictionary.suggest(word, &mut suggestions);
        suggestions.truncate(SUGGESTION_LIMIT);
        suggestions
    }

    fn check_line(&self, line: &str, base: usize, out: &mut Vec<Range<usize>>) {
        let mut index = 0;
        while index < line.len() {
            let rest = &line[index..];
            if rest.starts_with('`') {
                let run = rest.chars().take_while(|c| *c == '`').count();
                let after = &rest[run..];
                index += match find_backtick_run(after, run) {
                    Some(close) => run + close + run,
                    None => run,
                };
                continue;
            }
            if rest.starts_with('<')
                && let Some(end) = rest.find('>')
            {
                index += end + 1;
                continue;
            }
            if rest.starts_with("](")
                && let Some(end) = rest.find(')')
            {
                index += end + 1;
                continue;
            }
            if url_prefix_len(rest).is_some() {
                index += skip_url(rest);
                continue;
            }
            let Some(character) = rest.chars().next() else { break };
            if character.is_alphabetic() {
                let len = word_len(rest);
                let word = &rest[..len];
                if should_check(word) && !self.dictionary.check(word) && !self.accepts_typographic_apostrophe(word) {
                    out.push(base + index..base + index + len);
                }
                index += len;
                continue;
            }
            index += character.len_utf8();
        }
    }

    fn accepts_typographic_apostrophe(&self, word: &str) -> bool {
        word.contains('’') && self.dictionary.check(&word.replace('’', "'"))
    }
}

/// Adds underline spans for `ranges` over `spans`, keeping one owner per byte
/// range.
///
/// The input spans (Markdown highlighting) are sorted and non-overlapping. A
/// mark that lands inside one splits it: the piece keeps the syntax style and
/// gains the underline. A mark in a gap becomes an underline-only span. The
/// result is sorted and non-overlapping, as `TextEdit` requires, and is never
/// cached with the syntax spans: spelling arrives on its own 300 ms cadence.
pub fn compose_underlines(spans: Vec<Span>, ranges: &[Range<usize>], color: Color) -> Vec<Span> {
    let marks = normalize_marks(ranges);
    if marks.is_empty() {
        return spans;
    }
    let mut boundaries: Vec<usize> = Vec::with_capacity(spans.len() * 2 + marks.len() * 2);
    for span in &spans {
        if span.start < span.end {
            boundaries.push(span.start as usize);
            boundaries.push(span.end as usize);
        }
    }
    for mark in &marks {
        boundaries.push(mark.start);
        boundaries.push(mark.end);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut base: Vec<&Span> = spans.iter().filter(|span| span.start < span.end).collect();
    base.sort_by_key(|span| (span.start, span.end));

    let mut out: Vec<Span> = Vec::with_capacity(boundaries.len());
    for pair in boundaries.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start >= end {
            continue;
        }
        let owner = owner_at(&base, start);
        let underlined = mark_at(&marks, start);
        match owner {
            Some(span) => {
                if !underlined && span.start as usize == start && span.end as usize == end {
                    push_merged(&mut out, span.clone());
                } else {
                    let mut piece = span.clone();
                    piece.start = start as u32;
                    piece.end = end as u32;
                    if underlined {
                        piece.underline = Some(color);
                    }
                    push_merged(&mut out, piece);
                }
            }
            None if underlined => push_merged(&mut out, Span::new(start, end).underline(color)),
            None => {}
        }
    }
    out
}

fn normalize_marks(ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut marks: Vec<Range<usize>> = ranges.iter().filter(|mark| mark.start < mark.end).cloned().collect();
    marks.sort_by_key(|mark| (mark.start, mark.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(marks.len());
    for mark in marks {
        match merged.last_mut() {
            Some(last) if mark.start <= last.end => last.end = last.end.max(mark.end),
            _ => merged.push(mark),
        }
    }
    merged
}

fn owner_at<'a>(spans: &[&'a Span], at: usize) -> Option<&'a Span> {
    let index = spans.partition_point(|span| (span.start as usize) <= at);
    index.checked_sub(1).and_then(|index| spans.get(index)).copied().filter(|span| (span.end as usize) > at)
}

fn mark_at(marks: &[Range<usize>], at: usize) -> bool {
    let index = marks.partition_point(|mark| mark.start <= at);
    index.checked_sub(1).is_some_and(|index| marks[index].end > at)
}

/// Appends `span`, extending the previous one when only the range grew.
fn push_merged(out: &mut Vec<Span>, span: Span) {
    if let Some(last) = out.last_mut()
        && last.end == span.start
        && same_style(last, &span)
    {
        last.end = span.end;
        return;
    }
    out.push(span);
}

fn same_style(left: &Span, right: &Span) -> bool {
    left.weight == right.weight
        && left.mono == right.mono
        && left.size == right.size
        && left.color == right.color
        && left.bg == right.bg
        && left.bg_pad == right.bg_pad
        && left.underline == right.underline
        && left.italic == right.italic
        && left.image.is_none()
        && right.image.is_none()
        && left.rise == right.rise
}

fn front_matter_end(text: &str) -> usize {
    let Some(rest) = text.strip_prefix("---") else { return 0 };
    if !rest.starts_with('\n') && !rest.starts_with("\r\n") {
        return 0;
    }
    let mut offset = text.find('\n').map_or(text.len(), |index| index + 1);
    while offset < text.len() {
        let end = text[offset..].find('\n').map_or(text.len(), |index| offset + index + 1);
        let line = text[offset..end].trim();
        if line == "---" || line == "..." {
            return end;
        }
        offset = end;
    }
    0
}

fn fence_open(line: &str) -> Option<(char, usize)> {
    let marker = line.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = line.chars().take_while(|character| *character == marker).count();
    (run >= 3).then_some((marker, run))
}

fn is_fence_close(line: &str, marker: char, run: usize) -> bool {
    let count = line.chars().take_while(|character| *character == marker).count();
    count >= run && line[count..].trim().is_empty()
}

fn find_backtick_run(text: &str, run: usize) -> Option<usize> {
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if rest.starts_with('`') {
            let found = rest.chars().take_while(|character| *character == '`').count();
            if found == run {
                return Some(index);
            }
            index += found;
        } else {
            index += rest.chars().next()?.len_utf8();
        }
    }
    None
}

fn word_len(text: &str) -> usize {
    let mut len = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character.is_alphabetic() {
            len = index + character.len_utf8();
        } else if (character == '\'' || character == '’')
            && len == index
            && characters.peek().is_some_and(|(_, next)| next.is_alphabetic())
        {
            // The apostrophe stays inside the word; the next letter extends it.
        } else {
            break;
        }
    }
    len
}

fn should_check(word: &str) -> bool {
    let mut letters = 0;
    for character in word.chars() {
        if character.is_alphabetic() {
            if !is_latin(character) {
                return false;
            }
            letters += 1;
        }
    }
    if letters == 0 {
        return false;
    }
    // HTML, PDF or API: not dictionary words, and every dictionary flags them.
    let acronym = letters <= 5 && word.chars().all(|character| !character.is_lowercase());
    !acronym
}

fn is_latin(character: char) -> bool {
    matches!(character as u32,
        0x0041..=0x007A       // ASCII (with the gaps below)
        | 0x00C0..=0x024F     // Latin-1 Supplement through Latin Extended-B
        | 0x1E00..=0x1EFF     // Latin Extended Additional
        | 0x2C60..=0x2C7F     // Latin Extended-C
        | 0xA720..=0xA7FF     // Latin Extended-D
    )
}

fn url_prefix_len(text: &str) -> Option<usize> {
    const PREFIXES: [&str; 6] = ["http://", "https://", "ftp://", "file://", "mailto:", "www."];
    PREFIXES.iter().find_map(|prefix| {
        text.get(..prefix.len()).filter(|head| head.eq_ignore_ascii_case(prefix)).map(|_| prefix.len())
    })
}

fn skip_url(text: &str) -> usize {
    text.find(|character: char| character.is_whitespace() || matches!(character, ')' | ']' | '>' | '"' | '\'' | '`'))
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/spell").join(name)
    }

    fn english() -> Spell {
        Spell::open(&DictionaryFiles {
            language: "en_US".to_owned(),
            aff: fixture("system/en_US.aff"),
            dic: fixture("system/en_US.dic"),
        })
        .expect("the English fixture dictionary parses")
    }

    fn italian() -> Spell {
        Spell::open(&DictionaryFiles {
            language: "it_IT".to_owned(),
            aff: fixture("system/it_IT.aff"),
            dic: fixture("system/it_IT.dic"),
        })
        .expect("the Italian fixture dictionary parses")
    }

    fn words<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|range| &text[range.clone()]).collect()
    }

    #[test]
    fn the_english_fixture_loads_and_checks_words() {
        let spell = english();
        assert_eq!(spell.language(), "en_US");
        assert!(spell.is_correct("hello"));
        assert!(spell.is_correct("markdown"));
        assert!(!spell.is_correct("wrld"));
    }

    #[test]
    fn discovery_prefers_the_requested_language_and_its_region() {
        let roots = vec![fixture("system")];
        let italian = discover(&roots, &["it_it".to_owned(), "it".to_owned()]).expect("Italian exists");
        assert_eq!(italian.language(), "it_IT");
        assert!(!italian.fallback);

        let english = discover(&roots, &["en".to_owned()]).expect("English exists");
        assert_eq!(english.language(), "en_US");
        assert!(!english.fallback);
    }

    #[test]
    fn discovery_falls_back_to_english_only_when_the_language_is_missing() {
        let roots = vec![fixture("system")];
        let spanish = discover(&roots, &["es".to_owned()]).expect("the English fallback");
        assert_eq!(spanish.language(), "en_US");
        assert!(spanish.fallback, "Spanish was requested and English stood in");

        let none = discover(&[fixture("empty")], &["es".to_owned()]);
        assert!(none.is_none(), "no dictionary at all");
    }

    #[test]
    fn the_app_dictionary_folder_wins_over_the_system() {
        let roots = vec![fixture("app/dictionaries"), fixture("system")];
        let found = discover(&roots, &["en".to_owned()]).expect("a dictionary");
        assert_eq!(found.files.aff, fixture("app/dictionaries/en_US.aff"));
        let spell = Spell::open(&found.files).expect("the app dictionary parses");
        assert!(spell.is_correct("appword"));
        assert!(!spell.is_correct("hello"), "the system dictionary did not supply the words");
    }

    #[test]
    fn a_missing_folder_is_not_an_error() {
        assert!(discover(&[fixture("does-not-exist")], &["en".to_owned()]).is_none());
        assert!(dictionary_roots(None).len() >= 2, "the system folders are always searched");
    }

    #[test]
    fn the_ui_locale_wins_and_its_region_and_base_are_tried() {
        assert_eq!(requested_tags(Some("it_IT.UTF-8")), vec!["it_it", "it"]);
        assert_eq!(requested_tags(Some("en-US")), vec!["en_us", "en"]);
        assert!(requested_tags(Some("C")).is_empty(), "C is not a language");
    }

    #[test]
    fn check_returns_byte_ranges_of_misspelled_words() {
        let spell = english();
        let text = "hello wrld, this is markdown.\nA sentnce with a typo.\n";
        let found = spell.check(text);
        assert_eq!(words(text, &found), vec!["wrld", "sentnce", "typo"]);
        for range in &found {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }
    }

    #[test]
    fn ranges_stay_in_bytes_after_a_multibyte_word() {
        let spell = english();
        let text = "café wrld naïve";
        let found = spell.check(text);
        assert_eq!(words(text, &found), vec!["café", "wrld", "naïve"]);
        assert_eq!(found, vec![0..5, 6..10, 11..17]);
    }

    #[test]
    fn check_skips_code_front_matter_tags_and_urls() {
        let spell = english();
        let text = "---\ntitle: hellp\n---\n\n\
                    `wrld` and <span class=\"wrld\">hello</span> at https://exmple.com/wrld.\n\n\
                    ```rust\nlet wrld = \"hellp\";\n```\n\n\
                    [hello](https://exmple.com/wrld)\n";
        assert!(spell.check(text).is_empty(), "{:?}", words(text, &spell.check(text)));
    }

    #[test]
    fn check_leaves_non_latin_text_and_acronyms_alone() {
        let spell = english();
        let text = "世界 привет HTML API hello wrld";
        assert_eq!(words(text, &spell.check(text)), vec!["wrld"]);
    }

    #[test]
    fn apostrophes_stay_inside_a_word_and_typographic_ones_are_accepted() {
        let text = "it's a test's don’t hello";
        let spell = english();
        // The fixture has no contractions, so each one is one range, not two.
        assert_eq!(words(text, &spell.check(text)), vec!["it's", "test's", "don’t"]);
    }

    #[test]
    fn suggestions_come_from_the_dictionary() {
        let spell = english();
        let suggestions = spell.suggest("helo");
        assert!(suggestions.iter().any(|word| word == "hello"), "{suggestions:?}");
        assert!(suggestions.len() <= SUGGESTION_LIMIT);
    }

    #[test]
    fn italian_and_english_disagree_about_their_words() {
        let italian = italian();
        let text = "ciao mondo hello";
        assert_eq!(words(text, &italian.check(text)), vec!["hello"]);
    }

    fn theme_span(start: usize, end: usize) -> Span {
        Span::new(start, end).color(Color::BLACK)
    }

    fn marks(pairs: &[(usize, usize)]) -> Vec<Range<usize>> {
        pairs.iter().map(|&(start, end)| start..end).collect()
    }

    #[test]
    fn an_underline_splits_a_syntax_span_and_keeps_its_style() {
        let spans = vec![theme_span(0, 10)];
        let out = compose_underlines(spans, &marks(&[(3, 7)]), Color::WHITE);
        assert_eq!(out.len(), 3);
        assert_eq!((out[0].start, out[0].end), (0, 3));
        assert_eq!(out[0].underline, None);
        assert_eq!((out[1].start, out[1].end), (3, 7));
        assert_eq!(out[1].underline, Some(Color::WHITE));
        assert_eq!(out[1].color, Some(Color::BLACK), "the syntax colour stays");
        assert_eq!((out[2].start, out[2].end), (7, 10));
        assert_eq!(out[2].underline, None);
    }

    #[test]
    fn an_underline_in_a_gap_is_its_own_span_and_bridges_two_spans() {
        let spans = vec![theme_span(0, 4), theme_span(8, 12)];
        let out = compose_underlines(spans, &marks(&[(2, 10)]), Color::WHITE);
        // 0..2 plain, 2..4 underlined, 4..8 underline only, 8..10 underlined,
        // 10..12 plain.
        let ranges: Vec<(usize, usize, bool)> =
            out.iter().map(|span| (span.start as usize, span.end as usize, span.underline.is_some())).collect();
        assert_eq!(ranges, vec![(0, 2, false), (2, 4, true), (4, 8, true), (8, 10, true), (10, 12, false)]);
        assert_eq!(out[2].color, None, "the gap has no syntax colour");
    }

    #[test]
    fn composition_is_sorted_and_never_overlaps() {
        let spans = vec![theme_span(0, 5), theme_span(5, 9), theme_span(12, 20)];
        let marks = vec![1..2, 4..7, 6..8, 13..15, 19..25];
        let out = compose_underlines(spans, &marks, Color::WHITE);
        for span in &out {
            assert!(span.start < span.end);
        }
        for pair in out.windows(2) {
            assert!(pair[0].end <= pair[1].start, "{:?} overlaps {:?}", pair[0], pair[1]);
        }
        assert!(out.iter().any(|span| span.underline.is_some() && span.end as usize == 25));
    }

    #[test]
    fn no_marks_returns_the_syntax_spans_untouched() {
        let spans = vec![theme_span(0, 5)];
        assert_eq!(compose_underlines(spans.clone(), &[], Color::WHITE), spans);
        let empty: Vec<Span> = Vec::new();
        assert_eq!(compose_underlines(empty, &marks(&[(2, 4)]), Color::WHITE).len(), 1);
    }
}
