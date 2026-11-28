# Changelog

Notable changes, newest first. Detailed notes live in `docs/releases/` where a
release has them. "Unreleased" is on `main` but not yet tagged.

## [Unreleased] — 2026-11-28

### Added

- Multi-select `Ctrl+O` via upstream `Tx::pick_files`: first file opens in
  front, the rest as background tabs in selection order (OI-002/T-32).
- Rune 230-commit wave through `af6b1fdd`, then `07bf8c5d3`, bringing automatic
  rest-time cache eviction, glibc `malloc_trim`, fractional inline-image
  metrics/rise/labels, a11y label substitution, inline-image highlight boxes
  and static Mermaid SVG export.
- SVG vector shapes for block and inline `<svg>`; entity-aware source maps,
  `details open`, and single inline `<picture>` objects.
- Tab/tree ARIA (`aria-controls`, `current`, `owns`, `position_in_set`), IME
  palette guard, Alt+arrow navigation, inline-picture hover help and verified
  native pinch.
- Bounded 256 KiB block journal with exact-undo barrier, open-fence canonical
  rescan, stable nested task keys, and a task save queue with Undo for
  buffer-only ticks.
- Session schema gains find/filter/pane state, frame clamping, restore flag,
  activation tokens, tear-out follow and dirty-state wake-ups.

### Changed

- All four local Rune patches retired: `vendor/rune/patches/` is empty and
  lmdv uses `El::inline_image` / `MathImage` instead of `Span::image`.
- `scripts/test-rune-patches.sh` now runs 330 upstream feature tests.
- Block menu gained a rich-copy row; block wire no longer clones drawn text
  per frame.

### Fixed

- Large-document RSS returns to the simple-file baseline after rest (Wayland
  ~594 → ~225 MB, X11 ~625 → ~258 MB); OI-199 closed.
- Queued task-conflict test made deterministic; Windows-only `Launch` test
  literal given defaults for the new fields.

### Added

- Linux CSD via Rune's `App::titlebar` (ADR-028), focusable localized pane
  grips, lone-tab window drag, browser-key navigation, and a real external-open
  confirmation dialog. Upstream #2744/#2745 APIs replaced the remaining
  substitutes: clipboard write acknowledgement, built-in field-menu spelling
  suggestions and multi-caret editing.

### Fixed

- C-12 palette ARIA, OI-001/OI-049 startup/image probe jobs, OI-024 parser
  find, OI-051 modal confirmation, OI-061 syntax cache, OI-102 row measurement,
  OI-108 clipboard ack, OI-174 field menu, OI-176 locale test seam, OI-179 pane
  grips and the flaky task-conflict test.

### Accepted substitutes

- Sub/Sup no longer shifts the baseline vertically (font size only), OI-200.
- OI-044's document-level proxy scrollbar and OI-054's incremental-scroll
  budget remain partial; OI-155 Windows runtime checks are manual.

## [0.11.0] — 2026-11-25

Rune bump to `f4ec14e`, upstream close veto, Linux/GPU input improvements, and
the P1/P2 parity batch. Signed `SHA256SUMS` with the Ed25519 release key.
[Full notes](docs/releases/v0.11.0.md).

## [0.10.0] — 2026-11-24

Editor and preview parity, Find in document, selection rich copy, outline
cursor, per-tab notices, recents cleanup, project site and signed checksums.
[Full notes](docs/releases/v0.10.0.md).

## [0.9.0] — 2026-11-23

Editor enter/sync fade, local-date PDF headers, file reveal and print cleanup,
image links, encoded paths, outline/jump reserves and table overflow fixes.
[Full notes](docs/releases/v0.9.0.md).

## [0.8.0] — 2026-11-21

Per-user default Markdown handler on Linux and Windows; article reuse and
wheel-glide tuning raised a 1200×860 scroll trace from 20 to 41 FPS.
[Full notes](docs/releases/v0.8.0.md).

## [0.7.0] — 2026-11-21

Full start page and notice kinds, in-page anchors, link policies and actions,
localized key overlay, and copy buttons for every code block.
[Full notes](docs/releases/v0.7.0.md).

## [0.6.0] — 2026-11-20

Wiki-style links, Ctrl/middle-click link placement, external Markdown drag and
drop onto the window or tab strip. [Full notes](docs/releases/v0.6.0.md).

## [0.5.5] — 2026-11-18

One document per file with symlink/hard-link identity and Save As collision
handling, Linux spellcheck engine, and accessible math labels.
[Full notes](docs/releases/v0.5.5.md).

## [0.5.0] — 2026-11-18

First native Rune-based Markdown viewer/editor release: Linux AppImage and raw
binary, Windows NSIS installer, unsigned checksums.
