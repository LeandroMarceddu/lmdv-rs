//! Reading time (R-04/R-05): the words and CJK characters a document draws, the
//! minutes they take, and the label that follows the active tab's name.
//!
//! The count walks the parsed model, so Markdown markers, link destinations and
//! fence delimiters never count while headings, prose, code, tables, drawn HTML,
//! footnotes, image alt text and front matter do. Formulas, Mermaid source and
//! the footnote section's own labels draw no prose and are skipped.
//!
//! Speed: 220 words a minute, 500 CJK characters a minute, rounded up, never
//! below one ([`ReadingStats::minutes`]). A CJK character ends a word, exactly as
//! the original's `\p{Script=Han}|Hiragana|Katakana|Hangul` replacement does, and
//! a word is a run of letters, digits, `_`, apostrophes or hyphens.

use lmdv_md::{Block, FmValue, HtmlNode, Inline, InlineObjectKind, MdDoc};

/// Words a minute (R-04).
pub const WORDS_PER_MIN: usize = 220;
/// CJK characters a minute (R-04).
pub const CJK_PER_MIN: usize = 500;
/// The placeholder a heading with no text goes by (O-08).
pub const EMPTY_HEADING: &str = "\u{a7}";
/// The gap between the tab's name and its reading time (the pill's gap).
pub const TAB_META_GAP: f32 = 6.0;
/// The strip's trailing action and its gap: the plus button takes both.
pub const TAB_META_TRAILING: f32 = 34.0;
/// Spare room a hidden reading time must find before it comes in (T-35).
pub const TAB_META_SLACK_OFF: f32 = 2.0;
/// Spare room it keeps while shown, so it does not come and go (T-35).
pub const TAB_META_SLACK_ON: f32 = 0.5;
/// The title fades over its last points (`tab.rs`'s `FADE`); a label inside this
/// much of the title's end is still read whole.
pub const TAB_TITLE_FADE: f32 = 16.0;

/// Whether the reading time fits after the strip's last tab (T-35/R-05).
///
/// `list_width` is the tabs' laid-out width, which already includes the time
/// while `on`. A hidden time needs its own width plus a little slack; a shown
/// one only needs the strip not to squeeze a tab.
pub fn tab_meta_fits(strip_width: f32, list_width: f32, trailing: f32, meta_width: f32, on: bool) -> bool {
    let room = strip_width - list_width - trailing;
    let slack = if on { TAB_META_SLACK_ON } else { TAB_META_SLACK_OFF };
    meta_width + slack <= room
}

/// The tab's label with the reading time after its name, shortened if need be.
///
/// The kit's tab is at most 224 points wide and fades its title's last 16, so a
/// long name would eat the time. The name is cut with an ellipsis until the
/// time fits whole (the original faded the name and kept the time); `None` when
/// even a two-character name and the time cannot share the tab, so the strip
/// hides the time instead of clipping it.
pub fn fit_tab_label(title: &str, meta: &str, title_space: f32, font: rune::TextStyle) -> Option<String> {
    let measure = |text: &str| rune::text::measure(text, &[], font, None).0;
    let full = format!("{title}  {meta}");
    if measure(&full) + TAB_TITLE_FADE <= title_space {
        return Some(full);
    }
    let mut chars: Vec<char> = title.chars().collect();
    while chars.len() > 2 {
        chars.pop();
        let short: String = chars.iter().collect();
        let candidate = format!("{short}\u{2026}  {meta}");
        if measure(&candidate) + TAB_TITLE_FADE <= title_space {
            return Some(candidate);
        }
    }
    None
}

/// What a document takes to read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadingStats {
    /// Runs of letters, digits, `_`, apostrophes and hyphens, CJK excluded.
    pub words: usize,
    /// Han, Hiragana, Katakana and Hangul characters.
    pub cjk: usize,
    /// `ceil(words / 220 + cjk / 500)`, at least one.
    pub minutes: usize,
}

impl ReadingStats {
    /// What the word tooltip counts: words plus CJK characters (R-04).
    pub fn total(&self) -> usize {
        self.words + self.cjk
    }

