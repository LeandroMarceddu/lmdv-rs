//! The canonical, LF-normalized source buffer and its line index.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use memchr::memchr_iter;
use serde::{Deserialize, Serialize};

/// The line ending a file used. The buffer is always LF internally; this is
/// only applied when writing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Eol {
    #[default]
    Lf,
    CrLf,
}

/// The file identity captured when a document was loaded or saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct FileMeta {
    /// Length in bytes.
    pub len: u64,
    /// Modification time, nanoseconds since the Unix epoch; 0 when unknown.
    pub mtime_ns: u64,
    /// Linux `st_dev`; 0 when unknown.
    pub dev: u64,
    /// Linux `st_ino`; 0 when unknown.
    pub ino: u64,
}

impl FileMeta {
    /// Whether `(dev, ino)` can be used as a stable file identity.
    pub fn identity_known(&self) -> bool {
        self.dev != 0 && self.ino != 0
    }
}

/// One splice applied to a [`SourceBuffer`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditDelta {
    /// Byte offset where the edit started.
    pub start: usize,
    /// Bytes removed.
    pub removed: usize,
    /// Bytes inserted, after LF normalization.
    pub inserted: usize,
    /// The line containing `start`, before the edit.
    pub first_line: usize,
    /// Added lines minus removed lines.
    pub line_delta: isize,
}

/// A UTF-8, LF-normalized document buffer with an incrementally maintained
/// line index.
///
/// Invariants: `line_starts` always starts with 0 and holds the byte offset of
/// every line, including the empty final line after a trailing newline. The
/// stored text never contains `\r\n`.
#[derive(Clone, Debug)]
pub struct SourceBuffer {
    text: String,
    line_starts: Vec<u32>,
    revision: u64,
    eol: Eol,
    had_bom: bool,
}

impl SourceBuffer {
    /// Builds an empty buffer.
    pub fn empty() -> Self {
        Self { text: String::new(), line_starts: vec![0], revision: 0, eol: Eol::Lf, had_bom: false }
    }

    /// Builds a buffer from text, normalizing CRLF to LF and detecting the
    /// line ending. No BOM is assumed.
    pub fn new(text: &str) -> Self {
        let eol = detect_eol(text);
        let text = normalize_lf(text).into_owned();
        let line_starts = index_lines(&text);
        Self { text, line_starts, revision: 0, eol, had_bom: false }
    }

    /// Builds a buffer from raw file text: strips a leading UTF-8 BOM, records
    /// it, detects the line ending and normalizes CRLF to LF.
    pub fn from_file(raw: &str) -> Self {
        let (had_bom, raw) = strip_bom(raw);
        let eol = detect_eol(raw);
        let text = normalize_lf(raw).into_owned();
        let line_starts = index_lines(&text);
        Self { text, line_starts, revision: 0, eol, had_bom }
    }

