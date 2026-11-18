//! GitHub heading slugs, the pre-1.15.1 legacy slug and reserved app ids.
//!
//! Ported from the original app's `src/renderer.ts:29-54` and the reserved-id
//! rules at `:500-561`.

use std::collections::{HashMap, HashSet};

/// GitHub's heading id (`github-slugger`): lowercase, keep Unicode letters,
/// marks, numbers and connector punctuation plus spaces and hyphens, turn
/// spaces into hyphens, drop the rest. Falls back to the legacy slug when
/// nothing is left.
pub fn heading_slug(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    for ch in lower.chars() {
        if ch == ' ' || ch == '-' {
            out.push('-');
        } else if is_kept(ch) {
            out.push(ch);
        }
    }
    if out.is_empty() { legacy_slug(title) } else { out }
}

/// `markdown-it-anchor`'s default slug, used by the original app up to 1.15.0:
/// trim, lowercase, whitespace runs to `-`, then `encodeURIComponent`.
pub fn legacy_slug(title: &str) -> String {
    let trimmed = title.trim().to_lowercase();
    let mut collapsed = String::with_capacity(trimmed.len());
    let mut in_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !in_space {
                collapsed.push('-');
                in_space = true;
            }
        } else {
            collapsed.push(ch);
            in_space = false;
        }
    }
    encode_uri_component(&collapsed)
}

/// Assign unique heading ids and legacy ids in document order.
#[derive(Debug, Default)]
pub(crate) struct Slugger {
    github: UniqueIds,
    legacy: UniqueIds,
}

impl Slugger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The deduplicated id and, when it differs, the deduplicated legacy id.
    pub(crate) fn slug(&mut self, title: &str) -> (String, Option<String>) {
        let id = self.github.insert(heading_slug(title));
        let legacy = self.legacy.insert(legacy_slug(title));
        let legacy_id = (!legacy.is_empty() && legacy != id).then_some(legacy);
        (id, legacy_id)
    }
}

/// One id space (GitHub or legacy) that deduplicates a base slug.
///
/// The next suffix per base is remembered, so a run of duplicates costs one
/// attempt per heading instead of restarting the counter at 1 (OI-144). A
/// `base-N` already taken by another heading is still skipped: the counter
/// only moves forward.
#[derive(Debug, Default)]
struct UniqueIds {
    taken: HashSet<String>,
    next_suffix: HashMap<String, usize>,
}

impl UniqueIds {
    fn insert(&mut self, base: String) -> String {
        if self.taken.insert(base.clone()) {
            return base;
        }
        let mut counter = self.next_suffix.get(&base).copied().unwrap_or(1);
        loop {
            let candidate = format!("{base}-{counter}");
            counter += 1;
            if self.taken.insert(candidate.clone()) {
                self.next_suffix.insert(base, counter);
                return candidate;
            }
        }
    }
}

/// The ids of the app's own UI, which a document heading may not take over
/// (`renderer.ts:513-561`).
pub(crate) fn is_reserved_id(id: &str) -> bool {
    const FIXED: &[&str] = &[
        "toolbar",
        "links",
        "title",
        "name",
        "timeline",
        "format-bar",
        "editor-host",
        "print-page-meta",
        "palette",
        "palette-input",
        "palette-list",
        "palette-help",
        "palette-status",
    ];
    if FIXED.contains(&id) {
        return true;
    }
    numeric_suffix(id, "mmd-")
        || numeric_suffix(id, "dmmd-")
        || numeric_suffix(id, "immd-")
        || numeric_suffix(id, "tree-group-")
        || numeric_suffix(id, "palette-opt-")
        || numeric_suffix(id, "palette-g-")
}

/// `user-content-<id>` for a reserved id, with the written name kept aside.
pub(crate) fn rename_reserved(id: String) -> (String, Option<String>) {
    if is_reserved_id(&id) {
        let renamed = format!("user-content-{id}");
        (renamed, Some(id))
    } else {
        (id, None)
    }
}

fn numeric_suffix(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
}

/// `char::is_alphabetic` covers `\p{L}`; combining marks are not alphabetic,
/// so include their common blocks explicitly.
fn is_kept(ch: char) -> bool {
    ch.is_alphabetic()
        || ch.is_numeric()
        || matches!(ch,
            '\u{0300}'..='\u{036f}'
            | '\u{1ab0}'..='\u{1aff}'
            | '\u{1dc0}'..='\u{1dff}'
            | '\u{20d0}'..='\u{20ff}'
            | '\u{fe20}'..='\u{fe2f}'
        )
        || matches!(
            ch,
            '_' | '\u{203f}' | '\u{2040}' | '\u{2054}' | '\u{fe33}' | '\u{fe34}' | '\u{fe4d}'..='\u{fe4f}' | '\u{ff3f}'
        )
}

/// JS `encodeURIComponent` leaves ASCII alphanumerics and `-_.!~*'()` alone.
fn encode_uri_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')')
        {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}
