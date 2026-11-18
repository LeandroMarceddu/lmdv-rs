//! Tab indentation for the source editor (E-33).
//!
//! The original keymap binds Tab to CodeMirror's `indentWithTab`: Tab indents
//! and Shift+Tab outdents the selected lines, never moving keyboard focus.
//! [`indent_with_tab`] reproduces that with CodeMirror's default two-space
//! indent unit, as one text change so one undo step covers it.
//!
//! A caret (or a selection inside one line) indents that line; a multi-line
//! selection indents every line it touches. Our selection mapping mirrors
//! CodeMirror's: both ends ride their lines, and a selection ending exactly at
//! the next line's start does not include that line.

use rune::TextEdit;

/// CodeMirror's default indent unit.
pub const INDENT_UNIT: usize = 2;

/// Whether `event` is a Tab with no chord modifiers. Shift is the outdent
/// variant, so it is allowed.
pub fn plain_tab(event: &rune::KeyEvent) -> bool {
    event.key == rune::Key::Tab && !event.mods.cmd && !event.mods.ctrl && !event.mods.alt
}

/// Indents (`outdent == false`) or outdents the selected lines. Returns
/// whether the text changed; an outdent with nothing to remove is a no-op.
pub fn indent_with_tab(edit: &mut TextEdit, outdent: bool) -> bool {
    // Work on a copy: `edit` is mutated below and `text` would stay borrowed.
    let text = edit.text.clone();
    let (anchor, focus) = (edit.anchor.min(text.len()), edit.focus.min(text.len()));
    let (start, end) = if anchor <= focus { (anchor, focus) } else { (focus, anchor) };

    let lines = selected_lines(&text, start, end);
    let Some(&first) = lines.first() else { return false };
    let last = *lines.last().unwrap_or(&first);
    let region_start = first;
    let region_end = text[last..].find('\n').map_or(text.len(), |offset| last + offset + 1);
    let old = &text[region_start..region_end];

    if outdent {
        let mut removals = Vec::with_capacity(lines.len());
        let mut new = String::with_capacity(old.len());
        let mut line_start = region_start;
        let mut changed = false;
        for line in old.split_inclusive('\n') {
            let count = line.bytes().take(INDENT_UNIT).take_while(|&byte| byte == b' ').count();
            removals.push((line_start, count));
            changed |= count > 0;
            new.push_str(&line[count..]);
            line_start += line.len();
        }
        if !changed {
            return false;
        }
        edit.set_selection(region_start, region_end);
        edit.insert(&new);
        let new_anchor = map_outdent(anchor, &removals);
        let new_focus = map_outdent(focus, &removals);
        edit.set_selection(new_anchor, new_focus);
        return true;
    }

    let mut new = String::with_capacity(old.len() + lines.len() * INDENT_UNIT);
    if old.is_empty() {
        // The empty line at the end of a document still takes an indent.
        new.push_str(&" ".repeat(INDENT_UNIT));
    } else {
        for line in old.split_inclusive('\n') {
            new.push_str(&" ".repeat(INDENT_UNIT));
            new.push_str(line);
        }
    }
    edit.set_selection(region_start, region_end);
    edit.insert(&new);
    let map = |position: usize| {
        let inserted = lines.iter().filter(|&&line_start| line_start <= position).count();
        position + inserted * INDENT_UNIT
    };
    let new_anchor = map(anchor);
    let new_focus = map(focus);
    edit.set_selection(new_anchor, new_focus);
    true
}

/// The starts of the lines a selection touches. An empty selection touches the
/// caret's line; a line whose start is exactly the selection end is not touched.
fn selected_lines(text: &str, start: usize, end: usize) -> Vec<usize> {
    let first = line_start(text, start);
    let mut lines = vec![first];
    if end <= first {
        return lines;
    }
    let mut line = first;
    while let Some(offset) = text[line..].find('\n') {
        let next = line + offset + 1;
        if next >= end {
            break;
        }
        lines.push(next);
        line = next;
    }
    lines
}

