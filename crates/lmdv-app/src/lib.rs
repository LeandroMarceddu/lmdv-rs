//! The lmdv application shell.
//!
//! P2 scope: one Rune window over the shared document registry, a tab strip, a
//! start page with recents, a command registry, generation-guarded load jobs,
//! and the real reading surface: `lmdv-md` parses on a worker, `lmdv-render`
//! draws the article from the cached `Arc<MdDoc>`, `lmdv-math` supplies formula
//! images and `rune::ImageCache` the document's pictures. Outline, `--line` and
//! reading positions all address the model's source maps.
//!
//! Architecture rules this crate keeps:
//!
//! * no filesystem I/O in handlers or views — loading, saving, probing parses
//!   and store flushes are `jobs` posted back through a per-window
//!   [`WorkerMailbox`]; picture paths go through `lmdv-io`'s path policy and
//!   `rune::ImageCache` decodes on its own threads;
//! * every disk operation goes through `lmdv-io` in handlers and views;
//!   `export_io` is the one worker-only exception, for reading a policy-checked
//!   asset and writing an export target (never mounted in a frame);
//! * no `unwrap` in UI paths — a document that disappeared after a close race
//!   is a normal `None`;
//! * views stay pure: the reader view reads state and `Cx`, and reports its
//!   scroll offset through a `Cell` for `poll` to observe;
//! * one [`Document`](lmdv_core::Document) per file per process; at most one tab
//!   per document per window.
//!
//! The binary calls [`run`] with the parsed [`lmdv_io::Launch`].

#![forbid(unsafe_code)]

mod actions;
#[cfg(test)]
mod actions_tests;
mod app;
mod appearance;
mod block_ui;
#[cfg(test)]
mod block_ui_tests;
mod browser;
#[cfg(test)]
mod browser_tests;
mod commands;
mod edit_ui;
#[cfg(test)]
mod edit_ui_tests;
mod editor;
#[cfg(test)]
mod editor_tests;
mod export_io;
mod fuzzy;
mod help;
#[cfg(test)]
mod help_tests;
mod icons;
mod images;
mod indent;
mod jobs;
mod lightbox;
#[cfg(test)]
mod lightbox_tests;
mod links;
mod math;
mod nav;
#[cfg(test)]
mod nav_tests;
mod outline;
#[cfg(test)]
mod p2_tests;
mod palette;
#[cfg(test)]
mod palette_tests;
mod palette_ui;
mod preview_nav;
#[cfg(test)]
mod preview_nav_tests;
mod reader;
mod reader_find;
#[cfg(test)]
mod reader_find_tests;
mod reading;
mod reading_keys;
#[cfg(test)]
mod reading_keys_tests;
mod reading_meta;
mod section;
#[cfg(test)]
mod section_tests;
mod session;
mod settings;
#[cfg(test)]
mod settings_tests;
mod settings_ui;
mod share;
#[cfg(test)]
mod share_tests;
mod shared;
mod tab_card;
#[cfg(test)]
mod tab_card_tests;
mod tab_keys;
#[cfg(test)]
mod tab_keys_tests;
mod tab_list;
mod table_ui;
mod tables;
#[cfg(test)]
mod tables_tests;
mod tasks;
#[cfg(test)]
mod tasks_tests;
mod tear_out;
#[cfg(test)]
mod tear_out_tests;
#[cfg(test)]
mod test_support;
mod transfer;
#[cfg(test)]
mod transfer_tests;
mod view;
mod window;
mod windows;
mod word_boundary;

pub use actions::{ClosedTab, MAX_CLOSED_TABS, TabActionsState, pasted_text, pasted_title};
pub use app::{SCENES, Scene, run, run_with_dirs};
pub use appearance::{Appearance, AppearanceKey};
pub use browser::{
    BrowserMenu, BrowserMenuAction, BrowserMenuEntry, BrowserRow, BrowserState, SEARCH_DEBOUNCE, SearchRow,
    browser_menu_entries, collapse_key, markdown_link, relative_path, search_prefill,
};
pub use commands::{
    COMMANDS, CommandCategory, CommandSpec, command_for_key, dispatch_command, key_matches, lookup_command,
};
pub use edit_ui::{FindBar, GotoLine, SlashItem, SlashMenu};
pub use images::{ImageCacheState, WindowImages};
pub use jobs::{GenerationTracker, JobHub, JobMsg, WorkerMailbox};
pub use lightbox::{Lightbox, LightboxMode, scene_lightbox};
pub use links::{LinkTarget, is_markdown};
pub use math::{MathCache, WindowMath};
pub use nav::{JUMP_INSET, MIN_JUMP_DISTANCE, PASTED_PREFIX};
pub use preview_nav::{BlockMenu, locate_word};
pub use reader::{ReaderView, ScrollSignal, ScrollSnapshot, ViewportAnchor};
pub use reader_find::{ReaderFind, find_in_document, reader_find_prefill};
pub use reading::LineFlash;
pub use reading_meta::{ReadingStats, reading_stats};
pub use section::{SectionLineFrame, SectionLineState, SectionSegment, progress_width};
pub use settings::{
    COLUMN_WIDTHS, LARGE_FILE_MODES, LINE_HEIGHTS, READER_FONTS, SIDE_SIZES, SettingsState, THEMES, apply_locale,
    column_max_width, language_options, locale_index, pane_kit, pane_scale, theme_label,
};
pub use settings_ui::scene_settings;
pub use share::{
    ExportFonts, ExportFormat, ExportOutcome, ShareItem, ShareState, export_base, file_safe_name, medium_date,
    pdf_fonts,
};
pub use shared::{AppShared, ParsedDoc, UiMailbox, UiMsg, unix_millis};
pub use tab_card::{TAB_CARD_DELAY, TAB_CARD_KEY, TabCardContent, TabCardState, card_content};
pub use tab_list::{STRIP_OVERFLOW_SLACK, TAB_LIST_BUTTON_KEY, TAB_LIST_MENU_KEY, TabListState, strip_overflows};
pub use table_ui::scene_table_preview;
pub use tables::{
    TablePreview, TableState, TableTarget, export_table, fold, highlight_ranges, minimum_widths, plan_widths,
};
pub use transfer::{IncomingHover, Outgoing, ReturnReason, TabMenu, TransferMsg, TransferToken, WindowTarget};
pub use view::{
    scene_browser, scene_browser_menu, scene_editor, scene_editor_block, scene_editor_size, scene_help,
    scene_help_full, scene_reader, scene_reader_black, scene_reader_find, scene_reader_line, scene_reader_outline,
    scene_reader_progress, scene_reader_section, scene_reader_sepia, scene_share, scene_start, scene_tab_card,
    scene_tab_menu, scene_tab_menu_file, scene_tab_overflow, scene_tab_window_menu, scene_tabs,
};
pub use window::{
    CloseOutcome, HelpView, Mode, Notice, Tab, TabSource, TabView, UiState, WindowState, parse_tab_key, tab_key,
    title_for_path,
};
pub use windows::{
    OpenedWindow, RouteAction, RouteDecision, RouteQuery, RouteReason, WakeSet, WindowEntry, WindowHandles, WindowKind,
    WindowOpenRequest, WindowOpener, WindowRegistry, WindowSnapshot, open_runtime_window, window_key,
};
