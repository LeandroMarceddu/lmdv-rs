//! Fuzzy matching for the command palette (C-03).
//!
//! A port of the original's `fuzzy.ts`: what is typed is matched against a label
//! without accents or case. Every word of the query must be found, as one piece
//! first, else letter by letter in order (`sapdf` finds `Save as PDF`). The score
//! is fzf's small scheme: points for each matched letter, more at word starts
//! (the label's start, after a space or punctuation, a camelCase hump, a digit
//! run, every CJK character) and for letters in a row, a cost for the letters
//! skipped.
//!
//! Labels may carry *extra* words that find them too (a heading's ancestors, a
//! file's folder, the English label in another language): a word found only
//! among them counts for less, and the bold hits stay in the label.
//!
//! Folding is NFKD-like for the Latin letters the app shows: precomposed
//! accents lose their mark, `ß` becomes `ss`, the ligatures and stroked letters
//! (`æ œ ø ł đ ð þ ı`) their plain letters, full-width ASCII its half-width
//! form, and combining marks left alone are dropped. Unicode's full
//! compatibility decomposition is not embedded (no table, no dependency), so a
//! rare letter outside Latin-1/Extended-A folds as itself.

/// A label prepared for matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// The label as given.
    pub text: String,
    /// What queries are matched against: no accents, lower case, `ß` as `ss`.
    pub folded: Vec<char>,
    /// For each folded character, the char index in `text` it came from.
    pub from: Vec<usize>,
    /// For each folded character, what a match there earns on top (word starts).
    pub bonus: Vec<u8>,
}

/// A label to match, and optionally the label with more words that find it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub label: Prepared,
    pub all: Option<Prepared>,
}

/// One match: its score and the matched chars, as indices into the original text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    pub score: f64,
    pub hits: Vec<usize>,
}

// The score, as in fzf: a matched letter is worth MATCH; a gap costs GAP_START
// for its first skipped letter and GAP_EXT for each further one.
const MATCH: f64 = 16.0;
const GAP_START: f64 = 3.0;
const GAP_EXT: f64 = 1.0;
/// A letter right after the previous match earns at least this much.
const CONSECUTIVE: u8 = 4;
/// The query's first letter counts its bonus twice: where a word begins matters most.
const FIRST_MULT: f64 = 2.0;
// Where a match starts: the label's start, after a space, after punctuation, a
// CJK character (each one is a word), a camelCase hump or a digit run.
const B_START: u8 = 12;
const B_WHITE: u8 = 10;
const B_DELIM: u8 = 9;
const B_BOUNDARY: u8 = 8;
const B_CAMEL: u8 = 7;
/// Words found only among the extra ones count for less.
const EXTRA_FACTOR: f64 = 0.8;
/// Beyond these, a word is matched letter by letter the greedy way.
const MAX_WORD: usize = 32;
const MAX_TEXT: usize = 512;

const SPACE: u8 = 0;
const PUNCT: u8 = 1;
const LOWER: u8 = 2;
const UPPER: u8 = 3;
const DIGIT: u8 = 4;
/// Letters without case (Arabic, Thai…).
const LETTER: u8 = 5;
const CJK: u8 = 6;
/// Combining marks: part of the letter before them.
const MARK: u8 = 7;

/// A CJK character: each one starts a word, as the original's classes say.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2EFF
        | 0x3005
        | 0x3007
        | 0x3040..=0x30FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0x20000..=0x2FA1F)
}

/// A combining mark, by the blocks that carry them.
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn kind_of(c: char) -> u8 {
    if c.is_ascii_lowercase() {
        return LOWER;
    }
    if c.is_ascii_uppercase() {
        return UPPER;
    }
    if c.is_ascii_digit() {
        return DIGIT;
    }
    if c.is_ascii() {
        return if c == ' ' || c.is_ascii_whitespace() { SPACE } else { PUNCT };
    }
    if c.is_whitespace() {
        SPACE
    } else if is_cjk(c) {
        CJK
    } else if is_mark(c) {
        MARK
    } else if c.is_uppercase() {
        UPPER
    } else if c.is_lowercase() {
        LOWER
    } else if c.is_numeric() {
        DIGIT
    } else if c.is_alphabetic() {
        LETTER
    } else {
        PUNCT
    }
}