    /// The full form for one tab and for the tab's description: “4 min read”.
    pub fn full(&self) -> String {
        lmdv_i18n::t!("msg.minRead", "n" => self.minutes)
    }

    /// The short form among several tabs: “4 min” (R-05).
    pub fn short(&self) -> String {
        lmdv_i18n::t!("tab.minutes", "n" => self.minutes)
    }

    /// The hover card's and the accessible description's line: “4 min read · 440 words”.
    pub fn description(&self) -> String {
        let words = lmdv_i18n::number(self.total() as i64);
        format!("{} \u{b7} {}", self.full(), lmdv_i18n::t!("msg.words", "n" => words))
    }
}

/// Whether `c` is one of the four CJK scripts R-04 counts.
pub fn is_cjk(c: char) -> bool {
    matches!(
        u32::from(c),
        0x1100..=0x11FF       // Hangul Jamo
            | 0x2E80..=0x2FDF // CJK radicals, Kangxi radicals
            | 0x3005 | 0x3007 // 々 and 〇
            | 0x3021..=0x3029 // Hangzhou numerals
            | 0x3040..=0x309F // Hiragana
            | 0x30A0..=0x30FF // Katakana
            | 0x3130..=0x318F // Hangul compatibility Jamo
            | 0x31F0..=0x31FF // Katakana phonetic extensions
            | 0x3400..=0x4DBF // CJK extension A
            | 0x4E00..=0x9FFF // CJK unified ideographs
            | 0xA960..=0xA97F // Hangul Jamo extended A
            | 0xAC00..=0xD7AF // Hangul syllables
            | 0xD7B0..=0xD7FF // Hangul Jamo extended B
            | 0xF900..=0xFAFF // CJK compatibility ideographs
            | 0xFF66..=0xFF9D // halfwidth Katakana
            | 0x20000..=0x3FFFF // CJK extensions B and later
    )
}

/// The original's `[\p{L}\p{N}_'’-]` word character.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '\'' | '\u{2019}' | '-')
}