fn line_start(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    text[..byte].rfind('\n').map_or(0, |offset| offset + 1)
}

/// Maps a source position through the outdent deletions, whose starts are
/// original-text offsets. A position inside a removed indent lands at the
/// line's new start; a position at or before a line start is only shifted by
/// earlier deletions.
fn map_outdent(position: usize, removals: &[(usize, usize)]) -> usize {
    let mut removed = 0;
    for &(line_start, count) in removals {
        if count == 0 {
            continue;
        }
        if line_start >= position {
            break;
        }
        if position <= line_start + count {
            return line_start - removed;
        }
        removed += count;
    }
    position.saturating_sub(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(edit: &TextEdit) -> &str {
        &edit.text
    }

    #[test]
    fn tab_indents_the_caret_line_by_two_spaces() {
        let mut edit = TextEdit::new("one two");
        edit.set_caret(3);
        assert!(indent_with_tab(&mut edit, false));
        assert_eq!(text(&edit), "  one two");
        assert_eq!(edit.range(), (5, 5), "the caret rides its line");

        assert!(indent_with_tab(&mut edit, false));
        assert_eq!(text(&edit), "    one two");
        assert_eq!(edit.range(), (7, 7));
    }

    #[test]
    fn a_multiline_selection_indents_every_line_it_touches() {
        let mut edit = TextEdit::new("one\ntwo\nthree");
        edit.set_selection(1, 9); // mid-"one" through "three"'s first byte
        assert!(indent_with_tab(&mut edit, false));
        assert_eq!(text(&edit), "  one\n  two\n  three");
        assert_eq!(edit.range(), (3, 15));
    }

    #[test]
    fn a_selection_ending_at_the_next_line_start_does_not_indent_it() {
        let mut edit = TextEdit::new("one\ntwo\n");
        edit.set_selection(0, 4);
        assert!(indent_with_tab(&mut edit, false));
        assert_eq!(text(&edit), "  one\ntwo\n");
        assert_eq!(edit.range(), (2, 6));
    }

    #[test]
    fn shift_tab_removes_up_to_two_leading_spaces() {
        let mut edit = TextEdit::new("    one\n  two\none");
        edit.set_selection(0, edit.text.len());
        assert!(indent_with_tab(&mut edit, true));
        assert_eq!(text(&edit), "  one\ntwo\none");
        assert_eq!(edit.range(), (0, 13));

        assert!(indent_with_tab(&mut edit, true));
        assert_eq!(text(&edit), "one\ntwo\none");
        let before = edit.text.clone();
        assert!(!indent_with_tab(&mut edit, true), "column 0 has nothing to remove");
        assert_eq!(edit.text, before);
    }

    #[test]
    fn a_one_space_indent_loses_one_space_and_the_caret_stays_sensible() {
        let mut edit = TextEdit::new(" x");
        edit.set_caret(1);
        assert!(indent_with_tab(&mut edit, true));
        assert_eq!(text(&edit), "x");
        assert_eq!(edit.range(), (0, 0), "a caret inside the removed indent lands at the line start");
    }

    #[test]
    fn an_empty_document_takes_one_indent() {
        let mut edit = TextEdit::new("");
        assert!(indent_with_tab(&mut edit, false));
        assert_eq!(text(&edit), "  ");
        assert_eq!(edit.range(), (2, 2));
    }

    #[test]
    fn only_a_plain_tab_counts() {
        use rune::{Key, KeyEvent, Mods};

        assert!(plain_tab(&KeyEvent { key: Key::Tab, mods: Mods::default() }));
        assert!(plain_tab(&KeyEvent { key: Key::Tab, mods: Mods { shift: true, ..Mods::default() } }));
        assert!(!plain_tab(&KeyEvent { key: Key::Tab, mods: Mods { ctrl: true, cmd: true, ..Mods::default() } }));
        assert!(!plain_tab(&KeyEvent { key: Key::Text("\t".to_owned()), mods: Mods::default() }));
    }
}
