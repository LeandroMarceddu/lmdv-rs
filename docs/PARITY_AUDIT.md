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

**342 rows:** 205 done, 52 substitute, 56 partial, 11 missing, 18 n/a.

| Area | Rows | Done | Substitute | Partial | Missing | n/a |
|---|---:|---:|---:|---:|---:|---:|
| Reading and viewer | 30 | 22 | 3 | 4 | 1 | 0 |
| Tabs and windows | 35 | 19 | 8 | 6 | 2 | 0 |
| Editor | 40 | 26 | 2 | 9 | 2 | 1 |
| Markdown rendering | 34 | 29 | 3 | 2 | 0 | 0 |
| Full-window preview | 18 | 15 | 0 | 3 | 0 | 0 |
| Share, print and export | 16 | 7 | 1 | 7 | 1 | 0 |
| File browser and folder search | 22 | 19 | 2 | 1 | 0 | 0 |
| Outline, section line and history | 15 | 10 | 2 | 3 | 0 | 0 |
| Command palette | 13 | 10 | 2 | 1 | 0 | 0 |
| Links and media | 15 | 10 | 0 | 2 | 2 | 1 |
| System, CLI and file integration | 27 | 13 | 5 | 1 | 1 | 7 |
| Window chrome | 10 | 0 | 3 | 3 | 0 | 4 |
| Drag and drop | 8 | 2 | 2 | 2 | 2 | 0 |
| Help, start page, notices, rating | 13 | 7 | 0 | 5 | 0 | 1 |
| Accessibility | 15 | 11 | 0 | 4 | 0 | 0 |
| Internationalization | 11 | 4 | 3 | 3 | 0 | 1 |
| Persistence | 20 | 1 | 16 | 0 | 0 | 3 |
| **All** | **342** | **205** | **52** | **56** | **11** | **18** |

## P0 gaps (13)

Priority-0 rows that are not yet done. The full evidence and the precise missing
clause are in the area files.

### T — Tabs and windows (3)

- **T-01** (partial) — The strip now passes the measured `viewport_width`, so tabs shrink toward the kit's 28 pt minimum before scrolling (`view.rs`, `tab_list` tests; `--tree` proof). Still partial: the kit has no…
- **T-03** (partial) — Dirty shows the orange Attention dot; after a save the tab's leading slot becomes a check in the success colour for the existing 1.4 s flash (`window.rs`…
- **T-26** (partial) — `TabView` keeps anchor, find, reader find, nav, fragment, outline filter, parsed model, editor session, find bar, slash menu, details and dirty (`window.rs:78-140`);…

### E — Editor (2)

- **E-01** (partial) — Toolbar label/icon toggle and pane restore: `view.rs:451-460`, `editor_tests.rs::edit_mode_hides_the_reading_panes_and_restores_them`. Entering closes reader find and resets the section line…
- **E-07** (partial) — One-way sync, interpolation and manual-preview freedom pinned by `editor_tests.rs::a_source_wheel_scrolls_the_preview`, `::block_mode_maps_the_visible_row_to_its_source_line`,…

### X — Share, print and export (2)

- **X-02** (partial) — `share_tests::rich_copy_puts_markdown_and_html_on_the_clipboard` and the `lmdv-export/tests/rich_copy.rs` suite cover the inline-styled fragment, plain Markdown fallback, tasks, tables and links.…
- **X-05** (partial) — `share_tests::a_pdf_export_writes_an_a4_document_with_the_h1_break_from_prefs` (A4, one page per H1, flash, “Show in folder” action) and `::an_export_pins_its_document_until_the_job_ends`. OI-104…

### O — Outline, section line and history (1)

- **O-14** (partial) — Alt+←/→, mouse side buttons, tab menu and palette work: `crate::nav_tests::alt_left_and_right_dispatch_the_navigation_commands`, `the_mouse_side_buttons_navigate`,…

### C — Command palette (1)

- **C-01** (partial) — `palette_tests::ctrl_k_opens_and_closes_the_palette_while_reading`, `the_editor_keeps_ctrl_k_for_its_link_command`, `ctrl_shift_p_opens_it_anywhere`, `a_held_opening_key_does_not_close_it`,…

### L — Links and media (1)

- **L-11** (partial) — `LinkPlacement` + `WindowSpec::on_modifiers`: plain foreground, Ctrl background, Ctrl+Shift foreground, Shift new window, middle background (`links::`/`nav_tests` tests). Named gaps: plain opens a…

### D — Drag and drop (1)

- **D-04** (partial) — Tab drag exists through the kit and the T-22 transfer path, but see T-19: no reorder test, 4 pt slop, and the export pin refuses the drop rather than the gesture.

### H — Help, start page, notices, rating (1)

- **H-04** (partial) — `help_tests::the_full_scene_exposes_the_dialog_and_its_section_headings`, `::the_toolbar_help_opens_the_full_guide_and_closing_returns_focus`, `::every_locale_has_a_parsable_full_page`,…

### I — Internationalization (1)

- **I-10** (partial) — App dialogs/menus/notices and key caps are localized. rune-kit hardcodes “Resize sidebar”/“Resize inspector” and has no label override, so the pane grips stay English (OI-190).

## How this is maintained

1. Re-run the audit for the affected areas after a batch (read the spec rows, the
   implementation and the tests; do not mark `done` without evidence).
2. Update the area table; run `scripts/gen-parity-audit.py` and
   `scripts/gen-parity-checklist.py`. `--check` enforces the checklist in the gate.
3. Add a row to `OPEN_ITEMS.md` for a gap that needs a decision or a substitute
   acceptance; the audit keeps the per-behavior status.
