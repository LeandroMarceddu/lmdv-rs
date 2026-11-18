//! Rune-free Markdown parser and document model for lmdv.
//!
//! [`parse`] turns LF-normalized Markdown into a [`MdDoc`]: CommonMark plus
//! GFM tables, task lists and strikethrough, then the GitHub and app-specific
//! extensions — flat YAML front matter, GitHub alert callouts, footnotes,
//! heading anchors with legacy ids, `[TOC]`, the math scanner, the allowed
//! HTML subset and autolinks. The result is plain data: `Send`, `Sync`,
//! serializable and independent of Rune, so parsing can run on a worker
//! thread and exports need no UI.
//!
//! Sources of truth: `docs/ARCHITECTURE_DETAIL.md` §5 and the original
//! renderer under the upstream `src/`. Every extension names the
//! parity row it implements in its module documentation.

#![forbid(unsafe_code)]

mod blocks;
mod callout;
mod frontmatter;
mod heading;
mod html;
mod inline;
mod math;
mod model;
mod text;

pub use blocks::{parse, parse_with_revision};
pub use heading::{heading_slug, legacy_slug};
pub use model::*;

/// Run only the math pre-pass, for tests and diagnostics.
///
/// The returned text has the same byte length and line count as `source`;
/// each formula is represented by U+FFFC or, where a newline would be in the
/// way, a one-byte sentinel (`MathScan::placeholders` records where).
pub fn scan_math(source: &str) -> MathScan {
    let protected = blocks::protected_ranges(source);
    math::scan(source, &protected)
}

#[cfg(test)]
mod tests;