/// Counts one run of drawn text into `stats`.
///
/// Every call starts a new word, so two blocks' texts never merge into one word.
/// CJK characters are counted one by one and break the word around them.
pub fn count_text(text: &str, stats: &mut ReadingStats) {
    let mut in_word = false;
    for c in text.chars() {
        if is_cjk(c) {
            stats.cjk += 1;
            in_word = false;
        } else if is_word(c) {
            if !in_word {
                stats.words += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }
}

/// The reading time of a document (R-04).
pub fn reading_stats(doc: &MdDoc) -> ReadingStats {
    let mut stats = ReadingStats::default();
    for block in &doc.blocks {
        count_block(block, &mut stats);
    }
    for footnote in &doc.footnotes {
        for block in &footnote.blocks {
            count_block(block, &mut stats);
        }
    }
    if let Some(front) = &doc.front_matter {
        for (key, value) in &front.entries {
            count_text(key, &mut stats);
            match value {
                FmValue::Scalar(text) => count_text(text, &mut stats),
                FmValue::List(items) => {
                    for item in items {
                        count_text(item, &mut stats);
                    }
                }
            }
        }
    }
    // A [TOC] block draws the outline headings below it again, so the article's
    // text has them twice (the original measured the drawn article).
    for (index, block) in doc.blocks.iter().enumerate() {
        if !matches!(block, Block::Toc) {
            continue;
        }
        for heading in &doc.headings {
            if heading.in_toc && heading.block != usize::MAX && heading.block > index {
                count_text(&heading.text, &mut stats);
            }
        }
    }
    stats.minutes =
        ((stats.words as f64 / WORDS_PER_MIN as f64) + (stats.cjk as f64 / CJK_PER_MIN as f64)).ceil() as usize;
    stats.minutes = stats.minutes.max(1);
    stats
}

fn count_block(block: &Block, stats: &mut ReadingStats) {
    match block {
        Block::Para(inline) | Block::Heading { inline, .. } => count_inline(inline, stats),
        Block::Code { text, .. } => count_text(text, stats),
        Block::List { items, .. } => {
            for item in items {
                for block in item {
                    count_block(block, stats);
                }
            }
        }
        Block::Quote { blocks, .. } => {
            for block in blocks {
                count_block(block, stats);
            }
        }
        Block::Table(table) => {
            for cell in &table.head {
                count_inline(cell, stats);
            }
            for row in &table.rows {
                for cell in row {
                    count_inline(cell, stats);
                }
            }
        }
        Block::HtmlBlock(node) => count_html(node, stats),
        Block::Image(image) => count_text(&image.alt, stats),
        // Formulas, diagram source, rules, the TOC placeholder and the footnote
        // section's labels draw no prose of their own.
        Block::Mermaid { .. } | Block::MathBlock { .. } | Block::Rule | Block::Toc | Block::FootnoteDefs => {}
    }
}

fn count_inline(inline: &Inline, stats: &mut ReadingStats) {
    count_text(&inline.text, stats);
}

fn count_html(node: &HtmlNode, stats: &mut ReadingStats) {
    count_text(&node.text(), stats);
}

/// A heading's name for the section line: its text with runs of whitespace
/// collapsed, its image alt text when it has no text, or “§” (O-08).
pub fn heading_line(doc: &MdDoc, index: usize) -> String {
    let Some(heading) = doc.headings.get(index) else {
        return EMPTY_HEADING.to_owned();
    };
    let text = heading.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        return text;
    }
    // A heading that is only a picture goes by its alt text, as the outline does.
    let inline = match doc.blocks.get(heading.block) {
        Some(Block::Heading { inline, .. }) => Some(inline),
        _ => None,
    };
    let alt = inline.map(image_alts).unwrap_or_default();
    if alt.is_empty() { EMPTY_HEADING.to_owned() } else { alt }
}

/// The alt text of every image in a heading that draws no text.
fn image_alts(inline: &Inline) -> String {
    inline
        .objects
        .iter()
        .filter(|object| object.kind == InlineObjectKind::Image)
        .filter_map(|object| object.html.as_ref())
        .filter_map(|node| node.attr("alt"))
        .map(|alt| alt.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|alt| !alt.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(text: &str) -> ReadingStats {
        reading_stats(&lmdv_md::parse(text))
    }

    fn count(text: &str) -> (usize, usize) {
        let mut stats = ReadingStats::default();
        count_text(text, &mut stats);
        (stats.words, stats.cjk)
    }

    #[test]
    fn words_are_runs_of_letters_digits_and_joined_punctuation() {
        assert_eq!(count("Hello, world 123"), (3, 0));
        assert_eq!(count("don't stop"), (2, 0));
        assert_eq!(count("well-known caf\u{e9} na\u{ef}ve"), (3, 0));
        assert_eq!(count("snake_case_name"), (1, 0));
        assert_eq!(count("one\u{2019}s"), (1, 0));
        assert_eq!(count("-- --"), (2, 0), "a run of hyphens is a word, as the original regex reads it");
    }

    #[test]
    fn cjk_is_counted_per_character_and_breaks_the_word() {
        assert_eq!(count("\u{4f60}\u{597d}\u{4e16}\u{754c}"), (0, 4));
        assert_eq!(count("Hello \u{4e16}\u{754c} again"), (2, 2));
        assert_eq!(count("\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}"), (0, 5));
        assert_eq!(count("\u{30ab}\u{30bf}\u{30ab}\u{30ca}"), (0, 4));
        assert_eq!(count("\u{d55c}\u{ad6d}\u{c5b4}"), (0, 3));
        assert!(!is_cjk('A'), "Latin is not CJK");
        assert!(!is_cjk('\u{00e9}'), "accented Latin is not CJK");
    }

    #[test]
    fn minutes_round_up_and_never_fall_below_one() {
        assert_eq!(stats(""), ReadingStats { words: 0, cjk: 0, minutes: 1 });
        assert_eq!(stats(&"word ".repeat(219)).minutes, 1);
        assert_eq!(stats(&"word ".repeat(220)).minutes, 1);
        assert_eq!(stats(&"word ".repeat(221)).minutes, 2);
        assert_eq!(stats(&"\u{4e16}".repeat(500)).minutes, 1);
        assert_eq!(stats(&"\u{4e16}".repeat(501)).minutes, 2);
        let mixed = ReadingStats { words: 220, cjk: 500, minutes: 2 };
        assert_eq!(mixed.total(), 720);
    }

    #[test]
    fn marks_link_targets_and_fences_never_count() {
        let doc = lmdv_md::parse(
            "# Head **strong**\n\n[link](https://example.com/never-counted)\n\n```rust\nfn main() {}\n```\n\n| A | B |\n| --- | --- |\n| cell | other |\n\n![alt words](./picture.png)\n",
        );
        let stats = reading_stats(&doc);
        assert!(stats.words >= 10, "head, strong, link text, code, cells and alt: {stats:?}");
        // The URL is not in the drawn text, so the total is far below a count of
        // the raw source.
        assert!(stats.words < doc.source.split_whitespace().count());
    }

    #[test]
    fn footnotes_front_matter_and_toc_headings_are_counted_once() {
        let plain = stats("# Title\n\nProse with a note.[^1]\n\n[^1]: Footnote words here.\n");
        assert!(plain.words >= 7, "title, prose and footnote: {plain:?}");
        let without_front = stats("# Title\n\nProse.\n");
        let front = stats("---\ntitle: Painted Words\ntags: [one, two]\n---\n\n# Title\n\nProse.\n");
        assert_eq!(front.words, without_front.words + 6, "the card draws its keys and values");
        let toc = stats("# Title\n\n[TOC]\n\n## Alpha\n\nProse.\n\n## Beta\n");
        let without = stats("# Title\n\n## Alpha\n\nProse.\n\n## Beta\n");
        assert_eq!(toc.words, without.words + 2, "the TOC draws Alpha and Beta again, not the title above it");
    }

    #[test]
    fn the_description_carries_the_full_time_and_word_count() {
        let stats = ReadingStats { words: 440, cjk: 0, minutes: 2 };
        assert_eq!(stats.full(), "2 min read");
        assert_eq!(stats.short(), "2 min");
        assert_eq!(stats.description(), "2 min read \u{b7} 440 words");
    }

    #[test]
    fn a_long_name_gives_way_to_the_time_in_the_tabs_width() {
        let font = rune::TextStyle::new(13.0, 500.0);
        assert_eq!(fit_tab_label("Notes", "1 min", 166.0, font).as_deref(), Some("Notes  1 min"));
        let long = fit_tab_label("A rather long document name", "2 min read", 166.0, font);
        let long = long.expect("the time stays visible");
        assert!(long.ends_with(" 2 min read"), "after the name: {long}");
        assert!(long.contains('\u{2026}'), "the name gives way: {long}");
        assert!(fit_tab_label("A rather long document name", "12 min read", 40.0, font).is_none());
    }

    #[test]
    fn a_tab_keeps_its_time_only_while_the_strip_has_room() {
        // 1000 points of strip, 500 of tabs, 34 for the plus: 466 free.
        assert!(tab_meta_fits(1000.0, 500.0, TAB_META_TRAILING, 60.0, false));
        // A full strip: 16 free is not enough for the time.
        assert!(!tab_meta_fits(600.0, 550.0, TAB_META_TRAILING, 60.0, false));
        // 61 free: a shown time keeps its 0.5-point margin, a hidden one wants 2.
        assert!(tab_meta_fits(635.0, 540.0, TAB_META_TRAILING, 60.0, true));
        assert!(!tab_meta_fits(635.0, 540.0, TAB_META_TRAILING, 60.0, false));
    }

    #[test]
    fn a_heading_line_collapses_whitespace_and_falls_back_to_the_section_sign() {
        let doc = lmdv_md::parse("#   Spaced   *out*   name  \n\n## ![Picture](./a.png)\n\n### \u{00a0}\n");
        assert_eq!(heading_line(&doc, 0), "Spaced out name");
        assert_eq!(heading_line(&doc, 1), "Picture", "an image-only heading goes by its alt text");
        assert_eq!(heading_line(&doc, 2), EMPTY_HEADING, "a heading with nothing drawn is §");
        assert_eq!(heading_line(&doc, 99), EMPTY_HEADING);
    }
}