/// One character folded: compatibility forms undone, accents dropped, lower
/// case, then the letters with no decomposition (`ß` → `ss`).
fn fold_into(c: char, out: &mut String) {
    // Full-width ASCII and the ideographic space have half-width forms.
    let half = match c as u32 {
        0xFF01..=0xFF5E => Some(char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)),
        0x3000 => Some(' '),
        _ => None,
    };
    if let Some(half) = half {
        fold_into(half, out);
        return;
    }
    let expansion = match c {
        'ß' => Some("ss"),
        'æ' | 'Æ' => Some("ae"),
        'œ' | 'Œ' => Some("oe"),
        'ø' | 'Ø' => Some("o"),
        'ł' | 'Ł' => Some("l"),
        'đ' | 'Đ' | 'ð' | 'Ð' => Some("d"),
        'þ' | 'Þ' => Some("th"),
        'ı' => Some("i"),
        'ς' => Some("σ"),
        '‘' | '’' | '‚' | '‛' => Some("'"),
        '“' | '”' | '„' | '‟' => Some("\""),
        '–' | '—' | '―' => Some("-"),
        '…' => Some("..."),
        _ => None,
    };
    if let Some(expansion) = expansion {
        out.push_str(expansion);
        return;
    }
    // Everything else goes through the table's precomposed Latin folding, which
    // also drops a lone combining mark and lowercases.
    if let Some(folded) = crate::tables::fold_char(c) {
        for lower in folded.to_lowercase() {
            out.push(lower);
        }
    }
}

/// `text` folded for matching (accents and case gone, `ß` as `ss`).
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        fold_into(c, &mut out);
    }
    out
}

/// What a match on a character of kind `cur` earns, after one of kind `prev`
/// (`None`: the start of the label).
fn bonus_of(prev: Option<u8>, cur: u8) -> u8 {
    if cur == SPACE || cur == PUNCT || cur == MARK {
        return 0;
    }
    let Some(prev) = prev else { return B_START };
    if prev == SPACE {
        return B_WHITE;
    }
    if prev == PUNCT {
        return B_DELIM;
    }
    if cur == CJK || prev == CJK {
        return B_BOUNDARY;
    }
    if cur == UPPER && prev == LOWER {
        return B_CAMEL;
    }
    if (cur == DIGIT) != (prev == DIGIT) {
        return B_CAMEL;
    }
    0
}

/// Folds `text` for matching, keeping where each folded character came from.
pub fn prepare(text: &str) -> Prepared {
    let mut folded = Vec::with_capacity(text.len());
    let mut from = Vec::with_capacity(text.len());
    let mut bonus = Vec::with_capacity(text.len());
    let mut prev: Option<u8> = None;
    for (index, c) in text.chars().enumerate() {
        let kind = kind_of(c);
        let mut piece = String::new();
        fold_into(c, &mut piece);
        // A combining mark belongs to its letter: the next one follows that letter.
        let next_prev = if kind == MARK { prev } else { Some(kind) };
        let at = bonus_of(prev, kind);
        // "ß" becomes two letters: the first takes its place as a word start.
        for (step, folded_char) in piece.chars().enumerate() {
            from.push(index);
            bonus.push(if step == 0 { at } else { 0 });
            folded.push(folded_char);
        }
        prev = next_prev;
    }
    Prepared { text: text.to_owned(), folded, from, bonus }
}

/// The words of a query, folded as labels are; none for a blank one.
pub fn words(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in fold(query).split_whitespace() {
        if !word.is_empty() && !out.iter().any(|seen| seen == word) {
            out.push(word.to_owned());
        }
    }
    out
}

/// A label to match, with `extra` words (keywords, a folder) that find it too.
pub fn target(label: &str, extra: Option<&str>) -> Target {
    let all = extra.filter(|extra| !extra.is_empty()).map(|extra| prepare(&format!("{label}\n{extra}")));
    Target { label: prepare(label), all }
}

