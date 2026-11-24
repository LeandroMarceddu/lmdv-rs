# Testing and verification

The app is visual, stateful and full of edge cases. “It compiled” is not evidence. Every behavior that matters gets a test, a golden, a structural assertion or a documented manual check.

## 1. Test layers

| Layer | What | Where |
|---|---|---|
| Unit | IDs, `SourceBuffer`/line index, EOL/BOM, prefs clamps, path policy, front matter, slugging, math scanner, table model, find, line ops | each crate's `src/*_tests.rs` / `tests/` |
| Parser golden | `MdDoc` serialized (or compact projection) for every fixture: blocks, inline spans, source map, headings, diagnostics | `crates/lmdv-md/tests/golden` |
| Render golden | Rune element tree JSON + PNG at 1×/2× for scenes; pixel compare with tolerance | `crates/lmdv-testkit`, `tests/golden` |
| A11y | roles, names, levels, focus order, live regions, selection | `--tree` + `Runtime::accessibility_json` |
| Interaction | open, edit, save, find, palette, tabs, close prompts, routing, drag/drop | `tests/scripts` |
| Export | HTML exact/normalized, DOCX unzip+XML, PDF text/pages, PNG dimensions/pixels | `crates/lmdv-export/tests` |
| Security | XSS/HTML payloads, path traversal, image bombs, socket fuzz, huge tables/docs | `tests/security` |
| Perf | parse, typing, commit, scroll, export, launch | `benches/`, `--frames`, `--profile perf` |
| Manual | Wayland/X11, NVIDIA/Mesa, scaling, IME, multi-window, MIME, Orca | checklist below |

## 2. Fixture corpus

Adopt the ten families from `BEHAVIOR_SPEC.md` §9 under `tests/fixtures/`:

| Family | Contents | Parity IDs |
|---|---|---|
| F-CORE | reading, headings, outline, section line, long/short docs | R, O, M-03–M-05, M-20 |
| F-MARKDOWN | math, HTML, tasks, front matter, alerts, footnotes, code, mermaid, links, security | M, L |
| F-TABLE | prose/numeric/wide/nested/colgroup/task/print tables | M-06, V-11–V-18, X-12, F-05 |
| F-EDIT | edit, CRLF, BOM, draft, image, conflict, long edit, broken render | E, M-34, X-02 |
| F-TABS | six tabs, pasted, two windows, 15-tab overflow, 480 px | T, W, D |
| F-TREE | hidden/build dirs, depth cap, 2001 files, docs/README/index, search terms | F |
| F-SYSTEM | CLI, delete/rename, touch-only, network path, six languages, prefs, a11y | S, R-20–24, A, I, P |
| F-EXPORT | headings, code, table, callout, task, footnote, math, mermaid, image, links, TOC, front matter | X |
| F-PALETTE | command-like headings, tabs, 30 files, 12 recents, editor commands | C, A-08 |
| F-ERRORS | missing, unreadable, conflict, preview throw, bad Mermaid, clipboard, language reload, Save again | error rows |

Plus the `test-features.md`, `commonmark-math.md`, `nested/` and `img/` fixtures.

Rules:

- Every fixture maps to parity IDs; a fixture without an ID is not a gate.
- Generated fixtures use a committed seed/script.
- Fixtures are deterministic: no network, no clock, fixed locale/timezone.
- A new bug adds a fixture or a script before the fix.

## 3. Headless scenes

Define `AppSpec::scenes`/`lmdv-testkit` scenes for deterministic screenshots:

```text
viewer-light, viewer-sepia, viewer-dark, viewer-black
viewer-callout, viewer-math, viewer-table, viewer-frontmatter
viewer-toc, viewer-footnotes, viewer-html, viewer-mermaid
editor-split, editor-find, editor-formatbar, editor-block, editor-large
palette, share, help, keys, lightbox, table-preview
dirty-close, save-conflict, draft-restore, missing-file, render-error
start-page, multi-window, narrow-480
```

Implemented today: `reader`, `reader-outline`, `editor`, `editor-block`, `start`, `tabs`, `help`; the rest are planned.

`editor-block` generates a 3605-line, 1202-block document and enters edit mode, so the virtualized source pane shows block fields (`editor-block-0`…) and the `Block mode` label. To script block edits on a real file, use any document above 64 KiB / 1500 lines with `tests/scripts/block-edit.json`: it types in block 0, taps `editor-block-1`, types there, saves, then undoes twice.

