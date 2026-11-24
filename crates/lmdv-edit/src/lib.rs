//! Source editor sessions and commands for lmdv.
//!
//! This crate is the pure editor model the app binds to Rune's
//! [`TextEdit`](rune::TextEdit): no window, no tab, no file I/O. It owns
//!
//! - [`EditorSession`]: the canonical LF buffer plus selection, undo history,
//!   line ending, find state, syntax cache and the optional block editor for
//!   large documents (ADR-016);
//! - [`block::BlockEditor`]: one `TextEdit` per blank-line block with a global
//!   undo journal that spans blocks;
//! - [`find`](mod@find): literal, whole-word and regex search and replacement
//!   over the LF text, using the shared [`lmdv_core::FindState`] value type;
//! - [`format`](mod@format): the formatting bar, line commands and block
//!   inserts from the original's `src/editCommands.ts` (E-22–E-31);
//! - [`syntax`](mod@syntax): Markdown source highlighting from `rune-kit`'s
//!   highlighter, mapped to [`rune::Span`]s;
//! - [`spell`](mod@spell): Hunspell dictionary discovery, checking and the
//!   underline spans that compose with the syntax spans (ADR-008, E-SPELL).
//!
//! Canonical text is always LF. A file's line ending lives beside the buffer
//! and is applied only by [`EditorSession::save_text`], which also never adds
//! a byte-order mark (E-09).

#![forbid(unsafe_code)]

pub mod block;
pub mod find;
pub mod format;
pub mod session;
pub mod spell;
pub mod syntax;

pub use block::{
    BLOCK_MODE_MAX_BYTES, BLOCK_MODE_MAX_LINES, BlockEditKind, BlockEditor, EditBlock, JOURNAL_MAX_BYTES,
    JOURNAL_MAX_ENTRIES, block_ranges, should_use_block_mode,
};
pub use find::{FindOptions, find, find_error};
pub use format::{
    BlockKind, LinePrefix, delete_line, duplicate_line, goto_line, goto_target_offset, insert_block, insert_link,
    move_line_down, move_line_up, select_line, select_next_occurrence, set_heading, toggle_comment, toggle_line_prefix,
    toggle_wrap,
};
pub use session::{EditorSession, EditorSnapshot};
pub use spell::{
    APP_DICTIONARY_DIR, Availability, DictionaryFiles, SUGGESTION_LIMIT, Spell, SpellError, compose_underlines,
    dictionary_roots, discover, requested_tags,
};
pub use syntax::{SYNTAX_MAX_BYTES, SyntaxCache, syntax_spans};