/// A run of `len` letters from `at`, all in a row: letters after the first keep
/// the bonus their run started with, unless a better word start comes.
fn run_score(bonus: &[u8], at: usize, len: usize) -> f64 {
    let mut chunk = bonus[at];
    let mut score = MATCH + f64::from(chunk) * FIRST_MULT;
    for step in 1..len {
        let next = bonus[at + step];
        if next >= B_BOUNDARY && next > chunk {
            chunk = next;
        }
        score += MATCH + f64::from(next.max(chunk).max(CONSECUTIVE));
    }
    score
}

/// The score of the letters of a word matched at `at` (folded indices, rising).
fn align_score(bonus: &[u8], at: &[usize]) -> f64 {
    let mut score = 0.0;
    let mut chunk = 0u8;
    for (step, &j) in at.iter().enumerate() {
        let here = bonus[j];
        if step == 0 {
            score += MATCH + f64::from(here) * FIRST_MULT;
            chunk = here;
            continue;
        }
        let gap = j - at[step - 1] - 1;
        if gap == 0 {
            if here >= B_BOUNDARY && here > chunk {
                chunk = here;
            }
            score += MATCH + f64::from(here.max(chunk).max(CONSECUTIVE));
        } else {
            score += MATCH + f64::from(here) - GAP_START - (gap as f64 - 1.0) * GAP_EXT;
            chunk = here;
        }
    }
    score
}

/// The best spread of `word`'s letters over `prepared`, in order: for each letter
/// and each place it can take, the best score so far ending there, either right
/// after the previous letter (keeping its run) or after a gap (costing it).
fn subsequence(word: &[char], prepared: &Prepared) -> Option<(f64, Vec<usize>)> {
    let folded = &prepared.folded;
    let bonus = &prepared.bonus;
    let n = folded.len();
    let m = word.len();
    if m == 0 || m > n {
        return None;
    }
    let mut first = vec![0usize; m];
    let mut last = vec![0usize; m];
    let mut at = 0usize;
    for (step, &letter) in word.iter().enumerate() {
        let found = folded[at..].iter().position(|&c| c == letter)? + at;
        first[step] = found;
        at = found + 1;
    }
    let mut at = n;
    for step in (0..m).rev() {
        let letter = word[step];
        let found = folded[..at].iter().rposition(|&c| c == letter)?;
        last[step] = found;
        at = found;
    }
    if m > MAX_WORD || n > MAX_TEXT {
        return Some((align_score(bonus, &first), first));
    }

    const NONE: f64 = f64::NEG_INFINITY;
    let mut cells = vec![NONE; m * n];
    let mut back = vec![-1i64; m * n];
    let mut chunks = vec![0u8; m * n];
    for j in first[0]..=last[0] {
        if folded[j] != word[0] {
            continue;
        }
        cells[j] = MATCH + f64::from(bonus[j]) * FIRST_MULT;
        chunks[j] = bonus[j];
    }
    for step in 1..m {
        let row = step * n;
        let previous = row - n;
        let letter = word[step];
        let lo = first[step];
        let hi = last[step];
        let from = first[step - 1];
        // The best predecessor two or more places back, with its gap paid.
        let mut gap = NONE;
        let mut gap_from = -1i64;
        let mut k = from;
        while k + 2 <= lo {
            let candidate = cells[previous + k] - GAP_START - (lo as f64 - 2.0 - k as f64) * GAP_EXT;
            if candidate > gap {
                gap = candidate;
                gap_from = k as i64;
            }
            k += 1;
        }
        for j in lo..=hi {
            if j > lo {
                gap -= GAP_EXT;
                if j >= 2 && j - 2 >= from {
                    let candidate = cells[previous + j - 2] - GAP_START;
                    if candidate > gap {
                        gap = candidate;
                        gap_from = (j - 2) as i64;
                    }
                }
            }
            if folded[j] != letter {
                continue;
            }
            let here = bonus[j];
            let mut best = NONE;
            let mut from_index = -1i64;
            let mut chunk = here;
            if j > 0 {
                let diagonal = cells[previous + j - 1];
                if diagonal > NONE {
                    let mut run = chunks[previous + j - 1];
                    if here >= B_BOUNDARY && here > run {
                        run = here;
                    }
                    best = diagonal + MATCH + f64::from(here.max(run).max(CONSECUTIVE));
                    from_index = (j - 1) as i64;
                    chunk = run;
                }
            }
            if gap > NONE && gap + MATCH + f64::from(here) > best {
                best = gap + MATCH + f64::from(here);
                from_index = gap_from;
                chunk = here;
            }
            if best > NONE {
                cells[row + j] = best;
                back[row + j] = from_index;
                chunks[row + j] = chunk;
            }
        }
    }
    // The best end (the earliest of equals), then back to the first letter.
    let row = (m - 1) * n;
    let mut end = None;
    let mut top = NONE;
    for j in first[m - 1]..=last[m - 1] {
        if cells[row + j] > top {
            top = cells[row + j];
            end = Some(j);
        }
    }
    let mut end = end?;
    let mut taken = vec![0usize; m];
    for step in (0..m).rev() {
        taken[step] = end;
        if step > 0 {
            end = usize::try_from(back[step * n + end]).ok()?;
        }
    }
    Some((top, taken))
}

