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

**342 rows:** 262 done, 55 substitute, 7 partial, 0 missing, 18 n/a.

| Area | Rows | Done | Substitute | Partial | Missing | n/a |
|---|---:|---:|---:|---:|---:|---:|
| Reading and viewer | 30 | 27 | 3 | 0 | 0 | 0 |
| Tabs and windows | 35 | 25 | 8 | 2 | 0 | 0 |
| Editor | 40 | 36 | 2 | 1 | 0 | 1 |
| Markdown rendering | 34 | 31 | 3 | 0 | 0 | 0 |
| Full-window preview | 18 | 18 | 0 | 0 | 0 | 0 |
| Share, print and export | 16 | 14 | 2 | 0 | 0 | 0 |
| File browser and folder search | 22 | 20 | 2 | 0 | 0 | 0 |
| Outline, section line and history | 15 | 12 | 2 | 1 | 0 | 0 |
| Command palette | 13 | 11 | 2 | 0 | 0 | 0 |
| Links and media | 15 | 13 | 0 | 1 | 0 | 1 |
| System, CLI and file integration | 27 | 14 | 6 | 0 | 0 | 7 |
| Window chrome | 10 | 3 | 3 | 0 | 0 | 4 |
| Drag and drop | 8 | 5 | 2 | 1 | 0 | 0 |
| Help, start page, notices, rating | 13 | 12 | 0 | 0 | 0 | 1 |
| Accessibility | 15 | 14 | 1 | 0 | 0 | 0 |
| Internationalization | 11 | 6 | 3 | 1 | 0 | 1 |
| Persistence | 20 | 1 | 16 | 0 | 0 | 3 |
| **All** | **342** | **262** | **55** | **7** | **0** | **18** |

## P0 gaps (6)

Priority-0 rows that are not yet done. The full evidence and the precise missing
clause are in the area files.

### T — Tabs and windows (2)

- **T-01** (partial) — The strip now passes the measured `viewport_width`, so tabs shrink toward the kit's 28 pt minimum before scrolling (`view.rs`, `tab_list` tests; `--tree` proof). Still partial: the kit has no…
- **T-03** (partial) — Dirty shows the orange Attention dot; after a save the tab's leading slot becomes a check in the success colour for the existing 1.4 s flash (`window.rs`…

### O — Outline, section line and history (1)

- **O-14** (partial) — Alt+←/→ now dispatch from a window-root `on_key_preview` before a focused field can word-move; side buttons, tab menu and palette work with script evidence. **Gap:** pinned Rune still has no…

### L — Links and media (1)

- **L-11** (partial) — `LinkPlacement` + `WindowSpec::on_modifiers`: plain foreground, Ctrl background, Ctrl+Shift foreground, Shift new window, middle background (`links::`/`nav_tests` tests). Named gaps: plain opens a…

### D — Drag and drop (1)

- **D-04** (partial) — Tab drag exists through the kit and the T-22 transfer path, but see T-19: no reorder test, 4 pt slop, and the export pin refuses the drop rather than the gesture.

### I — Internationalization (1)

- **I-10** (partial) — App dialogs/menus/notices and key caps are localized. rune-kit hardcodes “Resize sidebar”/“Resize inspector” and has no label override, so the pane grips stay English (OI-190).

## How this is maintained

1. Re-run the audit for the affected areas after a batch (read the spec rows, the
   implementation and the tests; do not mark `done` without evidence).
2. Update the area table; run `scripts/gen-parity-audit.py` and
   `scripts/gen-parity-checklist.py`. `--check` enforces the checklist in the gate.
3. Add a row to `OPEN_ITEMS.md` for a gap that needs a decision or a substitute
   acceptance; the audit keeps the per-behavior status.
