# Parity audit

Status of every behavior in [`BEHAVIOR_SPEC.md`](BEHAVIOR_SPEC.md), checked against the
checkout on 2026-11-20. The detailed tables and evidence live in [`audit/`](audit/):

- [`audit/reading-outline-preview.md`](audit/reading-outline-preview.md) — R, O, V (63 rows)
- [`audit/tabs-windows-browser-palette.md`](audit/tabs-windows-browser-palette.md) — T, W, D, F, C (88)
- [`audit/editor-markdown-links.md`](audit/editor-markdown-links.md) — E, M, L (89)
- [`audit/export-system-a11y-i18n-help-persistence.md`](audit/export-system-a11y-i18n-help-persistence.md) — X, S, A, I, H, P (102)

**Statuses:** `done` meets the acceptance criterion with a named test/scene/fixture;
`substitute` is an accepted different mechanism (linked from `FEATURE_PARITY` or an OI);
`partial` is implemented with a named clause missing; `missing` has no implementation;
`n/a` is platform-specific and not applicable on Linux.

`PARITY_CHECKLIST.md` is generated from these tables, so the checkbox list always
matches this audit.

## Totals

**342 rows:** 173 done, 52 substitute, 86 partial, 13 missing, 18 n/a.

| Area | Rows | Done | Substitute | Partial | Missing | n/a |
|---|---:|---:|---:|---:|---:|---:|
| Reading and viewer | 30 | 18 | 3 | 8 | 1 | 0 |
| Tabs and windows | 35 | 18 | 8 | 7 | 2 | 0 |
| Editor | 40 | 20 | 2 | 15 | 2 | 1 |
| Markdown rendering | 34 | 25 | 3 | 6 | 0 | 0 |
| Full-window preview | 18 | 15 | 0 | 3 | 0 | 0 |
| Share, print and export | 16 | 7 | 1 | 7 | 1 | 0 |
| File browser and folder search | 22 | 19 | 2 | 1 | 0 | 0 |
| Outline, section line and history | 15 | 9 | 2 | 4 | 0 | 0 |
| Command palette | 13 | 10 | 2 | 1 | 0 | 0 |
| Links and media | 15 | 4 | 0 | 8 | 2 | 1 |
| System, CLI and file integration | 27 | 11 | 5 | 3 | 1 | 7 |
| Window chrome | 10 | 0 | 3 | 3 | 0 | 4 |
| Drag and drop | 8 | 2 | 2 | 2 | 2 | 0 |
| Help, start page, notices, rating | 13 | 2 | 0 | 8 | 2 | 1 |
| Accessibility | 15 | 9 | 0 | 6 | 0 | 0 |
| Internationalization | 11 | 4 | 3 | 3 | 0 | 1 |
| Persistence | 20 | 0 | 16 | 1 | 0 | 3 |
| **All** | **342** | **173** | **52** | **86** | **13** | **18** |

## P0 gaps (41)

Priority-0 rows that are not yet done. The full evidence and the precise missing
clause are in the area files.

### R — Reading and viewer (4)