/// One word: its best place as one piece (the earliest of equals), else, unless
/// only whole pieces count, its best spread letter by letter.
fn match_word(word: &[char], prepared: &Prepared, contiguous: bool) -> Option<(f64, Vec<usize>)> {
    let folded = &prepared.folded;
    let mut best = f64::NEG_INFINITY;
    let mut start: Option<usize> = None;
    if word.len() <= folded.len() {
        for at in 0..=folded.len() - word.len() {
            if folded[at..at + word.len()] != *word {
                continue;
            }
            let score = run_score(&prepared.bonus, at, word.len());
            if score > best {
                best = score;
                start = Some(at);
            }
        }
    }
    if let Some(start) = start {
        return Some((best, (start..start + word.len()).collect()));
    }
    if contiguous {
        return None;
    }
    // Letters strewn all over a long label are no match: each must earn its own
    // points despite the gaps.
    let (score, at) = subsequence(word, prepared)?;
    (score >= MATCH * word.len() as f64).then_some((score, at))
}

/// Matches every word of the query in `prepared`; `None` when one is missing.
pub fn match_all(words: &[String], prepared: &Prepared, contiguous: bool) -> Option<Match> {
    let mut score = 0.0;
    let mut hits: Vec<usize> = Vec::new();
    for word in words {
        let chars: Vec<char> = word.chars().collect();
        let (found, at) = match_word(&chars, prepared, contiguous)?;
        score += found;
        for j in at {
            let index = prepared.from[j];
            if !hits.contains(&index) {
                hits.push(index);
            }
        }
    }
    hits.sort_unstable();
    Some(Match { score, hits })
}

/// Matches `words` against a target: its label, letter by letter if need be, or
/// the label with its extra words, each word as one piece and the score
/// discounted. The hits are always in the label: there, every word it holds as
/// one piece, whichever of the two matches ranked it.
pub fn score(words: &[String], target: &Target) -> Option<Match> {
    let own = match_all(words, &target.label, false);
    let Some(all) = &target.all else { return own };
    let Some(wide) = match_all(words, all, true) else { return own };
    let discounted = wide.score * EXTRA_FACTOR;
    if own.as_ref().is_some_and(|own| own.score >= discounted) {
        return own;
    }
    let mut hits: Vec<usize> = Vec::new();
    for word in words {
        let chars: Vec<char> = word.chars().collect();
        if let Some((_, at)) = match_word(&chars, &target.label, true) {
            for j in at {
                let index = target.label.from[j];
                if !hits.contains(&index) {
                    hits.push(index);
                }
            }
        }
    }
    hits.sort_unstable();
    Some(Match { score: discounted, hits })
}

