//! Small source-text helpers shared by the parser passes.

use std::ops::Range;

/// The byte offset of every line start, including 0 and the empty line after
/// a trailing newline.
pub(crate) fn line_starts(source: &str) -> Vec<u32> {
    let mut starts = Vec::with_capacity(source.len() / 24 + 1);
    starts.push(0);
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push((index + 1) as u32);
        }
    }
    starts
}

/// The line containing `byte` (0-based), given [`line_starts`].
pub(crate) fn line_of(starts: &[u32], byte: usize) -> usize {
    let byte = byte.min(u32::MAX as usize) as u32;
    match starts.binary_search(&byte) {
        Ok(line) => line,
        Err(next) => next.saturating_sub(1),
    }
}

/// The content range of a line, without its trailing `\n`.
pub(crate) fn line_range(source: &str, starts: &[u32], line: usize) -> Option<Range<usize>> {
    let start = *starts.get(line)? as usize;
    if start > source.len() {
        return None;
    }
    let end = match starts.get(line + 1) {
        Some(next) => (*next as usize).saturating_sub(1),
        None => source.len(),
    };
    Some(start..end.max(start))
}

/// Whether a line holds only spaces and tabs.
pub(crate) fn is_blank(text: &str) -> bool {
    text.bytes().all(|byte| byte == b' ' || byte == b'\t')
}

/// Sorted, merged source ranges that other passes must leave alone (code,
/// inline code, raw HTML, already-scanned formulas).
#[derive(Clone, Debug, Default)]
pub(crate) struct Protected {
    ranges: Vec<Range<usize>>,
}

impl Protected {
    /// Build from arbitrary ranges, sorting and merging them.
    pub(crate) fn new(mut ranges: Vec<Range<usize>>) -> Self {
        ranges.retain(|range| !range.is_empty());
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        Self { ranges: merged }
    }

    /// Add one range, merging overlaps. Ranges usually arrive in increasing
    /// order, so the common append path stays cheap.
    pub(crate) fn insert(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        if self.ranges.last().is_none_or(|last| range.start > last.end) {
            self.ranges.push(range);
            return;
        }
        let start = self.ranges.partition_point(|candidate| candidate.end < range.start);
        let end = self.ranges.partition_point(|candidate| candidate.start <= range.end);
        if start == end {
            self.ranges.insert(start, range);
            return;
        }
        let merged = self.ranges[start].start.min(range.start)..self.ranges[end - 1].end.max(range.end);
        self.ranges.splice(start..end, [merged]);
    }

    /// Whether `offset` is protected.
    pub(crate) fn contains(&self, offset: usize) -> bool {
        let index = match self.ranges.binary_search_by(|range| range.start.cmp(&offset)) {
            Ok(_) => return true,
            Err(0) => return false,
            Err(next) => next - 1,
        };
        self.ranges[index].end > offset
    }

    /// The protected range containing `offset`, if any.
    pub(crate) fn at(&self, offset: usize) -> Option<Range<usize>> {
        let index = match self.ranges.binary_search_by(|range| range.start.cmp(&offset)) {
            Ok(index) => index,
            Err(0) => return None,
            Err(next) => next - 1,
        };
        let range = &self.ranges[index];
        (range.end > offset).then(|| range.clone())
    }

    /// Whether any protected range intersects `range`.
    pub(crate) fn overlaps(&self, range: Range<usize>) -> bool {
        let index = self.ranges.partition_point(|candidate| candidate.end <= range.start);
        self.ranges.get(index).is_some_and(|candidate| candidate.start < range.end)
    }
}