- **R-02** (partial) — Defaults exist: `READER_MAX_WIDTH=760`, `READER_PAD_X=48`, `READER_PAD_BOTTOM=96`, 16 pt/1.7 (`crates/lmdv-app/src/reader.rs:33-39`, `appearance.rs:522`), padding steps `20 pt @ ≤640`, `14/24 pt @…
- **R-20** (partial) — `file.reload` is bound to `Ctrl+R`/`F5` (`commands.rs:717-722`); reload keeps the anchor (`crate::editor_tests::a_clean_document_reloads_on_a_watch_change_and_keeps_its_anchor`). **Gap:** on a…
- **R-21** (partial) — `crate::editor_tests::a_clean_document_reloads_on_a_watch_change_and_keeps_its_anchor`; the watcher ignores the app's own `Access` events (`crates/lmdv-io/src/watch.rs`, OI-168). **Gap:** every…
- **R-23** (partial) — Rename follows file/recents/positions/history/dirty buffer: `crate::editor_tests::a_rename_follows_the_document_registry_recents_and_positions`, `a_rename_keeps_an_unsaved_buffer_dirty`;…

### T — Tabs and windows (4)

- **T-01** (partial) — The strip now passes the measured `viewport_width`, so tabs shrink toward the kit's 28 pt minimum before scrolling (`view.rs`, `tab_list` tests; `--tree` proof). Still partial: the kit has no…
- **T-03** (partial) — Dirty shows the orange Attention dot; after a save the tab's leading slot becomes a check in the success colour for the existing 1.4 s flash (`window.rs`…
- **T-12** (partial) — Tree and search hits honor Ctrl/Ctrl+Shift/Shift (`browser.rs:744-790`; `browser_tests::the_menus_open_rows_use_the_background_tab_and_new_window_paths`). Recents open on a plain press only…
- **T-26** (partial) — `TabView` keeps anchor, find, reader find, nav, fragment, outline filter, parsed model, editor session, find bar, slash menu, details and dirty (`window.rs:78-140`);…

### E — Editor (8)

- **E-01** (partial) — Toolbar label/icon toggle and pane restore: `view.rs:451-460`, `editor_tests.rs::edit_mode_hides_the_reading_panes_and_restores_them`. Entering closes reader find and resets the section line…
- **E-05** (partial) — `editor_tests.rs::typing_parses_the_preview_after_the_debounce_and_keeps_the_last_good_model_meanwhile`, `p2_tests.rs::a_parse_result_is_cached_latest_wins_per_document`; `PREVIEW_DEBOUNCE = 0.12`…
- **E-07** (partial) — One-way sync, interpolation and manual-preview freedom pinned by `editor_tests.rs::a_source_wheel_scrolls_the_preview`, `::block_mode_maps_the_visible_row_to_its_source_line`,…
- **E-13** (partial) — Guard exists (`editor_tests.rs::leaving_edit_mode_can_save_first_and_returns_to_the_viewer`), but the dialog offers **Save / Don’t save** with Esc as cancel, not the specified **Discard / Cancel**…
- **E-15** (partial) — One document per file and dirty-buffer preservation: `window.rs::a_symlink_open_merges_into_the_one_document_and_keeps_its_edits`,…
- **E-17** (partial) — `editor_tests.rs::a_crash_draft_is_offered_and_restore_enters_edit_mode_without_writing_the_file`, `::a_draft_equal_to_the_file_is_dropped_silently`,…
- **E-22** (partial) — Bar and pressed state: `edit_ui_tests.rs::format_commands_apply_the_selection_and_report_pressed_state`, `::the_editor_keymap_formats_and_does_not_toggle_the_reading_panes`; `view.rs:1160-1210`.…
- **E-25** (partial) — `lmdv-edit/src/format.rs::headings_set_replace_and_remove`, `::headings_cover_selected_lines_and_keep_indentation`, `::heading_levels_above_six_clamp`. Setext headings are not recognized by…

### M — Markdown rendering (4)

- **M-13** (partial) — Language labels and highlighting: `lmdv-md/src/tests.rs::code_blocks_keep_language_and_text`, `features_renders...code`. Copy works only when the document has exactly one top-level code block…
- **M-24** (partial) — Images: `images.rs::a_local_picture_resolves_and_decodes`, `::a_source_fragment_is_not_part_of_the_path`, `::resolution_is_cached_per_source` (512-entry cap). Query strings are not dropped…
- **M-30** (partial) — Refusal works: `links.rs::destinations_classify_without_touching_the_disk` (`//host/share/a.md` refused) and `images.rs::urls_data_urls_and_device_paths_never_touch_the_filesystem` (missing…
- **M-34** (partial) — Last-good preview retained: `editor_tests.rs::typing_parses_the_preview_after_the_debounce...`, `p2_tests.rs::an_edit_requests_a_new_parse_and_keeps_the_last_good_model_meanwhile`; a bad formula…

### X — Share, print and export (2)

- **X-02** (partial) — `share_tests::rich_copy_puts_markdown_and_html_on_the_clipboard` and the `lmdv-export/tests/rich_copy.rs` suite cover the inline-styled fragment, plain Markdown fallback, tasks, tables and links.…
- **X-05** (partial) — `share_tests::a_pdf_export_writes_an_a4_document_with_the_h1_break_from_prefs` (A4, one page per H1, flash, “Show in folder” action) and `::an_export_pins_its_document_until_the_job_ends`. OI-104…

### O — Outline, section line and history (2)

- **O-03** (partial) — Activation line and last-viewport slide: `crate::reader::tests::the_activation_line_slides_through_the_last_viewport`,…
- **O-14** (partial) — Alt+←/→, mouse side buttons, tab menu and palette work: `crate::nav_tests::alt_left_and_right_dispatch_the_navigation_commands`, `the_mouse_side_buttons_navigate`,…

### C — Command palette (1)