/// Splits `text` into byte ranges of matched and unmatched runs, for drawing the
/// hits in bold. A combining mark goes with its letter.
pub fn runs(text: &str, hits: &[usize]) -> Vec<(std::ops::Range<usize>, bool)> {
    let mut out: Vec<(std::ops::Range<usize>, bool)> = Vec::new();
    let mut previous_hit = false;
    for (char_index, (start, c)) in text.char_indices().enumerate() {
        let end = start + c.len_utf8();
        let hit = hits.contains(&char_index) || (previous_hit && is_mark(c));
        match out.last_mut() {
            Some((range, drawn)) if *drawn == hit => range.end = end,
            _ => out.push((start..end, hit)),
        }
        previous_hit = hit;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(query: &str, label: &str) -> Option<Match> {
        score(&words(query), &target(label, None))
    }

    #[test]
    fn accents_and_case_are_folded() {
        assert!(found("attivita", "Attività").is_some(), "an accent is not a difference");
        assert!(found("ATTIVITÀ", "attivita").is_some());
        assert!(found("strasse", "Straße").is_some(), "ß folds to ss");
        assert!(found("ae", "Ærø").is_some());
        assert!(found("full", "Ｆｕｌｌ ｗｉｄｔｈ").is_some(), "full-width letters fold");
        assert_eq!(fold("Attività"), "attivita");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("Ｈｅｌｌｏ"), "hello");
    }

    #[test]
    fn every_word_must_be_found_in_order() {
        assert!(found("read notes", "Reading notes").is_some());
        assert!(found("read missing", "Reading notes").is_none(), "every word must be there");
        assert!(found("notes missing", "Reading notes").is_none());
    }

    #[test]
    fn letters_strewn_in_order_match_a_short_label() {
        assert!(found("sapdf", "Save as PDF").is_some(), "a subsequence finds it");
        assert!(found("save pdf", "Save as PDF").is_some(), "whole words score too");
        assert!(found("pdfxx", "Save as PDF").is_none(), "letters still need their order");
    }

    #[test]
    fn word_starts_beat_letters_in_the_middle() {
        let start = found("no", "Notes").expect("matches").score;
        let middle = found("o", "Notes").expect("matches").score;
        assert!(start > middle, "a word start scores more: {start} > {middle}");
        let exact = found("save as pdf", "Save as PDF").expect("matches").score;
        let spread = found("sapdf", "Save as PDF").expect("matches").score;
        assert!(exact > spread, "the whole words score more: {exact} > {spread}");
    }

    #[test]
    fn extra_words_find_a_label_and_their_hits_stay_in_it() {
        let keyword_target = target("Stampa", Some("print share"));
        let print_words = words("print");
        let matched = score(&print_words, &keyword_target).expect("the English keyword finds it");
        assert!(matched.hits.is_empty(), "the hit is not in the label");
        let stampa_words = words("stam");
        let matched = score(&stampa_words, &keyword_target).expect("the label still matches");
        assert_eq!(matched.hits, vec![0, 1, 2, 3]);
        let direct = score(&print_words, &target("print", Some("share"))).expect("own match");
        assert!(matched.score < direct.score, "a keyword-only word counts for less");
    }

    #[test]
    fn a_heading_is_found_by_its_ancestors() {
        let heading = target("Windows", Some("Install \u{203a} Windows"));
        assert!(score(&words("install"), &heading).is_some(), "the ancestor finds the heading");
        assert!(score(&words("install windows"), &heading).is_some());
        assert!(score(&words("linux"), &heading).is_none());
    }

    #[test]
    fn hits_map_back_to_the_original_characters() {
        let prepared = prepare("Attività");
        let matched = match_all(&words("atv"), &prepared, false).expect("matches");
        assert_eq!(matched.hits, vec![0, 1, 4]);
        assert!(prepared.from.len() == prepared.folded.len());
        assert_eq!(prepared.folded.iter().collect::<String>(), "attivita");
    }

    #[test]
    fn runs_split_the_label_at_the_hits() {
        let runs = runs("Save as PDF", &[0, 1, 2, 3]);
        assert_eq!(runs.len(), 2);
        assert_eq!(&"Save as PDF"[runs[0].0.clone()], "Save");
        assert!(runs[0].1 && !runs[1].1);
    }

    #[test]
    fn a_combining_mark_goes_with_its_letter() {
        let text = "e\u{0301}xit";
        let runs = runs(text, &[0]);
        assert_eq!(&text[runs[0].0.clone()], "e\u{0301}", "the mark is drawn with the letter");
    }

    #[test]
    fn blank_queries_match_nothing_and_every_word_is_unique() {
        assert!(match_all(&[], &prepare("Save"), false).is_some());
        assert_eq!(words("  "), Vec::<String>::new());
        assert_eq!(words("Save save SAVE"), vec!["save".to_owned()]);
    }

    #[test]
    fn a_cjk_label_matches_by_character() {
        assert!(found("保存", "保存する").is_some());
        assert!(found("する", "保存する").is_some());
    }
}