`tests/scripts/p4-shared-close.json`, `p4-quit-prompt.json` and `p4-close-prompt.json` exercise P4-02a’s shared-view close, aggregate quit and last-view close on a two-window headless run; `tests/fixtures/session-two-windows.json` is a generic two-start-page session for `LMDV_TEST_RESTORE`.

`--scene lightbox` opens the image preview; `tests/scripts/lightbox.json` opens the image and diagram, zooms, pans and closes. `--scene table-preview` opens the filtered/sorted table grid; `tests/scripts/table-preview.json` clears the filter, closes, uses the bar and reopens, then copies. `tests/scripts/nav-history.json` (Alt+←/→) and `nav-buttons.json` (mouse side buttons) run at 1280×900 across `test-features.md` and `nested/deeper.md`. `--scene reader-sepia` / `reader-black` show the full palettes; `--scene editor-size` shows the 0.875× editor; `--scene editor-menus` opens the format bar's Insert menu; `--scene notice-stack` shows a warning over the reader find bar; `--scene tab-notice` proves a per-tab sticky notice; `--scene version` shows the same version on the start page and in Help. `--scene settings` opens preferences; `tests/scripts/settings.json` changes theme/font/size/format bar and switches to Italian. `--scene palette` opens the command palette; `tests/scripts/palette*.json` cover a query, `>`/`#` modes and outline. `--scene reader-progress` / `--scene reader-section` need `--frames 40` (the restore and 120 ms settle); `tests/scripts/reader-section.json` clicks a section segment. `--scene reader-find` / `--scene reader-line` show the viewer find bar and the line band; `tests/scripts/reader-find.json` types `Akeneo`, steps and asserts the table hit. `p6-pdf-wide.pdf` is the wide-table bbox fixture.

`LMDV_BROWSER_ROOT=<dir> --scene browser` renders the file browser; `tests/scripts/browser-search.json` focuses the folder search, types `PIM`, picks a result and opens it at its line.

`LMDV_TEST_RESTORE=1` with a scratch `LMDV_HOME` lets a headless run restore a hand-written `session.json` (debug builds only; never set by the app) so `--list-windows`/`--window` can assert restored windows.

`--scene tab-menu` / `--scene tab-window-menu` open the tab move menus; `tests/scripts/tab-move.json` moves the active tab through the chooser in a two-window headless run (`LMDV_TEST_WINDOW`).

`--scene share` opens the Share menu (share-item0…5, checkbox); `tests/scripts/share-export.json` exports HTML to `$LMDV_EXPORT_DIR` when `AppShared::is_headless()` is true, bypassing only the save dialog. The same env var is the only headless export seam; it is ignored in a normal session.

`tests/scripts/scroll-sync.json` and `scroll-single.json` drive the one-way E-07 sync: block mode wheels the source then the preview, single-field wheels the editor; runtime tests assert offsets because `--tree` has rects but no scroll positions.

`tests/scripts/preview-double.json` and `preview-menu.json` exercise E-35/E-04 on `test-features.md`: the first enters edit mode and double-clicks a word, the second right-clicks block `b2` and picks “Edit from here”.

`tests/scripts/task-tick.json` runs against `tests/fixtures/test-features.md --line 31`: it taps the “Installer QA” checkbox, then the notice's Undo action. The key `task-hit-b8-2-335575d7` is the block/item/label overlay key; the file must read `- [x]` after the tick and `- [ ]` after Undo, and the tree gains/loses `notice-action`.

Scene functions set `WindowState` prefs and insert fixture documents synchronously through the testkit; they do not run the async open path. Use `--size 1280x900 --scale 2 --theme light` for goldens, and 480×900 for narrow scenes.

## 4. Golden policy

- **Parser goldens:** serialize `MdDoc` (or a compact projection) and compare exactly. These catch source-map/block/inline regressions without GPU noise.
- **Render goldens:** PNG at 1× and 2×; compare with a perceptual tolerance (SSIM ≥ 0.995 or MAE ≤ 2/255). Text AA may differ across GPUs; structure must not.
- **Tree goldens:** `--tree` JSON for keys/roles/labels/focus/selection. This is the primary a11y gate.
- A golden update needs a written reason (“intended visual change because …”).
- Never widen a tolerance globally to hide a regression; use a per-image exception with a comment.
- `RUNE_BLESS`-style regeneration is allowed only with a review of the diff.