    /// The LF-normalized text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Monotonic text revision; bumped by every successful `replace` or
    /// `replace_all`, even when the text ends up identical.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Number of lines; always at least one. A trailing newline creates a
    /// final empty line.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The line containing `byte`. Offsets past the end clamp to the last line.
    pub fn line_of(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len()) as u32;
        self.line_starts.partition_point(|&start| start <= byte).saturating_sub(1)
    }

    /// The byte offset where `line` starts. Out-of-range lines clamp to the
    /// last line.
    pub fn line_start(&self, line: usize) -> usize {
        let line = line.min(self.line_starts.len() - 1);
        self.line_starts[line] as usize
    }

    /// The byte range of `line`'s content, excluding its trailing newline.
    /// Out-of-range lines clamp to the last line.
    pub fn line_range(&self, line: usize) -> Range<usize> {
        let line = line.min(self.line_starts.len() - 1);
        let start = self.line_starts[line] as usize;
        let end = self.line_starts.get(line + 1).map_or(self.text.len(), |&next| next as usize - 1);
        start..end
    }

    /// Replaces `range` with `text`, normalizing CRLF in the inserted text.
    /// Returns the splice description for the caller's undo journal and views.
    ///
    /// Panics if the range is out of bounds or not on UTF-8 character
    /// boundaries; callers must validate ranges that come from user input.
    pub fn replace(&mut self, range: Range<usize>, with: &str) -> EditDelta {
        assert!(range.start <= range.end, "reversed replace range {range:?}");
        assert!(range.end <= self.text.len(), "replace range {range:?} past end {}", self.text.len());
        assert!(
            self.text.is_char_boundary(range.start) && self.text.is_char_boundary(range.end),
            "replace range {range:?} is not on character boundaries"
        );

        let first_line = self.line_of(range.start);
        let old_line_count = self.line_starts.len();
        let with = normalize_lf(with);

        self.text.replace_range(range.clone(), &with);
        self.rebuild_line_starts(range.start, first_line);
        self.revision += 1;

        let line_delta = self.line_starts.len() as isize - old_line_count as isize;
        EditDelta { start: range.start, removed: range.end - range.start, inserted: with.len(), first_line, line_delta }
    }

    /// Replaces the whole text, for a reload or a restore. The final-newline
    /// policy is preserved exactly as given. Returns the new revision.
    pub fn replace_all(&mut self, text: &str, eol: Eol) -> u64 {
        self.text = normalize_lf(text).into_owned();
        self.line_starts = index_lines(&self.text);
        self.eol = eol;
        self.revision += 1;
        self.revision
    }

    /// Records the file's line ending; the text is unchanged.
    pub fn set_eol(&mut self, eol: Eol) {
        self.eol = eol;
    }

    /// The line ending to use when writing.
    pub fn eol(&self) -> Eol {
        self.eol
    }

    /// Whether the loaded file began with a UTF-8 BOM. The BOM is stripped on
    /// read and never written back.
    pub fn had_bom(&self) -> bool {
        self.had_bom
    }

    /// Updates the remembered BOM flag, for example after a reload.
    pub fn set_had_bom(&mut self, had_bom: bool) {
        self.had_bom = had_bom;
    }

    /// A cheap immutable snapshot for a parser or export job.
    pub fn snapshot(&self) -> Arc<str> {
        Arc::from(self.text.as_str())
    }

    /// Rebuilds the line index from the start of `first_line` forward,
    /// scanning only the suffix that the edit could have changed.
    fn rebuild_line_starts(&mut self, start: usize, first_line: usize) {
        self.line_starts.truncate(first_line + 1);
        for offset in memchr_iter(b'\n', &self.text.as_bytes()[start..]) {
            self.line_starts.push((start + offset + 1) as u32);
        }
    }
}

impl Default for SourceBuffer {
    fn default() -> Self {
        Self::empty()
    }
}

/// Normalizes CRLF to LF, leaving lone CR characters untouched.
///
/// Any run of CRs immediately followed by LF collapses to one LF, so the
/// result can never contain a CRLF pair, even for `"\r\r\n"`.
pub fn normalize_lf(text: &str) -> Cow<'_, str> {
    if !text.as_bytes().contains(&b'\r') {
        return Cow::Borrowed(text);
    }

    let mut out = String::with_capacity(text.len());
    let mut carriage_returns = 0usize;
    for ch in text.chars() {
        match ch {
            '\r' => carriage_returns += 1,
            '\n' => {
                carriage_returns = 0;
                out.push('\n');
            }
            _ => {
                out.extend(std::iter::repeat_n('\r', carriage_returns));
                carriage_returns = 0;
                out.push(ch);
            }
        }
    }
    out.extend(std::iter::repeat_n('\r', carriage_returns));
    Cow::Owned(out)
}

/// Detects the file's line ending; CRLF wins when both are present.
pub fn detect_eol(text: &str) -> Eol {
    if text.as_bytes().windows(2).any(|pair| pair == b"\r\n") { Eol::CrLf } else { Eol::Lf }
}

/// Removes a leading UTF-8 BOM, reporting whether there was one.
pub fn strip_bom(text: &str) -> (bool, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, text),
    }
}