- **C-01** (partial) — `palette_tests::ctrl_k_opens_and_closes_the_palette_while_reading`, `the_editor_keeps_ctrl_k_for_its_link_command`, `ctrl_shift_p_opens_it_anywhere`, `a_held_opening_key_does_not_close_it`,…

### L — Links and media (7)

- **L-01** (partial) — `p2_tests.rs::a_fragment_link_scrolls_to_its_heading`, `nav_tests.rs::a_same_document_anchor_link_jumps_and_records_the_place_left`, `::a_jump_that_lands_within_eight_points_records_nothing`,…
- **L-02** (partial) — `links.rs::destinations_classify_without_touching_the_disk` accepts https/mailto and rejects script/file schemes. `tel:` is not accepted (`links.rs:120`), and unsupported schemes produce a notice…
- **L-07** (partial) — A non-folder link produces the “not a Markdown file” notice (asserted in `links.rs::a_relative_markdown_link_resolves_inside_the_document_folder`), but there is no **Open** whitelist, no **Show in…
- **L-08** (partial) — `open_linked_document` opens a tab for a missing Markdown target, so the user gets an error tab rather than “Not found: …”; recents/tree/search are not refreshed. `msg.notFound` is only used by…
- **L-09** (partial) — A pasted document refuses with `link.needsFile` (`links.rs:219`); in-page anchors still work. No **Save as…** action, and the specified `msg.pastedLink` string is unused.
- **L-10** (partial) — Network paths are refused before lookup (`links.rs:129-131`), satisfying the main rule. The notice differs from the specified text, and the same-share exception is absent (a UNC document is…
- **L-11** (partial) — `LinkPlacement` + `WindowSpec::on_modifiers`: plain foreground, Ctrl background, Ctrl+Shift foreground, Shift new window, middle background (`links::`/`nav_tests` tests). Named gaps: plain opens a…

### S — System, CLI and file integration (2)

- **S-04** (partial) — `window.rs::a_forwarded_launch_opens_a_tab_with_its_line` and `windows.rs::a_launch_for_a_new_file_goes_to_the_idle_window_and_clears_its_claim` cover one forwarded launch and the shared inbox. No…
- **S-12** (partial) — `app.rs:20-22` uses 1100×760 logical and min 560×420. The spec is 1200×860 and min 480×480. No test asserts either number. Native frame/decorations and the “visible after first render” rule are…

### D — Drag and drop (1)

- **D-04** (partial) — Tab drag exists through the kit and the T-22 transfer path, but see T-19: no reorder test, 4 pt slop, and the export pin refuses the drop rather than the gesture.

### H — Help, start page, notices, rating (3)

- **H-01** (partial) — `view.rs:714-745` draws the wordmark/tagline, **Open a file…**, recents and version in a scrolling page. It does not draw “No document open”, the lead line, **Paste from clipboard**, the three…
- **H-04** (partial) — `help_tests::the_full_scene_exposes_the_dialog_and_its_section_headings`, `::the_toolbar_help_opens_the_full_guide_and_closing_returns_focus`, `::every_locale_has_a_parsable_full_page`,…
- **H-06** (partial) — One notice at a time with optional action and dismiss (`view.rs:2042-2072`), exercised by save/share/task tests. `Notice` is only `{text, deadline, action}`: there is no warning/error/info/success…

### A — Accessibility (1)

- **A-03** (partial) — Toolbar/menu controls pass `.label`/`.shortcut` to rune-kit's `IconButton`, which feeds accesskit `keyboard_shortcut` (`vendor/rune/crates/rune-kit/src/actions.rs:215-224,398`) and strips the…

### I — Internationalization (1)

- **I-10** (partial) — Six catalogs, keyed dialogs/menus/notices (settings, share, tasks, table notices) and localized confirmation buttons are covered by the settings/share/help suites. rune-kit's pane grips still say…

### P — Persistence (1)

- **P-05** (partial) — `recents.json` max 12/dedupe (`store.rs::recents_are_capped_and_deduplicated`), written on open and rename (`editor_tests::a_rename_follows_the_document_registry_recents_and_positions`). There is…

## How this is maintained

1. Re-run the audit for the affected areas after a batch (read the spec rows, the
   implementation and the tests; do not mark `done` without evidence).
2. Update the area table; run `scripts/gen-parity-audit.py` and
   `scripts/gen-parity-checklist.py`. `--check` enforces the checklist in the gate.
3. Add a row to `OPEN_ITEMS.md` for a gap that needs a decision or a substitute
   acceptance; the audit keeps the per-behavior status.