## 5. Structural and accessibility assertions

For each main surface:

- Every heading has `role=heading`, correct `level`, non-empty name.
- The tab strip is a `tablist`; each tab is a `tab` with `selected`; close button has a name.
- The outline is a `tree`/`treeitem` with levels and expanded state.
- The file browser is a `tree`, directories expanded/collapsed, keyboard navigation.
- Find bar: input name, live count, next/prev names.
- Notices are `status`/`alert` live regions; same-message repeat is announced.
- Modal dialogs are `dialog` with focus trapped; Esc closes; focus returns.
- The article is `article`/`document`; links have names; images have alt; math has the TeX label.
- Task checkboxes expose checked/mixed and the item text (capped).
- Table preview: region instructions, live filter count, `aria-sort`.
- Reduced motion is honored; focus is visible.

Record tree JSON for the main surfaces in `tests/trees/`. A tree change that is not intentional fails review.

## 6. Interaction scripts

Cover at least:

- Open file → tab; title/reading time; second open activates.
- Link → in-app / new tab / new window / external; Back/Forward.
- Find → n/N; Esc; wide table/code match scrolls into view.
- Ctrl+E → type → dirty dot → Ctrl+S → saved check.
- Close dirty tab → Save / Don't save / Cancel; window close names all.
- Tab reorder, tear-out, drop on another window (X11), Wayland fallback.
- Paste Markdown → unsaved tab named after heading; Save as.
- Task checkbox click → file write + Undo.
- File drop, drop on strip, image paste/drop.
- Command palette `>`/`#`, Ctrl+Enter, Shift+Enter.
- Math delimiter cases from `commonmark-math.md`.
- `--line N`, `--new-window`, second-instance routing.
- Draft restore and pasted-draft auto-restore.
- Watcher conflict/reload/rename/delete.

Scripts live in `tests/scripts/`, named by parity ID where possible. Use `--trace` for motion regressions on animated surfaces.

## 7. Export tests

- **HTML:** parse/compare normalized DOM; assert no external URLs; images/fonts/math/diagrams embedded; tables/code/quotes semantic; anchors correct.
- **DOCX:** unzip and assert `word/document.xml` contains headings/tables/images; open in LibreOffice headless if available; equations either OMML or images as documented.
- **PDF:** page count, page size, header/date, page numbers, H1 page breaks, no clipped wide table; `pdftotext` if available; visual page snapshots.
- **PNG:** dimensions, 2×/4×, transparency/white background policy.
- **Rich copy:** clipboard text+HTML; table TSV; plain Markdown.
- **Reader independence:** set 22 px serif + full width + black theme, export, assert standard light 16 px layout.

## 8. Security tests