fn index_lines(text: &str) -> Vec<u32> {
    let mut starts = Vec::with_capacity(memchr_iter(b'\n', text.as_bytes()).count() + 1);
    starts.push(0);
    starts.extend(memchr_iter(b'\n', text.as_bytes()).map(|offset| (offset + 1) as u32));
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_starts(text: &str) -> Vec<u32> {
        let mut starts = vec![0];
        starts.extend(memchr_iter(b'\n', text.as_bytes()).map(|offset| (offset + 1) as u32));
        starts
    }

    fn assert_index_matches_text(buffer: &SourceBuffer) {
        let text = buffer.text();
        let expected = expected_starts(text);
        assert_eq!(buffer.line_count(), expected.len(), "line count for {text:?}");
        for (line, &start) in expected.iter().enumerate() {
            assert_eq!(buffer.line_start(line), start as usize, "start of line {line} in {text:?}");
            let end = expected.get(line + 1).map_or(text.len(), |&next| next as usize - 1);
            assert_eq!(buffer.line_range(line), start as usize..end, "range of line {line} in {text:?}");
        }
        for byte in 0..=text.len() {
            let line = expected.partition_point(|&start| start as usize <= byte).saturating_sub(1);
            assert_eq!(buffer.line_of(byte), line, "line of byte {byte} in {text:?}");
        }
    }

    #[test]
    fn empty_buffer_has_one_empty_line() {
        let buffer = SourceBuffer::empty();
        assert_eq!(buffer.text(), "");
        assert_eq!(buffer.revision(), 0);
        assert_eq!(buffer.line_count(), 1);
        assert_eq!(buffer.line_of(0), 0);
        assert_eq!(buffer.line_range(0), 0..0);
        assert_index_matches_text(&buffer);
    }

    #[test]
    fn line_index_after_multiline_replace_at_start() {
        let mut buffer = SourceBuffer::new("one\ntwo\nthree\n");
        let delta = buffer.replace(0..0, "zero\n");
        assert_eq!(buffer.text(), "zero\none\ntwo\nthree\n");
        assert_eq!(delta, EditDelta { start: 0, removed: 0, inserted: 5, first_line: 0, line_delta: 1 });
        assert_eq!(buffer.line_count(), 5);
        assert_eq!(buffer.line_of(5), 1);
        assert_eq!(buffer.line_range(1), 5..8);
        assert_eq!(buffer.revision(), 1);
        assert_index_matches_text(&buffer);
    }

    #[test]
    fn line_index_after_multiline_replace_in_middle() {
        let mut buffer = SourceBuffer::new("one\ntwo\nthree");
        let delta = buffer.replace(4..7, "two\nmore");
        assert_eq!(buffer.text(), "one\ntwo\nmore\nthree");
        assert_eq!(delta, EditDelta { start: 4, removed: 3, inserted: 8, first_line: 1, line_delta: 1 });
        assert_eq!(buffer.line_start(2), 8);
        assert_eq!(buffer.line_range(2), 8..12);
        assert_index_matches_text(&buffer);
    }

    #[test]
    fn line_index_after_multiline_replace_at_end() {
        let mut buffer = SourceBuffer::new("one\ntwo\nthree");
        let end = buffer.text().len();
        let delta = buffer.replace(end..end, "\nfour");
        assert_eq!(buffer.text(), "one\ntwo\nthree\nfour");
        assert_eq!(delta, EditDelta { start: end, removed: 0, inserted: 5, first_line: 2, line_delta: 1 });
        assert_eq!(buffer.line_count(), 4);
        assert_eq!(buffer.line_range(2), 8..13);
        assert_eq!(buffer.line_range(3), 14..18);
        assert_index_matches_text(&buffer);
    }

    #[test]
    fn line_index_after_removing_newlines() {
        let mut buffer = SourceBuffer::new("a\nb\nc\nd\n");
        let delta = buffer.replace(1..4, "");
        assert_eq!(buffer.text(), "ac\nd\n");
        assert_eq!(delta, EditDelta { start: 1, removed: 3, inserted: 0, first_line: 0, line_delta: -2 });
        assert_eq!(buffer.line_count(), 3);
        assert_index_matches_text(&buffer);
    }

    #[test]
    fn final_newline_semantics_are_preserved() {
        let with_newline = SourceBuffer::new("a\n");
        assert_eq!(with_newline.line_count(), 2);
        assert_eq!(with_newline.line_range(0), 0..1);
        assert_eq!(with_newline.line_range(1), 2..2);

        let without_newline = SourceBuffer::new("a");
        assert_eq!(without_newline.line_count(), 1);
        assert_eq!(without_newline.line_range(0), 0..1);

        let mut buffer = SourceBuffer::new("a\n");
        buffer.replace_all("a", Eol::Lf);
        assert_eq!(buffer.text(), "a");
        assert_eq!(buffer.line_count(), 1);

        buffer.replace_all("a\n", Eol::Lf);
        assert_eq!(buffer.text(), "a\n");
        assert_eq!(buffer.line_count(), 2);
    }

    #[test]
    fn crlf_is_normalized_and_remembered() {
        let mut buffer = SourceBuffer::from_file("a\r\nb\r\n");
        assert_eq!(buffer.text(), "a\nb\n");
        assert_eq!(buffer.eol(), Eol::CrLf);
        assert!(!buffer.had_bom());
        assert_eq!(buffer.line_count(), 3);
        assert_index_matches_text(&buffer);

        let delta = buffer.replace(2..2, "x\r\ny");
        assert_eq!(buffer.text(), "a\nx\nyb\n");
        assert_eq!(delta.inserted, 3);
        assert_index_matches_text(&buffer);

        buffer.set_eol(Eol::Lf);
        assert_eq!(buffer.eol(), Eol::Lf);
        assert_eq!(buffer.revision(), 1, "set_eol does not change text revision");
    }

    #[test]
    fn bom_is_stripped_and_remembered() {
        let mut buffer = SourceBuffer::from_file("\u{feff}# Title\n");
        assert_eq!(buffer.text(), "# Title\n");
        assert!(buffer.had_bom());
        assert_eq!(buffer.eol(), Eol::Lf);

        buffer.set_had_bom(false);
        assert!(!buffer.had_bom());
    }

    #[test]
    fn lone_cr_is_not_a_line_break() {
        let buffer = SourceBuffer::new("a\rb\nc");
        assert_eq!(buffer.text(), "a\rb\nc");
        assert_eq!(buffer.line_count(), 2);
        assert_eq!(buffer.line_range(0), 0..3);
    }

    #[test]
    fn crlf_runs_collapse_without_leaving_a_crlf_pair() {
        assert_eq!(normalize_lf("a\r\nb").as_ref(), "a\nb");
        assert_eq!(normalize_lf("a\r\r\nb").as_ref(), "a\nb");
        assert_eq!(normalize_lf("a\rb").as_ref(), "a\rb");
        assert_eq!(normalize_lf("a\r").as_ref(), "a\r");
    }

    #[test]
    fn replace_all_bumps_revision_and_normalizes() {
        let mut buffer = SourceBuffer::new("old");
        let revision = buffer.replace_all("new\r\ntext", Eol::CrLf);
        assert_eq!(revision, 1);
        assert_eq!(buffer.revision(), 1);
        assert_eq!(buffer.text(), "new\ntext");
        assert_eq!(buffer.eol(), Eol::CrLf);
        assert_index_matches_text(&buffer);
    }

    #[test]
    fn snapshot_is_an_owned_sendable_string() {
        fn assert_send<T: Send>(_: &T) {}
        let buffer = SourceBuffer::new("hello\n");
        let snapshot = buffer.snapshot();
        assert_send(&snapshot);
        assert_eq!(&*snapshot, "hello\n");
    }

    #[test]
    fn unicode_offsets_stay_on_character_boundaries() {
        let mut buffer = SourceBuffer::new("héllo\nwörld\n");
        let start = buffer.text().find('w').unwrap();
        let delta = buffer.replace(start..start + 'w'.len_utf8(), "W");
        assert_eq!(buffer.text(), "héllo\nWörld\n");
        assert_eq!(delta.inserted, 1);
        assert_index_matches_text(&buffer);
    }

    #[test]
    fn line_index_survives_random_edits() {
        let mut state = 0x1234_5678_9abc_def0_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let pieces = ["a", "b", "\n", "z", "\n\n"];

        let mut buffer = SourceBuffer::new("start\n");
        for _ in 0..400 {
            let len = buffer.text().len();
            let start = (next() as usize) % (len + 1);
            let end = start + (next() as usize) % (len - start + 1);
            let mut inserted = String::new();
            for _ in 0..(next() as usize % 4) {
                inserted.push_str(pieces[(next() as usize) % pieces.len()]);
            }

            let delta = buffer.replace(start..end, &inserted);
            assert_eq!(delta.start, start);
            assert_eq!(delta.removed, end - start);
            assert_eq!(delta.inserted, inserted.len());
            assert_index_matches_text(&buffer);
        }
    }
}