- HTML: `<script>`, `<style>`, `<iframe>`, `<object>`, `<embed>`, `on*`, `javascript:`, `data:text/html`, popover/command attributes, tagfiltered tags.
- Paths: `../` escape, symlink escape, `/etc/passwd`, `/proc`, `/dev`, FIFOs, `//host/share`, `\\?\`, device paths.
- Images: huge dimensions, decompression bomb, SVG external refs/scripts.
- Socket: oversized message, bad JSON, wrong uid, stale socket, path traversal in launch.
- Huge doc/table: caps and notices; no freeze.
- Watcher: rename chain, atomic save, own-write echo, deleted/restored.
- Export: no network, no script in HTML, no path escape in assets.

## 9. Performance budgets

p95, not median; record outliers. Target machine: CachyOS, NVIDIA/Wayland; also Mesa/X11.

| Metric | Budget |
|---|---|
| Launch to first frame (warm release) | < 350 ms |
| Open 1 MB file to readable | < 250 ms |
| Parse 500 KB | < 80 ms |
| Typing commit, 64 KB single field | < 16 ms |
| Typing commit, 1 MB block mode | < 16 ms |
| Scroll frame, 2k blocks | < 8 ms CPU |
| Idle | zero frames, zero timers |
| HTML export 1 MB | < 1 s |
| PDF 50 pages | < 5 s |
| PNG diagram 2× | < 1.5 s |

Use Rune's `frame-profile` and `long_note`-style benches; a regression opens a perf task with the trace/fixture. Do not “fix” a budget by removing a feature.

## 10. Manual smoke matrix

| Axis | Values |
|---|---|
| Session | Wayland (Plasma/Hyprland/sway), X11 |
| GPU | NVIDIA proprietary, Mesa |
| Scale | 100%, 150%, 200% |
| Theme | light, sepia, dark, black, system |
| Width | 480, 760, 1024, 1600, 4K |
| Input | US, Italian, German, French, Spanish, Chinese IME |
| Files | CRLF, BOM, no final newline, missing, read-only, deleted, renamed, 1 MB |
| Windows | 1, 3, same doc in two, tear-out, close last |
| CLI | `lmdv file.md`, `--line`, `--new-window`, second launch, missing file |
| Export | HTML in browser, DOCX in LibreOffice/Word, PDF in viewer, print |
| A11y | Orca: headings, outline, tabs, find, notices, dialog focus |
| Desktop | Dolphin/Nautilus/Thunar double-click, MIME default, install/uninstall |

## 11. Commands

**Test ownership and machine load.** Workers run only the targeted tests for the code they touch: `cargo test -p <crate> <filter>`, a specific integration test binary, or a scene/script. A worker must not run `cargo test --workspace`, `scripts/run-gates.sh`, or a whole-crate suite. The orchestrator runs the full workspace gate, because those runs freeze the user's machine for several minutes. If a worker needs a broad regression signal, it reports the exact command and leaves it to the orchestrator.

The gate is build-I/O heavy, not test-I/O heavy: linking one debug test binary writes hundreds of megabytes. `Cargo.toml` sets `debug = 1` for the dev and test profiles, which keeps useful backtraces without the full-debug 1.3 GB binaries. `RUST_TEST_THREADS` only limits test execution and does not reduce the build write burst.

The vendor patch proof tests are separate and build `vendor/rune/target`. Run them after a vendor bump or patch edit, or set `LMDV_RUNE_PATCH_TESTS=1` for a gate that includes them.

**CI** (`.github/workflows/ci.yml`) runs the full gate on `ubuntu-latest` for pushes to `main` and pull requests, with the native winit/wgpu/fontconfig packages installed. `windows-latest` runs only `cargo check --workspace --all-targets`; Windows is compile-checked, never expected to pass Linux-only scripts or GUI tests.

```bash
# full gate — orchestrator only
scripts/run-gates.sh

# vendor patch proof — after a vendor bump or patch edit
scripts/test-rune-patches.sh

# visual / structural
cargo run -p lmdv -- --snap /tmp/opencode/snap.png --scene viewer-dark --size 1280x900 --scale 2
cargo run -p lmdv -- --tree /tmp/opencode/tree.json
cargo run -p lmdv -- --script tests/scripts/reading.json --frames 60 --out /tmp/opencode/frames

# performance
cargo build --profile perf -p lmdv
target/perf/lmdv --snap /tmp/opencode/perf.png --scene viewer-large
```

GUI-only tests require `RUNE_GUI_TESTS=1` and a desktop session; headless tests are the default gate.

**Headless determinism exception:** `--snap`, `--tree` and `--script` parse the document inline and wait for image decoding (`Image::wait`) so a one-frame gate shows the finished article. Interactive windows always parse/decode on workers and show a placeholder until ready. This is intentional; do not “fix” the headless path to use asynchronous jobs without updating the golden methodology.

## 12. Failure policy

- A flaky test is a bug: record seed/command in `docs/STATUS.md`, quarantine only with a tracked task.
- A visual diff is either intended (update golden with a reason) or a regression (fix).
- A performance regression is a bug even if the feature works.
- An a11y regression is a bug even if the app “looks fine”.
- Do not delete a test to make the suite green. Fix it, `#[ignore]` with a task, or remove it with the user's approval.
- A P0 row without evidence is not done.

## 11. Typing benchmark (SP-03)

`cargo run --release -p lmdv-edit --example typing_bench` prints per-keystroke p50/p95/p99 for single-field, block mode, syntax spans, undo/redo and the single-block worst case. It makes no timing assertions; record the numbers with the hardware in `docs/STATUS.md` when the threshold or journal changes.

## 12. Parser security corpus

`cargo test -p lmdv-md --test security` runs 175 curated pathological documents plus a fixed-seed 2,000-case generator (no panic, span/source-map invariants, 5 s per-case budget). Add a case when a parser bug or new syntax feature lands.
