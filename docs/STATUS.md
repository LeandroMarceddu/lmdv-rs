# Status

**Updated:** 2026-11-25
**Phase:** P6 command/tab, reading polish, both i18n sweeps, tab quick wins, P6-SAFE batch 4, Linux spellcheck, the math a11y patch, P6 batch 5, the lightbox/tab-menu/overflow batches and the P5 export extras are complete. The 342-row parity audit is current (263 done, 54 substitutes, 7 partial, 0 missing, 18 n/a), the app-side P0s and the broad P1/P2 batch are implemented, the diagram/AVIF export path and bounded Windows runtime are in, the default Markdown handler covers Windows and Linux, and v0.11.0 is released. Remaining: 6 P0 partials (kit patches or small Rune patches), the OI-061/OI-102 shared-design residuals, reproducible builds/platform signatures and the held/blocked items.
**Implementation:** P0, P1, P2 parser/render/math, P3 editor core + app edit mode + editor UX + block mode, and P5 HTML/DOCX/PDF export are complete and committed.

## P0 — complete

- Git repo and baseline commits.
- Full pinned Rune vendored at `vendor/rune/` (`07bf8c5d3`, 2026-11-28, Rust 1.99.1); root repeats the eight `[patch.crates-io]` entries (including `naga`, `sctk-adwaita` and `accesskit_atspi_common`).
- Rune `07bf8c5d3` provides fractional inline-image metrics, `rise`, `label`, a11y label substitution, inline-image highlight boxes and Mermaid SVG (upstream #2743). All four local patches are retired; `vendor/rune/patches/` is empty.
- Close veto is upstream: `App::on_close_request -> CloseResponse` runs before hiding; programmatic closes stay force-close. `crates/lmdv-app/src/app.rs` returns `Defer` while a document is dirty.
- `lmdv-core`: IDs, `SourceBuffer`, `Document`, `DocumentRegistry`, edit lease, prefs, nav/draft/session types; 53 tests pass.
- `scripts/run-gates.sh` is green: parity checklist, vendor check, formatting (own crates only), workspace check/test/clippy. Vendor patch proof tests are separate (`scripts/test-rune-patches.sh`, after a vendor bump or patch edit) so the gate does not rebuild `vendor/rune/target` every time.
- User-level handler scripts tested end-to-end in a temporary HOME: install, MIME default, wrapper launching the real binary, uninstall restore.
- Proof snapshots: `/tmp/opencode/lmdv-p0.png`, `/tmp/opencode/lmdv-p0-math.png`.

## P1 — complete

- `lmdv-io` complete: CLI parser, path policy, atomic/read/write, XDG store, watcher, single-instance socket; 68 tests pass.
- `lmdv-app` shell complete: `AppShared`, `WindowState`, mailboxes/jobs, command registry, tabs/start page, open-file plumbing; 72 tests pass. Binary wired to `lmdv_app::run`.
- Real launcher path verified: install in a temp HOME, run the wrapper with `tests/fixtures/test-features.md --snap ...`; the file opens in a tab and the source preview renders.
- P1-03 viewer complete: outline, per-tab positions, `--line`, reader scenes. The temporary `rune-kit` reader was replaced in P2.
- Single-instance routing wired: a second `lmdv file.md --line N` forwards through the Unix socket and opens a tab in the running window; headless tooling skips the claim. Socket-level and handler-level tests pass.

## P2 reader pipeline — complete

- The app now parses with `lmdv-md` on a worker and renders with `lmdv-render`; the temporary `rune-kit` reader and direct `pulldown-cmark` are gone.
- Math: RaTeX formulas render inline/display with cached `InlineImage`s and TeX fallback.
- Images: `ImageCache` + `PathPolicy`, missing/broken placeholders; Mermaid; tables/sticky headers; callouts/front matter/footnotes/TOC; links; details cards.
- `--line`, heading-id positions, outline from `MdDoc`, parse cache, 114 app tests.
- **P2 gate:** reader/render/math fixtures green. Follow-ups (`find`, source-range highlights, click-to-source, goldens, perf, task ticks, full theme palettes) are in `docs/OPEN_ITEMS.md` and scheduled P3/P6/P7.

## P3-01 — complete

- `lmdv-edit`: `EditorSession`, single-field and fence-aware block mode, global block undo journal, literal/whole-word/regex find and replace, formatting/line/comment/link commands, syntax spans with the 128 KiB window, drafts, EOL/BOM save policy. 90 tests pass.

## P3-02 — complete

- `Mode::Editor` on `TabView`; `Ctrl+E` acquires/releases the document lease; foreign lease → dialog.
- Split view (source left, live preview right, draggable divider; below 760 pt source-only with a preview toggle). Preview parses 120 ms after the last key and keeps the last good model.
- `Ctrl+S` via `save_document` with `expected_meta`, session EOL, `create=false`; the saved hash is the text actually written, so typing during a save stays dirty. Save As / Save again, conflict (Overwrite / Keep theirs), deleted (Save again / Cancel), error notices.
- Drafts: 1 s debounce, flush on tab switch/close/quit, Restore/Discard offered, equal-to-disk drafts dropped.
- Watcher: dedicated thread, sequence-tagged events, 1.5 s own-write suppression, clean reload with anchor, dirty prompt, rename rekey, delete + Save again.
- Close guards for tab/window/native close; the window dialog names every dirty document; `Ctrl+Q` uses the same guard.
- Scene `editor` and 31 integration tests; app total 146 tests. Independent scripted check: `Ctrl+E` → type → `Ctrl+S` wrote the file and left no draft. Evidence: `/tmp/opencode/p3-editor-check.png`, `/tmp/opencode/p3-editor-tree-check.json`.

## P5-01 — complete

- `lmdv-export`: `ExportDoc` from `MdDoc` + `AssetResolver`, then one self-contained HTML file: semantic markup, inline print CSS, data-URI images, 2× PNG math with TeX in `alt`/`aria-label`, GFM tables with alignment, callouts, footnotes with backlinks, TOC, front matter opt-in, code token spans.
- Link/media policy: relative links become `file:///` with an absolute `source_path`; `javascript:`/`vbscript:`/`data:`/explicit `file:` links are refused; network images never fetched.
- Deterministic output, no `<script>`, no file I/O in the crate. 11 unit + 20 integration + 1 doctest pass; clippy `-D warnings` clean.
- Evidence: `/tmp/opencode/lmdv-p5-export-test-features.html`, `/tmp/opencode/lmdv-p5-export-commonmark-math.html`.

## P5-02 — complete

- `lmdv-export::export_docx` writes a deterministic OOXML package from `ExportDoc`: real `Heading1`–`Heading6` styles, `Title`/`Quote`/`ListParagraph`/`CodeBlock`, 9-level numbering with start overrides, tasks as ☑/☐, GFM tables with repeated shaded header rows, callouts, code with preserved whitespace, hyperlinks (external + internal anchors), images/diagrams as media relationships, math as 2× PNG with TeX in `descr`/`title`, TOC as a linked list, plain footnote list, A4/1-inch/Calibri 11 pt — independent of reader prefs.
- Hand-written stored-ZIP container with fixed 1980 timestamps and stable order → byte-identical output; no production zip dependency (`zip`/`roxmltree` are dev-deps for tests).
- 26 unit + 18 DOCX + 20 HTML + 1 doctest pass; clippy `-D warnings` and fmt clean. Evidence: `/tmp/opencode/lmdv-p5-02-test-features.docx` (17 parts, 8 images), `…-commonmark-math.docx` (35 parts, 26 images); `unzip -t`, `python-docx` (43 paragraphs/8 tables/8 shapes), style/rel/numId cross-checks clean. LibreOffice absent, so its smoke test skipped.
- Gaps: no OMML, no `footnotes.xml`, TOC is not a field, SVG/AVIF not embedded, uncompressed package (OI-066/076–080).

## P3-03 — complete

- Find/replace bar: `Ctrl+F`/`Ctrl+H`, query/replace fields, case/whole-word/regex toggles, match count, Enter/Shift+Enter/F3/Shift+F3, Replace/Replace All in one undo group each, regex error text, Esc returns to the source.
- Format bar: bold/italic/strike/code/link, paragraph/H1–H3, bullet/ordered/task/quote, table/code/math/Mermaid/note/tip/warning/divider; app-side pressed state; editor keymap (`Ctrl+B/I`, `Ctrl+Shift+X/M`, `Ctrl+K`, `Ctrl+1–3`, `Ctrl+/`, line commands, `Alt+↑/↓`).
- Slash menu with filtering/↑/↓/Enter/Esc; go-to-line (`Ctrl+G`) with validation; editor-keys help section; 37 `edit.*` commands enabled only in edit mode.
- 18 new integration tests; app total 164 tests. Scene `editor` now shows the find bar (`1 / 2`). Independent scripted `/table` + Enter + `Ctrl+S` check passed; evidence `/tmp/opencode/p3-03-check.png`, `/tmp/opencode/p3-03-tree-check.json`.
- Gaps remain: no paint-only highlights (OI-043/081), inline (not caret-anchored) slash panel and two undo steps (OI-082), app-side format state/Setext (OI-061), English go-to-line strings (OI-083), no viewer find (OI-084).

## P5-03 — complete

- `lmdv-export::export_pdf` via Typst 0.15.1 (`typst`/`typst-pdf`/`typst-layout`), in-memory `World`, no Rune or external printer process.
- A4 20/16/18/16 mm, header = title · localized date, footer `page / pages`, tagged PDF, embedded subset fonts, h1–h6 outline, tables with repeated headers, callouts, lists/tasks, code, images, math PNG with TeX `/Alt`, footnotes, TOC, optional `break_on_h1`.
- Deterministic output (no dates/IDs); 15 PDF tests; export total 80 tests. Evidence: `/tmp/opencode/lmdv-p5-03-test-features.pdf` (3 pages), `…-commonmark-math.pdf` (4 pages); `pdfinfo` A4/tagged, `pdftotext` text, `qpdf --check` clean, `pdfimages` images present.
- Gaps at the time: default serif font (fixed in P5-04a), X-12 wide-table fitting (fixed in P6-PDF-T), raw HTML flattening (OI-087), page-split/U+200B/PDF-UA details (OI-088/134).

## P3-04a — complete

- Block mode is now bound in the app: `LargeFileMode::Block` forces it, `Auto` enables it above 64 KiB / 1500 lines, chosen once when the edit session starts.
- The source pane is a virtualized `editor-blocks` column (`El::virtual_rows`, only mounted fields build); each field has its own syntax spans and key `editor-block-{i}`. Scene `editor-block` shows `Block mode` / `1202 blocks` with 15 mounted fields for a 3605-line document.
- Typing commits the active block into the journal/shared document; save, drafts and the 120 ms preview see committed text. `Ctrl+Z`/`Ctrl+Y` walk the global journal across blocks. Enter twice splits, Backspace/Delete at a block edge merge.
- E-33 parity fixed: Tab/Shift+Tab indent/outdent selected lines in both modes (2 spaces, one undo step), Tab never moves focus, and Esc then Tab (within 2 s) leaves the source for the preview toggle. New `indent.rs` with 7 unit tests.
- 200 app tests pass (186 + 14); clippy/fmt clean. Independent scripted checks: Tab indents in small and block mode without changing focus; click-based two-block edit + save + undo; `editor-block` snap/tree. Evidence `/tmp/opencode/p3-04a-check.png`, `/tmp/opencode/p3-04a-tree-check.json`.
- Gaps: trailing-blank-line/paste boundary normalization until reload (OI-089), structural-undo caret (OI-090), SP-03 benchmark (OI-064), scroll sync (OI-075).

## P3-04b — complete

- Live task checkboxes in viewer mode (M-07): a per-window `TaskWire` only for viewer tabs; clicking the box flips exactly one source byte, writes through a save job with `expected_meta`/EOL (the one allowed viewer write), updates the saved base, clears drafts and reparses. Text clicks and every edit-mode click stay inert.
- Undo: localized “Task checked/unchecked” notice with an **Undo** button for 8 s; a second tick replaces the pending undo.
- Conflicts restore the optimistic tick before prompting Overwrite / Keep theirs; Deleted/errors leave no unsaved viewer write. Dirty/leased/pasted documents keep the tick in the buffer and let the owner save.
- 21 new tests; app total 221. Independent scripted check on `test-features.md` line 31: tap → `- [x]`, Undo → `- [ ]`, tree shows/clears `notice-action`. Evidence `/tmp/opencode/p3-04b/p3-04b-task.png`.
- Renderer gaps: `TaskWire` wraps the row, so the app overlays a transparent box-sized hit area; `TaskRef` lacks a source range (OI-091/092). English fallbacks and serialized saves: OI-093/094.

## P3-04c1 — complete

- Renderer `RenderWires::block` / `BlockRef` / `PreviewText` (additive, default `None`): every top-level block exposes its key, source lines and drawn text segments; trees are unchanged when the wire is unset.
- E-35: while editing, double-clicking preview text uses Rune's native word selection as the hit signal, maps the word back to the block's source slice (whole → any case → inside a longer word, with preceding-text occurrence disambiguation), and focuses/selects in the single field or the right `editor-block-{i}`. Links, task boxes, images, diagrams and summaries keep their own clicks; viewer double-click still only selects (R-27).
- E-04: right-click a preview block opens “Edit from here” (plus Cancel); it enters edit mode at the block's 0-based line or moves the caret. Esc/outside close, Shift+F10 opens it from a focused block.
- 23 new app tests + 1 renderer test; app 244, renderer 33. Evidence `/tmp/opencode/p3-04c1/`; scripts `tests/scripts/preview-double.json`, `preview-menu.json`.
- Gaps: padding double-click, keyless repeated text, preview-height placement, keyboard shortcut, Copy menu (OI-095–098).

## P3-04c2 — complete

- One-way editor→preview scroll sync (E-07): `on_scroll_offset` on `editor-scroll` records the source top; `poll_scroll_sync` maps it to a fractional source line and interpolates between preview block anchors, with a 16 pt margin and sticky-table header cover.
- Block mode uses the visible row's measured height and source-line span; single-field uses line height. Caret moves and parse completions reveal the whole block when it fits, else the heading, else the estimated line; 300 ms reveal window and 200 ms edit-entry settling.
- Manual preview scrolling is never read back: the next source scroll moves the preview again. No feedback loop.
- 14 new tests (pure mapping, block mapping, reveal, clamps, header cover, scripted wheels, entry settling); app total 258. Scripts `tests/scripts/scroll-sync.json`, `scroll-single.json`. Evidence `/tmp/opencode/p3-04c2/`.
- Approximations: wrapped-line mapping, within-block scaling, header cover scope, front matter (OI-100/101); per-row view rerun (OI-102).

## P5-04a — complete

- Share menu (X-01): toolbar button + `Ctrl+Shift+E`, six items in order, first enabled focused, Esc/outside/arrow behavior; checkbox persists `Prefs::print_break_h1`; disabled without a document; whole-document rich copy was enabled in P5-04b.
- Export job: one background job per request builds `ExportDoc` once; HTML/DOCX/PDF written atomically through `lmdv-io` after `PathPolicy` asset checks. Save dialogs with localized filters and safe default names; success flash/notice, PDF Show in folder, Print writes a temp PDF and opens the system viewer.
- PDF fonts: Noto Sans + DejaVu Sans Mono loaded via `fontdb` and cached; fallback to Typst serif recorded. Export title now = first h1, else source stem (X-14).
- `lmdv-io` gained `PathPolicy::read_asset`, `atomic::canonical_target` and `open_external`, so `lmdv-app` has no `std::fs` outside tests.
- 17 new share tests (app 276), io 70; clippy/fmt/gate green. Evidence `/tmp/opencode/p5-04a-share.png`, scripted HTML `/tmp/opencode/p5-04a-check-export2/test-features.html` (70 KB, no script, 6 data images, lang en, title = h1).
- Gaps: selection copy/TSV/Mermaid raster (P5-04b/106), export freshness during debounce, UTC date, temp cleanup, tab moves (OI-103–107).

## P5-04b — complete

- `lmdv-export::export_rich_html` writes one Word-friendly clipboard fragment with inline styles only: light palette, 760 px column, headings, inline marks, lists/tasks (☑/☐), tables, code as shaded single-cell tables with token colours, localized callouts with emoji, TOC/footnotes, images/math as data URIs, absolute links; no `<style>`, classes, scripts, inputs or app ids. Heading/footnote ids remain as bookmarks.
- Share › Copy as rich text is enabled (`Ctrl+Shift+C`): a background job builds the rich HTML from the parsed model and returns the current Markdown as the plain form; `tx.copy_rich` writes both; success flashes the Share check with a localized notice, failure shows an error. No filesystem access in handlers.
- 18 rich-copy export tests (export total 104) and 3 app tests with Rune's memory clipboard (app 279); clippy/fmt clean. Evidence `/tmp/opencode/p5-04b/`.
- Gaps: math is PNG not MathML (OI-066), Mermaid is source (OI-106), selection copy/table TSV (OI-096/110), clipboard ack (OI-108), dedicated notice key (OI-109), stale parsed model during debounce (OI-103).

## P3 polish — complete

- OI-089: a commit that creates or removes a blank-line boundary re-partitions the edited field ± one neighbour immediately, text+layout in one journal entry; covers separator typing, multi-paragraph paste, deleted separators, empty fields, EOF and closed fences.
- OI-090: structural journal entries restore the caret and owning field after split/merge undo/redo; the app focuses the restored block.
- OI-091/092: `TaskControlWire` wraps the real checkbox (overlay removed; press + Space/Enter work), and `TaskRef::source` locates the marker directly from the inline source map.
- 99 edit + 35 renderer + 285 app tests; clippy/fmt/gate green. Evidence `/tmp/opencode/p3-wrap/`.
- Residual: unclosed fences spanning >1 neighbour may normalize on a later commit (OI-111); identical nested task labels can share a tree key (OI-112).

## P4 research — complete (read-only)

- Verdict: pinned Rune supports runtime document windows via `rune::open_window(WindowSpec<S>)` from a handler/poll on the UI thread; `App::window` is the wrong API (secondary windows hide and do not keep the app alive).
- Recorded capsule: `docs/ARCHITECTURE_DETAIL.md` §7.6 — exact APIs/citations, traps (UI-thread, unique keys, headless inertness, `AppHandle::update` is `Send`, last-window `on_quit` skipped, Wayland placement/always-on-top limits, one title API), P4-01/P4-02 slices, file-level changes, test strategy and open product decisions.
- No code changed. Next worker must be given that capsule verbatim.

## P4-01 — complete

- Runtime document windows via `rune::open_window` (`WindowSpec::from_app(...).state(prebuilt)`), not `App::window`; opener is UI-thread only and builds the state before opening. Primary and runtime windows share the same `AppSpec` hooks.
- `windows.rs`: `WindowEntry`/`WindowHandles`/`WindowSnapshot`, `WakeSet`, pure routing matrix (`--new-window` → doc holder → claim → idle normal → focused → any → new), claims cleared on publish, focus stamps, placement cache/cascade.
- `shared.rs`: registry moved out, per-window wakers, routing/focus/close requests; `app.rs` uses `rune::run_windows` and adds the `LMDV_TEST_WINDOW` headless second window; `window.rs` routes launches once, implements `FocusWindow`/`OpenLaunch`, `Ctrl+Shift+N`, closing flag.
- 312 app tests; headless `--list-windows` (`window-1`, `second`), `--window second --tree`, `--desktop` two-window snapshot; manual X11 check: forwarded `--new-window` created a second window and closing the first kept the process alive.
- Remaining P4-02: tab transfer/tear-out, aggregate close/quit, session restore, activation tokens/Wayland focus, dirty repaint broadcast (OI-074/113–118).

## P4-02a — complete

- Close rules now use live subscribers: a dirty document prompts only when this window holds its last view; a shared view closes silently and keeps the other window's buffer, lease and draft. Pasted tabs are named on window close per T-24.
- `closing` is published while a prompt is open and cleared on Cancel; a second close/quit while a prompt is open focuses the asking window or refuses a second prompt.
- `Ctrl+Q` aggregates dirty/pasted documents across all windows, deduped by `DocumentId`, saves each once in sequence (including pasted Save As), then quits; Cancel aborts, failed saves abort. The last window flushes the store from `Drop` (Rune skips its `on_quit`).
- 13 new tests; app total 325. Headless evidence: silent shared-view close, T-24 prompt, aggregate quit naming `test-features.md` once; `/tmp/opencode/p4-02a/`.
- Product choice recorded: closing a window with a dirty doc open elsewhere is silent (recommended in §7.6).
- Remaining after P4-02a: tab transfer (P4-02b), session restore (P4-02d) and tear-out (P4-02c) are done; activation tokens/Wayland focus (OI-117) and dirty repaint (OI-074) remain.

## P4-02b — complete

- In-process transfer protocol (`Outgoing`, `TransferMsg::{Hover,Adopt,Adopted,Returned}`) through the same-thread `UiMailbox`; no `Send`/`AppHandle::update` and no 15 s webview timeout (vanished target restores locally).
- Source detaches the tab but keeps subscribers/lease until adoption; target applies T-22 rules (clean duplicate merges, dirty duplicate refused and returned), inserts with a fresh tab id, subscribes/watches and takes the lease; source releases on `Adopted` or reinserts on `Returned` with the full `TabView` (anchor, find, nav, editor session).
- Tab context menu + chooser: Move to a new window, Move tab to window…, Close tab; `WindowKind`/closing/enable predicates; scenes `tab-menu`/`tab-window-menu`.
- 20 new tests; app total 345. Headless evidence: menu images, chooser tree, desktop frames showing the tab move between two windows; `/tmp/opencode/p4-02b/`.
- Remaining: X11 tear-out is P4-02c (done); Close others/right + full tab menu (OI-119), loading/start-page transfer (OI-120), i18n strings (OI-118), dirty repaint (OI-074).

## P4-02d — complete

- Session schema is additive (`WindowSession.maximized/kind`, `TabSession.mode`) and stores only reopenable state: windows, tabs (file/pasted/start), active index, anchors, pending line, mode, placement/kind. Editor text is never stored; a test asserts it cannot appear.
- Windows publish a session fragment after poll; the store write is debounced 500 ms. A window close removes its entry; the last close or quit writes `clean: true`. Startup writes `clean: false`; a crash restores with the recovery notice.
- Restore runs in the `run_windows` factory only when `prefs.restore_session`, not headless, no CLI path, no `--new-window`; keys `main`, `window-2`, …; X11 frames restored, Wayland position omitted but maximized applied; pasted tabs only with a draft (missing ones dropped with a notice); file tabs use the normal open path without rewriting recents; editor mode is applied after ready.
- 22 session + 5 factory tests; app 372, io 72; clippy/fmt/gate clean. Headless seam `LMDV_TEST_RESTORE=1` (debug-only) validates `--list-windows`/`--window` restore; evidence `/tmp/opencode/p4-02d/`.
- Residuals: nav/find/pane state not stored, Wayland maximized/scale/clamp, stale-draft mode, duplicate restore loads, optional `--restore-session` flag (OI-121–124).

## P4-02c — complete

- X11 only: the tab strip gets `TearOutSpec` when the platform reports screen points; Wayland/headless keep `tear_out(None)` so a drag never detaches and the tab menu remains the substitute.
- Release-only placement: on release, hit-test other windows' cached placements → `move_tab_to_window_at` (target resolves the hovered slot), else open a new window at `tear_out_position` and adopt. `on_tear_move` posts `Hover` for the target strip's incoming gap.
- Position formula handles scale/decoration/display clamp; `build_layered` floats the ghost over the 28 pt strip. Manual X11/XWayland check found and fixed a start-page adopt `index` panic; regression tests added.
- 22 tear-out/transfer tests; app total 394. Evidence `/tmp/opencode/p4-02c/`; headless tabs snap byte-identical, tree unchanged.
- Substitutes recorded: release-only (no live follow/reattach), Wayland menu, T-21 lone-tab window move (no Rune API), KWin/XWayland repositioning prevented on-screen position validation.
- Remaining P4 polish: activation tokens/Wayland focus + pin (OI-117), dirty repaint broadcast (OI-074), full tab menu/close others (OI-119), loading/start-page transfer (OI-120), live follow/T-21 (OI-125/126), tab i18n (OI-118).

## P6-F — complete

- `lmdv-io::tree`: flat listing/search model with the specified rules — 4 levels, 2,000 Markdown files, skip hidden/build/system folders, Markdown-only extensions, directories before files, empty folders pruned, 200 search hits with 160-char previews.
- `lmdv-app::browser`: folder header/up/refresh, search box, virtualized tree with collapsed state persisted through `ui.json`, focus restoration, roving focus stop, per-window `TreeWatcher` with unchanged-listing scroll preservation, background/foreground/new-window opens, folder links re-root, `Ctrl+Shift+O` / `Ctrl+Shift+F`.
- 17 browser tests + 8 tree tests; app 411, io 80; clippy/fmt/gate green. Scene `browser` with `LMDV_BROWSER_ROOT`; script `tests/scripts/browser-search.json`; evidence `/tmp/opencode/p6f-*`.
- Residuals: F-12 context menu, F-18 hidden state, F-19–21 pane behaviors; multi-window ui root sync/watcher limits; search query → viewer find (OI-128–131).
- Test flake fixed (`7b1df7a`): opening a file also queues `TreeListed`, so the two load tests now poll until the document reaches Ready/Error instead of waiting for any mailbox message.

## P6-X — complete

- DOCX footnotes are now native OOXML: `word/footnotes.xml` with separator/continuation entries, `FootnoteText`/`FootnoteReference` styles, body `w:footnoteReference` runs, first-reference bookmarks and localized backlink tooltips; `document.xml.rels` + content type; note-scoped `footnotes.xml.rels` for pictures/links; `w:footnotePr` in `sectPr`.
- Repeated references reuse ids; notes can carry runs/links/code/tables/images/math with the body writers; no plain end-of-document list.
- 23 DOCX tests (export 132) pass; `unzip -t`, structural XML checks and python-docx open cleanly; evidence `/tmp/opencode/p6x-test-features.docx`. LibreOffice smoke skipped (not installed).
- OI-066 (no OMML) and OI-079 (uncompressed package) remain.

## P6-V — complete

- Reader find bar (R-16–R-19): `Ctrl+F` in viewer opens a floating bar, prefill from the article selection (≤100 chars), case-insensitive plain matching over drawn model text mapped through source maps, `current/total`, Enter/Shift+Enter/F3, `n`/`N`, Esc, per-tab state.
- Matches paint as `Find`, current as `Current`; reveal uses `highlight_key` + `scroll_into_view` with a 58 pt band. `--line` paints as `Line` while the flash lasts. Folder-search results carry their query into the opened tab (F-16).
- 18 find tests; app total 430. Headless scenes `reader-find`/`reader-line`, script evidence; `/tmp/opencode/p6v/`.
- Limits: R-19 band approximations, task-checkbox `--line` object, search surface excludes front matter/TOC titles/math/Mermaid/raw HTML (OI-132/133).

## P6-PDF-T — complete

- PDF tables measure their min-content width in Typst, then either stay plain (byte-identical), scale proportionally with the 0.3 floor, or use a break-anywhere fallback; headers repeat, tagged tables, images/code scale, nested quote/callout/list budgets apply.
- 19 PDF tests (export 140); bbox check shows zero words outside the A4 margins on a 12-column/180-char-token fixture; normal PDFs remain byte-identical. Evidence `/tmp/opencode/p6-pdf-wide*.pdf`.
- Residuals: U+200B below the floor, inline HTML/oversized image edge cases, hard-coded insets, measuring pass (OI-134).

## P6-T — complete

- Every GFM table gets a hover/focus bar (**Copy table (for Excel)**, **Open preview**) via the additive renderer `TableWire`/`TableRef`; no-wire trees stay byte-identical.
- Copy builds the table's TSV through `lmdv-export::table_tsv`, excluding filter-hidden rows, and writes it with the Rune clipboard plus a check/notice.
- Preview modal: purpose-built grid with sticky header, frozen first column on overflow, virtual rows, 80 ms filter (every whitespace word, accent/case-folded, highlighted), `n of total rows`, no-rows message, typed stable sort (asc/desc/original), Esc/outside close, modal shortcut swallowing; refuses empty tables.
- 26 app tests (456 total) + renderer 50; clippy/fmt/gate green. Scene `table-preview`, script `tests/scripts/table-preview.json`, evidence `/tmp/opencode/p6t/`.
- Gaps: V-12 zoom/V-17 links, refit on resize (OI-135); grid/bar/sort substitutes (OI-136); three notice keys (OI-137); pre-existing reader table width overflow (OI-138); pinned scrollbar (OI-044).

## P6-PDF-H — complete

- PDF raw HTML blocks now render structurally: tables (thead/tbody/th/td, alignment, colspan/rowspan via Typst cells, header repetition), ordered/unordered nested lists with `start`, details cards with localized fallback title, quotes, pre/code and media filename placeholders; local media become file links, remote URLs never print.
- Zero-width inline HTML objects (standalone media/details) now reach the writers instead of being dropped by the range slicer.
- 20 PDF tests (export 147); A4/tagged, qpdf clean, bbox inside margins, no remote URLs; evidence `/tmp/opencode/p6-pdf-html.pdf`. OI-087 closed; parser-level details/figure/span residuals OI-139.

## P6-L — complete

- Renderer `MediaWire`/`MediaRef` for block images and supported Mermaid diagrams (additive; no-wire trees byte-identical, broken media never reaches the wire).
- Full-window lightbox: click / Enter / Space / block-menu “Open preview”; pan (3 pt threshold, capture), wheel/Ctrl+wheel zoom anchored at the pointer (0.5–2), +/−/0/W/1 keys, Fit/Width/100%, resize re-fit, clamped pan, focus return, modal scrim and Esc/backdrop close. Diagrams re-render at window width.
- 19 app tests (475 total) + renderer 52; clippy/fmt/gate green. Scene `lightbox`, script `tests/scripts/lightbox.json`, evidence `/tmp/opencode/p6l/`.
- Gaps: copy/save PNG/SVG and rasterization (V-08/V-09), inline images, hint placement/pinch, Mermaid double layout (OI-140).

## P7-B — complete (early)

- `crates/lmdv-edit/examples/typing_bench.rs`: deterministic 32 KiB–1 MiB documents, per-keystroke single-field vs block-field+commit, syntax-span cost, undo/redo, and the OI-058 single-block worst case; p50/p95/p99/max, no CI assertions.
- Results on this machine (release): 64 KiB single-field p95 **0.031 ms**, 1 MiB block prose p95 **0.166 ms**, 1 MiB one-block field p95 **2.39 ms**, block undo/redo p95 ~0.07 ms. Single-field undo keeps 7 steps at 64 KiB and 0 at 1 MiB; block mode keeps 100.
- Verdict: 64 KiB threshold stays (typing has huge headroom; undo depth and app-frame costs are the real limit). OI-064 closed; OI-058 cap/relayout follow-up quantified. Full output `/tmp/opencode/typing-bench.txt`.

## P6-S — complete

- Reading progress hairline (R-03): 2 pt accent, `offset/max`, hidden ≤24 pt, editing/start/error; no timer.
- Reading time (R-04/R-05/T-35): 220 wpm + 500 CJK/min, ceil/min 1; full form with one tab, short form with several; hidden editing/full strip; tab description/tooltip carries time + words.
- Section line (O-07–O-10): heading path, measured spans, 56 pt band, table-top suppression, fast-scroll 2.4 pt/ms +120 ms settle, 250 ms outline hold, 500 ms double-press shield; aria-hidden spans click like the outline. Scenes `reader-progress`/`reader-section` (need `--frames 40`), script `tests/scripts/reader-section.json`.
- 32 new tests; app 507. Clippy/fmt/gate green; evidence `/tmp/opencode/p6s/`.
- Gaps: focus-under/table-about-to precision (OI-141); Back/glide/flash, hover card/T-35 pointer widths and word-count edge cases (OI-142).

## P7-SEC — complete

- `crates/lmdv-md/tests/security.rs`: 175 curated pathological documents (deep nesting, huge lines/tables, malformed math/HTML/YAML, controls/bidi, link/HTML policy, both fixtures) plus a fixed-seed 2,000-case fuzz; parses twice under `catch_unwind`, checks source/line/span/map/heading/footnote invariants and a 5 s case budget. Full suite ~1.3 s.
- Found and fixed a real panic: code-span math with a multibyte first character sliced at `start + 3` inside the character (`$` + “é”). `closing_dollar` now searches from `start + 2`; the regression test is un-ignored and the fuzz workaround removed. 105 md + 10 security tests.
- OI-143: inline-math paragraph rescans cached across the monotone scan; 10k formula lines 14.61 s → **46 ms** (20k 94 ms).
- OI-144: per-base suffix maps for GitHub and legacy ids; 4k duplicate headings 2.73 s → **21 ms** (8k 47 ms).
- OI-145: `SourceMap::source_range` clamps quote-expanded endpoints to the next segment or the inline's own source end; caller audit and regressions added.
- OI-146: every `srcset` candidate passes the same URL policy as `src`; the security corpus checks all candidates, including `<picture><source>`.
- Full gate green on 2026-11-17; clippy/fmt clean.

## P6-I18N-A — complete

- Six app catalogs at **600 keys** each, identical key sets and valid JSON; five CLDR plural bases (`editor.lines/blocks`, `rel.minutes/hours/days`). All new app strings go through `lmdv_i18n::t!`.
- Platform wording: `file.reveal` is “Show in folder”, reveal failures and start-page tips use file manager; File Explorer/Store-only passages removed from all six help pages and the Windows section rewritten as desktop integration. P6-I18N-B completed the full help-dialog integration.
- OI-033 relative times use `rel.justNow` and plural units; OI-083/109/118/137 command, rich-copy, window/tab and table notices are keyed.
- OI-152 settings strings keyed; reset keeps `print_break_h1`; E-37 editor scaling verified intact. Spellcheck and the test-only locale skip remain.
- OI-148 palette footer uses `Keybind` keycaps, heading cap 20,000, page step measures the list area. Rune still lacks `aria-autocomplete`/`aria-controls`.
- Evidence `/tmp/opencode/p6-i18n-a/` (palette English/Italian, settings Italian); full gate green on 2026-11-17: app **754**, i18n **18**, all workspace tests/clippy.
- Remaining: OI-093 buffer Undo, OI-148 Rune a11y/NFKD/recents, OI-152 spellcheck/test locale, OI-037, OI-104.

## P6-I18N-B — complete

- All six `locales/help/<id>.html` pages are embedded through `lmdv_i18n::help_page` (build.rs watches the folder); unknown locales fall back to English.
- F1 / `app.help` opens the full localized guide; `?` / `help.keys` keeps the compact shortcut list. Escape, Ctrl+W, `?`, F1 again, backdrop and Close dismiss it; focus returns to the toolbar.
- Dependency-free parser for the pages' HTML subset (headings, paragraphs, lists, key tables, kbd/code/emphasis, links, entities) renders one scrollable panel, cached per locale. It fits 480×760; the compact `?` list now scrolls too.
- `pasted_title` now handles setext headings and common inline forms (OI-160).
- Evidence `/tmp/opencode/p6-i18n-b/` (`help-full-1100`, `help-full-480`, trees, compact list); full gate green on 2026-11-17: app **771**, i18n **18**.
- Remaining: H-04 license/repository links (OI-165), no help section sidebar/TOC, full page is not virtualized (fine for six pages). Spellcheck/OI-037/OI-104 remain elsewhere.

## P6-TAB-QUICK — complete

- OI-147: file rows in the palette open in a background tab on middle press; focus return and page-selection restore are runtime-tested. Headings/commands/tabs deliberately ignore it.
- OI-158: a ⌄ all-tabs menu appears when the measured strip overflows, listing every tab (active ●, folder hint) plus New tab; kit fades/scroll stay. Scene `tab-overflow`.
- OI-159: Ctrl+PgDn/PgUp wrap, Ctrl+1–9 select by physical digit (Ctrl+9 last), Ctrl+Shift+PgUp/PgDn move clamped, and the tab menu's New tab inserts after its tab (Ctrl+T still appends). Compact Help lists the Tabs keys.
- 25 new tests; app **796**. Evidence `/tmp/opencode/p6-tabquick/` (`tab-overflow.png`, `tab-keys.json`, `tab-menu-new.json`, `help.png`). Full gate green on 2026-11-17.
- Residuals: OI-166 (plain vertical wheel over the horizontal strip, one-frame ⌄ appearance/resize, no keyCode fallback).

## P6-READ-C — complete

- OI-167: the article viewport is keyed only by `article-scroll`, so its offset survived a tab/document change. A tab switch, cached/recent open or closed-tab return could draw the new short or medium page at the previous document's offset — the nearest heading above — with the progress hairline starting part way. `begin_restore` now queues the top too and `apply_pending_scroll` issues `scroll_to_y(0)` instead of trusting a “fresh” viewport; a heading restore is cancelled only by Rune's **hand** report (wheel/drag/key/instant), never by the inherited layout report.
- Recents, error retry, Open, links, browser opens, tab strip/keys, reopen and close flush the restore in the same transaction, so the first frame lands at the target rather than flashing at the old offset.
- Regression tests: README after a 600 pt long page (first frame 0 and hairline 0), a second view of the same document, a remembered heading at −40 pt after an inherited offset, a top restore with a nonzero snapshot; the reading unit tests now drive the hand verdict. Targeted: `p2_tests` 20, `reading::tests` 15, `window::tests` 23, `nav_tests` 43, `session::tests` 22, plus links/commands/palette/view/scroll green.
- Binary evidence `/tmp/opencode/scroll-e2e/`: a restored two-tab session (OPEN_ITEMS at 3000 pt, README at top); tapping the README tab or Ctrl+Tab ends with `article-scroll` 0.0.
- Full gate green on 2026-11-18: app **802**, workspace test/check/clippy/fmt clean. Evidence `/tmp/opencode/scroll-e2e/` (`tree.json`, `tree-ctrl.json`).

## P6-READ-D — complete

- OI-168: the actual cause of the reported README snap. The document watcher watches the parent folder and accepted **every** event that named the file, including the `Access(Open(Any))` Linux emits when the app itself loads it. Every load therefore looked like a change: the watcher reported `Changed`, the app reloaded the file, and the reload restored the tab's old anchor. Scrolling up from “Known packaging findings” was pulled back to the bottom every couple of seconds; the outline highlight and progress hairline flickered with each restore.
- `DocWatcher::touched` now ignores all `Access` events (open/read/close). Content changes still arrive as `Create`/`Modify`/`Remove`; a write's access events accompany its `Modify`, so nothing is lost.
- Regression: `the_live_watch_ignores_its_own_reads` reads the watched document and asserts no event, then writes it and asserts `Changed`; `reading_a_document_is_not_a_change` covers read/open/close versus a data write. IO **83** tests.
- Before/after evidence: `/tmp/opencode/live-race/trace.log` shows the repeated `file_event Changed` → `start_reload` → `begin_restore`; the fixed build keeps a wheel-up in place for five seconds (`/tmp/opencode/live-fixed/up6s.png`).
- OI-167 remains a separate fix (the shared article viewport kept the previous document's offset); it was real but not this symptom.
- Full gate green on 2026-11-18: app **802**, io **83**; workspace check/test/clippy/fmt clean.

## P7-RUNE — complete

- Vendored Rune `cca5df6fafb4721d96fe105f126ebfd2ec351bb6` (2026-11-17; 226 commits from the previous pin). Toolchain stays Rust 1.99.1. `docs/RUNE_PIN.md` and `vendor/rune/UPSTREAM.md` updated.
- Patches 0001 and 0002 rebased (import/insertion re-anchor in `text.rs`; `Shared` moved to `app/requests.rs`); `scripts/apply-rune-patches.sh --check` and `scripts/test-rune-patches.sh` pass (3 close-veto, 6 inline-layout, 2 inline-scene).
- Root: Rune `tooling` feature enabled for release headless gates, `accesskit_macos` patch added, `tiny-skia-path` remains Rune's path dependency; `scripts/check-vendor.sh` checks all of it.
- `fast_exit` is Rune's default now; `run_with_dirs` calls `rune::orderly_exit()` so window `Drop` and the single-instance socket are still cleaned up.
- Full gate green on 2026-11-17: app **754** tests, render 53, md 105 + 10 security, io 81, export 49 + integration; clippy/fmt clean. `reader` snap/tree inspected (inline math, checkboxes, headings).
- Post-bump visual/manual checks were verified by the user; OI-164 is closed. OI-127/141/150/126 remain upstream gaps; OI-147 is unblocked by `El::on_middle_press`.

## P6-C — complete

- Command palette (C-01–C-13): `Ctrl+K` viewer / `Ctrl+Shift+P` everywhere, toggle-close, Esc/backdrop.
- Groups: enabled commands, h1–h4 headings with ancestors, other tabs by recency, recent files and browser-root files (deduped, capped), plus Find/Search rows.
- Fuzzy matching (accent/case/ß/full-width, contiguous then ordered, word-start bonuses), `>`/`#` prefix modes, outline centring, score→recency→shorter ranking, 8 persisted recents (P-10).
- Enter/Ctrl+Enter/Ctrl+Shift+Enter/Shift+Enter activation, PageUp/PageDown groups, modal key trapping, focus return, combobox/listbox/option a11y with live count.
- 61 new tests; app 568. Scene `palette`, scripts `palette*.json`, evidence `/tmp/opencode/p6c/`.
- Gaps: middle-click/button info (OI-147 accepted), ARIA attrs/NFKD/heading bound/footer (OI-148), missing commands for full C-11 coverage (OI-149).

## P6-CMD — complete

- C-11 command coverage: paste-as-document (`Ctrl+V` in viewer/start), copy path/reveal, `view.pin` (X11; clear Wayland refusal), `tab.next`/`tab.previous` (`Ctrl+Tab`/`Ctrl+Shift+Tab`), five theme commands, `app.reading-settings`, `app.help` (F1) and `browser.up`; palette start group leads with Open/Paste/Search/New window/Reading settings/Help.
- T-07 reopen: per-window 25-tab stack with old index and reading place, pasted text restored as originally pasted with its paste id, `Ctrl+Shift+T`; already-open documents activate instead. T-18 per-kind tab menu (Copy path/Reveal files, Save as… pasted, Close others/right, Reopen). T-23 close-others/right runs sequentially through the ordinary dirty prompt; Cancel or a failed save stops it.
- 36 new action tests; app **701** tests. Scenes `tab-menu`/`tab-menu-file`; scripts `tab-cycle`, `tab-menu`, `palette-new`, `palette-start-page`; evidence `/tmp/opencode/p6cmd/`.
- Full gate green on 2026-11-16 (`scripts/run-gates.sh`): workspace check/test/clippy, 701 app tests. Vendor patch proof tests are now separate.
- Gaps: T-17 overflow list (OI-158); Ctrl+PgDn/PgUp, Ctrl+1…9, Ctrl+Shift+PgUp/PgDn move and T-04 menu insertion (OI-159); new notices/title subset/File Explorer wording (OI-160); closed stack not restored across sessions (OI-121).

## P6-READ-A — complete

- Reading keys: `j`/`k` scroll 80 pt and accumulate from the previous target within 400 ms; `Tx::key_repeat` makes held repeats instant. `Space`/`Shift+Space` page 90% of the viewport; Rune owns `PageUp`/`PageDown` and is not double-handled.
- Headings: `.`/`,` step through the outline on a pinned index (including trailing headings), `g g` goes top and `G` end with the 500 ms/single-`g` rules. Heading, outline and `#anchor` jumps glide on Rune's spring; `--line`, remembered positions, Back/Forward and new-document restores stay instant. Reduced motion jumps in one frame.
- O-04 pin: outline clicks, section-line parts, anchors and `.`/`,` keep the jumped row highlighted through the glide; wheel/drag/instant reports release at once, otherwise after 250 ms of quiet; reparse/tab/edit/scene resets drop it.
- 20 integration + 6 unit tests; app **727**. Script `tests/scripts/reading-keys.json`; trace and trees `/tmp/opencode/p6read/`. Full gate green on 2026-11-16.
- Gaps: landing flash and section-line hold timing (OI-142); focused-link Space substitute (OI-161); footnote jumps stay instant (OI-162); OI-132/141/017/127/098 and the `?` overlay remain for P6-READ-B.

## P6-READ-B — complete

- R-19 find scrolling: 60 pt top/bottom bands, +30 pt while the section line shows, centre-vs-reveal decision from the owning block with one post-reveal probe, and a 40 pt horizontal scroller margin. Block proxy residual in OI-163.
- O-10/O-09: heading landing flash (static 1.2 s accent band; fade residual in OI-163), open-ended section-line hold released after the glide is still for 250 ms, and the outline activation line slides through the last viewport; outline rows carry a button role.
- OI-098: `F2` / palette `Edit from here` opens the source at the reading block; word boundaries use a sorted combining-mark table for major scripts. R-11: `?` toggles the keys overlay on the reader and start page.
- Notched mouse wheel: plain vertical non-precise deltas accumulate over 200 ms and glide through `Tx::scroll_to`; trackpad/precise, modified and horizontal wheels stay native. This is an intentional extra over a browser-based viewer.
- Evidence `/tmp/opencode/p6readb/`; scripts `reading-{wheel,help,landing,end-spy}.json`; app **754** tests. Full gate green on 2026-11-16.
- Residuals: OI-127 zero-width object paint is blocked by Rune's shaped-glyph highlight boxes; OI-141 still approximates table-header height; OI-161 focused-link Space and OI-162 footnote-jump glide remain open.

## P6-H — complete

- Per-tab Back/Forward history (O-12–O-15): places = document key + scroll anchor; 50 cap, Forward cleared on a new place, <4 pt dedupe; record on cross-document links, outline/section jumps, `#anchor`, `file.md#section`, and any open; never on reload/Back/Forward; 8 pt landing suppression.
- Travel switches tabs, opens files waiting for the load, restores anchors, drops gone files with “Not found”, drops missing pasted places silently, and never lights the landing flash. Alt+←/→, mouse side buttons, tab-menu entries and palette commands.
- Per-tab lists survive switches and session restore (`TabSession.nav` additive); renames move places; rail/palette tests 32, app 600, io 80.
- Evidence `/tmp/opencode/p6h/`; scripts `tests/scripts/nav-history.json`, `nav-buttons.json` (1280×900).
- Gaps: BrowserBack/Forward keys absent in Rune and Alt+arrows consumed by focused fields (OI-150 accepted); one-frame heading geometry, pasted cross-window, background-tab menu, session clone cost (OI-151).

## P6-M — complete

- File tree context menu (F-12): files Open in new tab/window, Show in File Explorer, Copy path, Copy as Markdown link (relative/percent-encoded, extension stripped); folders only reveal/copy; Shift+F10/Menu key, A-08 navigation, focus return.
- Tab hover card (T-16): 550 ms/immediate tab→tab, full name/path, dirty or reading time/words, pasted/start variants; hide matrix; T-35 reading time hidden while dragging/pointer-frozen.
- 20 new tests; app 620. Scenes `browser-menu`/`tab-card`; scripts; evidence `/tmp/opencode/p6m/`.
- Residuals: right-click-while-open gesture, strip scroll/hover-out grace, reveal error key, background-tab time refresh.

## P6-PREF — complete

- Settings dialog (`Ctrl+,`, toolbar gear, palette `app.settings`): theme Auto/Light/Sepia/Dark/Black, size 13–22, Sans/Serif, spacing, width Narrow/Normal/Wide/Full, panel size, large-file mode, format bar, spellcheck (disabled), language Auto+six, session restore, H1 page break, reset confirm.
- All changes funnel through `AppShared::set_prefs` (store patch + broadcast); theme/typography/panes/language apply live; session restore next start, large-file next edit; focus trap/roles/labels; 29 tests; app 651. Scene `settings`, script; evidence `/tmp/opencode/p6pref/`.
- Gaps: settings keys/reset/spellcheck (OI-152); palette polish notes (OI-154).

## P6-PREF palettes — complete

- Full Sepia (warm paper) and Black (true-black OLED) palettes from the reference tokens, applied to window chrome, panes, controls, article/`DocTheme`, code/tables/callouts and Mermaid; `is_dark` corrected (Sepia light, Black dark).
- E-37: editor fields are reader size × 0.875 with 1.6 line spacing, single-field and block mode, live re-measure; find/scroll sync uses the same line height. 16 → 14 pt, 22 → 19.25 pt.
- 13 new tests; app 664. Scenes `reader-sepia`, `reader-black`, `editor-size`; tree color proof and live-size caret evidence.
- Residual polish: native frame colour, exact sepia hljs hexes, neutral shadows (OI-154).

## P7-PKG — complete (recipes)

- Desktop entry, AppStream metainfo, MIME XML, AppImage builder (`--dry-run`/AppDir staging), Windows NSIS installer, packaging README.
- Original icon `assets/lmdv.svg` + 512/256 PNGs (page + ochre bookmark), validated at 16–1024 px and grayscale/light/dark.
- `.github/workflows/release.yml` builds a Linux AppImage + raw binary and a Windows NSIS installer on `v*` tags; **v0.5.0**, **v0.5.5**, **v0.6.0**, **v0.7.0**, **v0.8.0** and **v0.9.0** were published from it; **v0.10.0** is the first with signed `SHA256SUMS`.
- Validation: shellcheck, desktop-file-validate, xmllint, JSON; AppStream expected warnings; no signed or reproducible release yet (OI-153).

## Cross-platform compile — complete (Linux-tested, Windows compile-checked)

- `lmdv-io` Unix-only code is now `#[cfg(unix)]` with Windows branches: plain OpenOptions/transient retry, no POSIX modes, dirs fallbacks, Windows path-prefix policy, compile-only single-instance stubs, Unix test helpers gated.
- `libc` is a Unix-only target dependency; Linux behavior and tests unchanged (io 81, app 665, full gate OK).
- Windows compilation is verified by the user (`cargo check --workspace --all-targets`), and `.github/workflows/ci.yml` repeats that compile check on `windows-latest` for pushes to `main` and pull requests. Windows is compile-checked only; the runtime gaps stay open in OI-155.
- CI: `ubuntu-latest` installs the winit/wgpu/fontconfig packages plus `poppler-utils`/`qpdf` so the PDF tests assert instead of skip, then runs `scripts/run-gates.sh` (parity checklist, vendor check, fmt, workspace check/test, clippy); `windows-latest` runs the compile check. Superseded runs are cancelled; Linux caches only the Cargo registry.

## P6-SAFE batch 4 — complete

- OI-005/007/010/012/071/073 fixed; OI-072 waits on the user's decision about the E-14 substitute (behavior unchanged).
- One document per file: load completion compares the canonical path and `(dev, ino)`. An already-ready document wins over the async read, a loading duplicate is absorbed, tabs keep their strip place, anchor and history, and other windows join through `UiMsg::DocumentMerged`.
- Save As (E-11/E-12): folder is the file's own, else the pane root, else the platform Documents folder; pasted Back/Forward places are renamed; a worker probe runs before any write, a dirty duplicate aborts, and a clean duplicate is released (in-window, or cross-window via `UiMsg::ReleaseDocument`).
- Startup: `AppDirs::resolve_reported` (portable→XDG fallback), the five `*_reported` loaders and `StoreHandle::start_with`/`protect_files` keep a future-schema or foreign state file untouched; one notice per launch.
- Documents over 8 MiB warn once and open in the reader; restored edit mode is suppressed and `Ctrl+E` still edits.
- A deleted or unreadable file offers its crash draft, and the recovered text reaches **Save again**; the document watcher's setup error is shown once.
- Commits `c2f37b2` (io) and `252b0c5` (app); 607 i18n keys; full gate green. New follow-ups OI-169–173.
- CI follow-up: the deleted-file draft test asserted the draft file before the store writer flushed it (run 44386181362). It now flushes the store first, so the check is deterministic; no shipped behavior changed.

## P6-I18N spellcheck — complete

- `lmdv-edit::spell` (Spellbook, pure Rust, no C dependency): dictionary discovery in the app's `dictionaries` folder, `$XDG_DATA_HOME/hunspell`, `/usr/share/hunspell` and `/usr/share/myspell`; the UI locale picks the tag (region then base) and English stands in only when nothing matches.
- `Spell::check` returns misspelled byte ranges (front matter, code, HTML, link targets and URLs skipped); `compose_underlines` paints them over the cached syntax spans without overlap in both editor modes.
- The app runs discovery, loading and checks on workers with a 300 ms trailing debounce; block mode checks only the visible rows. The Settings switch exists only while a dictionary is available, persists in `prefs.json`, and applies to spelling on the next start (E-32).
- 17 engine, 74 editor and 34 settings tests; fixtures under `tests/fixtures/spell/`; 608 i18n keys. Residuals: OI-174 (suggestion menu), OI-175 (wire key), OI-176 (test-only locale skip).

## P6-I18N math a11y — complete (patch 0003)

- `InlineImage::label` carries the words a screen reader says where the image draws; the renderer passes the localized `Math: {tex}` template for formulas and the alt text for inline pictures.
- Patch 0003 substitutes that label for U+FFFC in a11y names, values and text runs, with per-character lengths and word starts that still match the spoken value.
- `--tree` on `tests/fixtures/commonmark-math.md` reads `Math: E = mc^2` (and `Math: \sqrt{a^2 + b^2}`) where it used to read the object marker; the real `accessibility_tree()` regression test asserts no U+FFFC. `scripts/test-rune-patches.sh` passes with three patches.

## P6 batch 5 — reader, tables, panes — complete

- OI-127: Rune patch 0004 keeps an inline image's box when a text highlight covers its placeholder, so find/line highlights paint over formulas and inline pictures. The renderer also maps formula TeX, link destinations, front-matter keys/values and generated TOC labels.
- OI-135: table preview zoom (buttons, `1`, Ctrl+wheel, 0.5–2) with re-laid-out text, link-aware cells followed on click, task glyphs, and a resize that replans columns.
- OI-138: the reader passes the column width to tables, so a table wider than the 760 pt column scrolls inside it instead of overflowing.
- OI-162: footnote and back-reference jumps use the heading glide and landing when the target is on screen.
- OI-128/129/130: persisted pane visibility, auto-fold plus `SidebarPeek` overlay, widths with keyboard/palette commands, shared-vs-per-window browser state with broadcast, visible-only watchers, watch-limit notices, and the search→viewer-find regression test.
- OI-148: full NFKD folding table, real-store palette recents; Rune still has no `aria-autocomplete`/`aria-controls`.
- OI-169/170: draft diagnostics and protection in `lmdv-io` plus the app wiring; portable roots are checked for write access before selection.
- Four Rune patches; 617 i18n keys; full gate and patch proof tests green. New follow-ups: OI-178 (inotify overflow), OI-179 (pane grip arrows, accepted substitute).

## P5 extras — DOCX and PDF — complete

- OI-076: SVG assets (including Mermaid scene SVGs) rasterize to PNG during DOCX media registration with resvg/usvg — 2× intrinsic size, 4096 px cap, bundled fonts, file/URL references refused. AVIF still falls back to alt text (OI-183: no decoder in the tree).
- OI-078: a real Word `TOC` field with a dirty begin, hyperlinked cached entries and `w:updateFields`, plus `TOCHeading`/`TOC1`–`TOC6` styles. The cached result has no page numbers; Word computes them on update.
- OI-079: XML and `.rels` deflate at level 6; `test-features.docx` 95,541 → 52,617 bytes and `commonmark-math.docx` 118,800 → 68,429, still byte-identical across processes.
- OI-080: quoted lists use `QuoteListParagraph` with the muted run colour.
- OI-088: PDF/UA-1 validated by Typst, localized title fallback and alt text for images/formulas; short code and tables keep together with a page-boundary regression, long tokens keep U+200B. Long blocks still split (inherent), and CI has no veraPDF.
- OI-103/105/068 (app): exports flush and parse the exact snapshot; the exporting tab is pinned against switch/reorder/close/transfer; an ignored large-document benchmark records HTML 302 ms / DOCX 359 ms / PDF 4.81 s on a 140 KB document.
- Dependencies: `resvg` and `flate2` were already compiled in the Typst graph; no new package entered `Cargo.lock`.
- Full gate green after the batch. Open export items: OI-104 (held), OI-106 (needs the rune-mermaid serializer, OI-181), OI-183 (AVIF decoder).

## P0 missing — wiki links, click placement, file drop — complete

- L-03: `lmdv_io::resolve_document_link` probes the spelling, each Markdown extension, then `index`/`README`/`readme`; the app resolves non-Markdown links on a worker (`JobMsg::LinkResolved`), keeps anchors and drops delayed answers after a tab/document change. An explicit `.md` spelling opens directly.
- L-11: `WindowSpec::on_modifiers` feeds `LinkPlacement` — plain foreground, Ctrl background, Ctrl+Shift foreground, Shift new window, middle background. Residuals: plain opens a foreground tab (tab-per-document model), middle-drag cannot be distinguished, Shift loses the `#section`.
- D-01/D-02/H-12: the root accepts Markdown drops, shows “Drop to open” while a drag is over the window, opens first-front/rest-background, and inserts at the tab index from the measured slots; non-Markdown drops are refused with a notice. X11/Windows only (Wayland reports no file drags).
- Audit: 169 done, 52 substitute, 90 partial, 13 missing, 18 n/a; 45 P0 gaps remain (partials plus image drop and drag-to-scroll).
- v0.6.0 tag; release notes in `docs/releases/v0.6.0.md`.

## P0 tabs strip — batch complete

- T-02: pasted tabs show the clipboard glyph; file/start glyphs and the stem-only title with the full accessible name are unchanged.
- T-03: the saved check replaces the tab's leading mark for 1.4 s with a polite “Saved” announcement; typing restores the dot. The close-slot position needs a kit Saved status.
- T-05: Delete/Backspace on a focused tab closes it through the normal dirty prompt.
- T-13: a background open no longer consumes a front start page (browser/search/palette use `open_path_background`); a pasted tab is never replaced.
- T-19: runtime reorder and Esc-cancel tests; the strip drops drag/tear-out while an export pins the document; kit slop 4 pt accepted against the spec's 5.
- T-01: `viewport_width` now sizes tabs down to the kit minimum; the toolbar move and per-name widths need kit APIs.
- Audit: 173 done, 52 substitute, 86 partial, 13 missing, 18 n/a; 41 P0 gaps. Kit gaps tracked as OI-190.

## P0 batches — tabs, start page, editor, links, reading — complete

- Tabs: clipboard glyph, saved check + announcement, Delete/Backspace close, background opens no longer consume a start page, reorder/Esc/export gating. T-01/T-03 stay partial on kit APIs (OI-190).
- Start page/notices/a11y: full H-01 page with tips, recents Clear + middle-click + “yesterday”, notice kinds/icons and live-region roles, localized `?` overlay, `aria-keyshortcuts` assertion. H-02's dead-file pruning (OI-004) and H-06's bar position (OI-191) remain.
- Editor: 200 ms preview with parse-error containment, Discard/Cancel leave-edit, Setext-aware format bar. Insert/Tools menus remain (OI-192).
- Links/markdown: 24 pt anchor landing + table-header compensation, `tel:`, ignored unknown schemes, Open/Show-in-folder for other files, Not-found notice, Save-as for pasted links, same-share network rule, per-block code copy, query-dropping images, transactional rendering.
- Reading/system: width-based padding, F5 semantics, mtime-only touches, rename notice, outline scroll-spy, cold-start burst, 1200×860/480×480 window.
- Audit: 200 done, 52 substitute, 61 partial, 11 missing, 18 n/a; 18 P0 gaps remain.
- v0.7.0 tag; release notes in `docs/releases/v0.7.0.md`.

## Default Markdown handler — Windows and Linux — complete (`e4ade81`, v0.8.0)

- Settings → General has one “Make lmdv the default for Markdown files” row on Windows and Linux.
- Linux applies at once (`lmdv_io::assoc::linux`): reuses a usable `lmdv.desktop` byte for byte (the installer wrapper on this machine), otherwise writes a managed entry for `$APPIMAGE`/`current_exe` plus the embedded icon; sets `text/markdown`, `text/x-markdown` and `application/x-markdown` first in `mimeapps.list` under both default and added associations, preserving other handlers. No root, no `xdg-mime`/`gio`; best-effort `update-desktop-database`.
- Windows registers the same HKCU keys as the NSIS installer and opens Default apps (filtered on 11) with an Open-with fallback; `UserChoice` is never read, deleted or forged.
- API: `lmdv_io::assoc::{default_supported, make_default_for_markdown, Confirmation, AssociationError}`; the Windows planner stays pure and testable on Linux.
- i18n 633 keys; Linux detail and “applied” notice in six locales.
- Evidence: 24 `assoc` tests, 40 settings tests, 18 i18n tests; Linux + Windows-target checks clean; `--scene settings` shows the row. Windows runtime behavior still needs a real Windows run (OI-155).

## Scroll latency and frame pacing — complete (`ec3b50b`, v0.8.0)

- The reported “30 FPS / delay when scrolling quickly” was per-frame article work, not vsync: the reading view passes link/block/table/media wires, so `lmdv-render`'s per-block memo was unreachable and every wheel frame rebuilt the whole article. On the reporter's 951 KiB guide: **49.9 ms/frame (20 FPS)**, `view_cpu` 14–15.5 ms, layout 0.13 ms.
- `ReaderView::memo` now wraps the article in `rune::memo`, versioned by document revision (+ source hash while revision 0), theme fingerprint, content width, scale, highlight hash and app state (tab, mode, open details/front matter, table copy flash). Documents drawing a picture or diagram stay unmemoised (a picture decodes on a worker, a diagram follows the appearance).
- The wheel glide is speed-aware: Rune's 0.32 s spring for a single notch or a wheel after a pause, stiffening to 0.10 s during a burst so the content trails ~72 pt instead of ~300. Single-notch motion is unchanged.
- Result: **20 → 41 FPS** at 1200×860 (46.5 at 900×700), `view_cpu` **1.9 ms**, burst trail **297 → 117 pt**. The renderer's own commit/apply/paint over 15.5k nodes is now the cost; 60 FPS on this document needs Rune-side work or a virtualized article.
- No FPS setting: this Rune rev exposes no present mode or refresh cap, and every surface is already `AutoVsync` (desktop refresh). The speedup is the fix the reporter asked for; no API request is tracked.
- Fixture: `tests/fixtures/stress/developer-guide.md` (951 KiB, 10.5k lines, 51 tables, 186 fences); release notes in `docs/releases/v0.8.0.md`.

## P0 cleanup — E-22, H-06, E-15, E-17, T-12 — complete

- E-22: the format bar keeps the inline/heading/list buttons and adds **Insert** (table, code, formula, diagram, three callouts, divider) and **Tools** (find, find & replace, go to line, move/duplicate/delete/select line, spellcheck), with the plain “Type / for blocks” hint. The eight block buttons moved into Insert; `/` and the keymap are unchanged.
- H-06: one notice card at the top centre of the content pane with the kind icon/colour, action, dismiss ×, `status`/`alert` and polite/assertive live regions. It and the reader find bar share one pinned top-centre column, so they stack without overlap; scene `notice-stack` (also at 480 px). The R-19 find band does not grow by the notice height (OI-163 residual).
- E-15: a same-spelling reopen checks the disk through `ReloadIntent::ReopenCheck`: dirty keeps the buffer and asks the existing `DiskChanged` question when the disk differs; clean reloads differing text at its reading anchor; identical text is a no-op. A `--line` on an open editing tab moves the source caret and scrolls both panes.
- E-17: a draft is not offered while another window holds the shared edit lease.
- T-12: recents take all four click placements; document links already used `LinkPlacement::for_click` + modifiers, so the stale audit claim is corrected. Tree/search were already done.
- Tests: 14 new; `lmdv-app` lib 983 passed; scenes `editor-menus`, `notice-stack`; locales 634 keys.
- Audit: 205 done, 52 substitute, 56 partial, 11 missing, 18 n/a; **13 P0 gaps** left.

## Table cell overflow — fixed

- Long unbreakable tokens (especially code spans) were capped at the 360 pt wrapped-column width even as the “longest word” floor, so ink ran through the cell and table borders. The floor is now unbounded above and measured in the word's own face (code/`<kbd>` monospace at 0.9× body, strong 600, emphasis italic). When the floors do not fit the reading column, the table grows to them and the existing horizontal scroller is used.
- `docs/audit/tabs-windows-browser-palette.md` at 1160×680: the first table grows 573 → 1006 pt, its evidence column 360 → 793 pt, and the 776 pt text box sits inside it; ordinary tables are unchanged.
- A `word`×40 fixture grows 419 → 1650 pt and scrolls; a long code token is measured in the monospace face.
- The full-window table preview still uses its own 360-clamped planner (OI-194).
- Tests: `lmdv-render` `a_long_word_widens_its_column_past_the_cap_and_the_table_scrolls`, `a_long_code_word_widens_its_table_column_in_the_monospace_face`; updated app `a_table_wider_than_the_reading_column_fits_the_column`.

## P1 batch — editor, PDF, links, reading, windows — complete

- E-01: the editor surface fades in (`Transition::opacity()`, off under Reduce Motion), and Ctrl+E twice returns to an identical reading tree/roles/anchor/focus/scroll.
- E-07: exact per-source-line visual mapping (own font/spans/width, cached), block-local mapping, every table header that can stick measured, front-matter anchor, measured section-line reserve. Fixed the inert `b{n}/table/head` lookup shared with the section line.
- E-15/E-17/X-05/X-14: PDF/Print dates use the local day; Show in folder reveals and selects via `FileManager1.ShowItems` / `explorer /select,` / `open -R` with a folder fallback; Print temps clean up after a 30 s grace; the print-dialog failure path is tested.
- L-06/L-15/M-25/M-26: image links open the preview, percent-decoded paths, inline alt→src fallback, network `srcset` candidates dropped at parse. L-12 gained the safe `map`/`area`/`svg` + `xlink:href` parse subset.
- R-17/O-05/O-11: reader-find prefill trims/caps and handles empty selections; outline threshold/empty/per-tab/Esc behavior; `--line`/footnote jumps reserve the section line and front matter, with the table-header exception.
- T-14/T-29/T-31/S-13/F-11/W-07/W-09/R-29: background-tab queue insertion, drag-aware Ctrl+W, 3 s cascade + max/min inheritance, tree traversal (fixed Down skipping row 1), visible Pin button and quoted title, sticky notices.
- H-04: full Help About row with Repository and License (MIT) links; MIT `LICENSE` added; repo is public.
- Residuals/OIs: L-12 renderer/context menu (OI-195), SVG rasterizer (OI-196), R-17 block-menu find (OI-197), O-05 keyboard cursor (OI-198), inline picture hover (OI-182), diagram serializer (OI-181); T-26/X-02 and the Rune/kit P0s remain.
- Audit: **223 done, 52 substitute, 40 partial, 9 missing, 18 n/a; 9 P0 gaps**.
- v0.9.0; release notes in `docs/releases/v0.9.0.md`.

## P1/P2 batch — editor, reader, windows, packaging — complete

- Editor: E-03 caret from the measured block/`--line`, E-06 Mermaid cache, E-28 caret-anchored slash + one undo, E-30 comment no-ops, E-31 block go-to-line, E-34 tail, E-35 padding/keyless identities/preview-height caret, E-36 image paste/drop into `assets/`. E-29 keeps Ctrl+D only; multi-cursor is a Rune gap.
- Reader/export: R-11 modal key routing (guide body keeps PageDown/Home), R-17 Find in document menu, R-26/X-03 selection rich copy, X-15 stricter single parent rule (accepted substitute), A-13 table-preview a11y, O-05 outline cursor, L-12 area/SVG renderer + area wire, public SVG rasterizer OI-196.
- Windows: T-26 per-tab sticky notice/scheme (tree root per window, OI-129), H-02 dead recents on the display clock, H-08/H-09/H-13 tests for saved check/error card/version.
- Packaging: S-08 `~/Templates` substitute, current installer icon, `SHA256SUMS` + signed `SHA256SUMS.asc` with the Ed25519 key, metainfo-only rDNS `io.github.leandromarceddu.lmdv`, ADR-026.
- New scenes: `tab-notice`, `version`; help PageDown fix.
- Audit: **242 done, 54 substitute, 23 partial, 5 missing, 18 n/a; 8 P0 gaps**.
- v0.10.0; release notes in `docs/releases/v0.10.0.md`.

## Project site — GitHub Pages — live

- `site/` is a static landing page (start/reader/editor screenshots, features, platform downloads, latest-release asset links with release-page fallbacks, checksums link) published by `.github/workflows/pages.yml` at <https://leandromarceddu.github.io/lmdv-rs/>.
- The repository homepage and AppStream `<url type="homepage">` point at it; `appstreamcli validate` is now clean.
- ADR-027 supersedes ADR-025's no-homepage clause; OI-157 is fixed. No analytics or app webview.

## Diagrams, Windows runtime, UI polish — complete

- Diagram export: `rune-mermaid` patch 0005 adds static SVG serialization; rich copy/HTML/DOCX/PDF embed diagrams, and the lightbox gives diagrams 2×/4×/Save PNG through the public SVG rasterizer. X-02/X-07/X-08/X-11/V-08/V-09 green; OI-106/181 fixed.
- AVIF: `imgalifier-avif` (pure Rust, no NASM) decodes into PNG for DOCX/PDF and lightbox copy/save; extensionless AVIF is sniffed. OI-183 fixed.
- Windows runtime (bounded): named-mutex + named-pipe single instance, `ShellExecuteW` external open, volume-serial/file-index identity, current-user DACLs; compile-checked only. OI-155 remains open for the manual Windows checklist.
- UI: reader drag-to-scroll with autoscroll, toolbar breakpoints + Save/Aa, background-tab pulse, localized palette keycaps, right-click link menu. D-06/R-28/T-34/W-10/T-15/I-07/OI-195 menu closed; L-12 keeps only inline SVG vector shape drawing.
- Table preview floors now grow past 360 and scroll; A-01 four-theme contrast (Light accent darkened); I-08 palette keyword test. OI-194 fixed.
- Audit: **263 done, 54 substitute, 7 partial, 0 missing, 18 n/a; 6 P0 gaps**; 12 open OIs. v0.11.0.

## Rune f4ec14e — ported (v0.11.0)

- Vendor moved from `cca5df6` to `f4ec14e` (927 commits). The root repeats
  eight `[patch.crates-io]` entries; `check-vendor.sh` passes with four
  patches.
- Patch 0002 is retired: upstream `on_close_request -> CloseResponse` runs
  before hiding; the app returns `Defer` while a document is dirty.
- 0001/0003/0004/0005 are re-ported to `rune-text`, `a11y` and the modular
  `rune-mermaid`. Applying the four patches to a pristine formatted `f4ec14e`
  tree reproduces `vendor/rune` exactly.
- Patch proof: 7 inline-layout, 2 inline-scene, 2 a11y, 1 highlight and 9
  Mermaid SVG tests pass.
- Bump regression fixed: Rune also routes the synthetic Shift+F10/Menu press
  to the row. `by_keys()` presses now take the keyboard anchor, so the menu
  follows the focused row again.
- Pre-existing, not ours: `cargo test -p rune-text` fails
  `rtl_mirrors_positive_and_negative_indent_to_inline_start_edge` on a
  pristine `f4ec14e`; it is outside the workspace gate.
- Version 0.11.0; release notes in `docs/releases/v0.11.0.md`.

## Rune 07bf8c5d3 — patches retired (unreleased)

- Pin moved from `af6b1fdd` to `07bf8c5d3` (8 commits). Upstream #2743
  implemented exactly the four behaviors we carried as patches: fractional
  `InlineImage` metrics and `rise`/`label`, a11y label substitution,
  inline-image highlight boxes and static Mermaid SVG export.
- `vendor/rune/patches/` is empty. lmdv-render now attaches images with
  `El::inline_image` and exposes `MathImage` to the math resolver; the app no
  longer depends on `Span::image`/`Span::rise`.
- `scripts/test-rune-patches.sh` now runs the upstream feature tests:
  8 cursor, 16 pill, 145 a11y, 160 Mermaid and 1 kit-Mermaid test pass.
- Full gate, Windows `--all-targets` check and the headless proof-math/tree
  evidence are green. The retained memory fix from `af6b1fdd` remains.
- Residual: Sub/Sup no longer shifts the baseline vertically (only font size);
  recorded as OI-200, an accepted substitute. No tag for this bump; version
  remains 0.11.0.

## Worker batch — parity and polish — complete

- **SVG/HTML:** OI-025/026/041/042/195 closed. Supported SVG elements draw as
  `Shape` in blocks and inline; advanced SVG (defs/use, gradients, CSS,
  clip-path, dasharray) remains a documented fallback.
- **ARIA/drag:** A-05, A-06 and D-08 closed with `--tree` proofs. A-06's
  `owns` names the folder's first visible child row (flat virtual list), a
  documented substitute; roving focus stays OI-131's activedescendant model.
- **Input:** C-01 closed (IME chords suppressed); O-14's Alt+arrows half
  closed via a window-root `on_key_preview`. BrowserBack/BrowserForward keys
  remain upstream-blocked.
- **Reader:** OI-182 closed — pinch was already wired and now has a test;
  inline pictures carry a localized hover help on their own link.
- **Tasks/editor:** OI-058/059/074/093/094/096/097/111/112 closed. The block
  journal is capped at 256 KiB per field with an exact-undo barrier;
  identical nested task keys carry source ranges; the block wire no longer
  clones drawn text per frame. OI-061 and OI-102 remain partial with exact
  shared-design reasons recorded in OPEN_ITEMS.
- **Multi-open:** OI-002/T-32 closed with `Tx::pick_files` — first picked file in front, rest in background in selection order; headless proof with three files.
- **Session:** OI-117/120/121/122/123/124/125 closed; OI-126 keeps its menu
  substitute. OI-125's torn window follows the pointer and adopts the tab on
  release (no mid-drag ghost).
- Applying the four Rune patches still reproduces `vendor/rune` from pristine.
  Full gate green; no tag for this batch.


## Next up

**Next when work resumes:**

1. Optional: Rune-side commit/paint cost or a virtualized article for 60 FPS on very large documents (the app-side view is now 1.9 ms). No FPS setting is planned.
2. Close or accept the 6 remaining P0 rows: T-01/T-03/D-04/I-10 need rune-kit patches (OI-190); O-14's browser-key half and L-11's middle-drag release need small Rune patches. C-01, O-14 Alt+arrows, L-12, A-05/A-06, V-02 and D-08 are done; E-29 stays a large Rune patch or an accepted gap.
3. Decide the two shared residuals: OI-061 needs a live bounded syntax-tree source for format-bar/slash state, and OI-102 needs an owned layout callback after virtual remount. Then take the remaining 12 OIs in bounded slices.
4. P7 packaging: Authenticode/AppImage signatures and reproducible builds; rDNS, homepage, installer icon, LICENSE and signed SHA256SUMS are done. Windows runtime is implemented but needs the manual checklist in OI-155.

**Fixed flake (task queue):** `tasks_tests::a_conflict_with_a_queued_tick_reverts_every_optimistic_byte` used to change disk after both toggles, so the save could finish first and no conflict occurred. It now changes disk while the first save is in flight and before the second toggle; 12/12 stress runs and the full gate pass.

**Known-failing:** none in the workspace gate. Outside it, the vendored `rune-text` suite fails `rtl_mirrors_positive_and_negative_indent_to_inline_start_edge` on a pristine pinned vendor tree too, and an unrelated macOS Rune smoothing test fails on Linux.

**Known flake:** CI run 44821493750 failed once in `reading::tests::a_viewer_find_hit_over_an_inline_image_paints_its_box` with Rune's debug assertion `text_spec.rs:245: a text changed after its shape key was taken`; the rerun and eight local full-suite runs passed. Record any repeat with the run id.

**Known flake (2026-11-22):** CI run 45052468637 (`3d988a0`) failed twice on the Linux gate, each time in an unrelated test: attempt 1 and the docs follow-up `aaef8a0` timed out `lmdv-io store::tests::drafts_are_capped_by_count_and_characters` on its first `flush(2s)`; attempt 2 hit the known Rune debug assertion `text_spec.rs:245: a text changed after its shape key was taken` in `reading::tests::a_viewer_find_hit_over_an_inline_image_paints_its_box` (the same flake as run 44821493750). The drain batch fsyncs every draft, and CI's loaded disk can exceed two seconds; its first flush timeout is now **5 s**, matching the second half. The Rune assertion remains a low-probability shape-cache flake: the test alone passed 30 local runs and five full app-suite runs on two cores. Record any repeat with the run id; do not mask it with another timeout.

**Known flake (instance lock) — tmpfs artifact:** `lmdv-io instance::tests::a_stale_socket_is_reclaimed_but_a_live_one_is_not` intermittently panics with “a free lock must be primary” under the parallel suite. Forensics on 2026-11-24 reproduced it 7/30 on `/tmp` (tmpfs), 0/10 single-threaded, and 0/35 with `TMPDIR` on btrfs; `flock` returns `EWOULDBLOCK` with no fd or `/proc/locks` entry. This is a tmpfs/sandbox artifact, not app behavior. For a clean gate use `TMPDIR=$PWD/target/tmp cargo test -p lmdv-io` (or the full gate with that TMPDIR).

## P6-01b — complete

- Added 10 `render.*` keys to all six catalogs (callout titles, footnotes, image, Mermaid, details, table); P6-01c added four more (external-open, footnote number, math, backlink); all 413-key sets identical and valid JSON.
- `DocLabels::localized_in(locale)` now localizes callouts, image, footnotes, Mermaid, details and table; `english()`/`Default` unchanged.
- `lmdv-export` resolves the same keys from `ExportDoc.lang` for HTML/DOCX/PDF (no new option field); callout titles, footnote labels, image/Mermaid alt text and the “Pasted text” title fallback are localized.
- Tests: 16 i18n, 34 renderer, 82 export (26+19+21+16) pass; non-English HTML/DOCX/PDF assertions plus English regressions. Residual a11y strings are OI-099.

## P6-01c — complete

- Added four `render.*` keys to all six catalogs (413 total): external-open, footnote number, math label, footnote backlink. Placeholders are exact in every locale; JSON valid and key sets identical.
- Wired reader a11y labels (`DocLabels::footnote`, `math`, `open_externally`) and HTML export backlink/math aria labels via `ExportLabels` and the document `lang`; English defaults unchanged.
- 17 i18n + 36 renderer + 105 export tests; clippy/fmt clean. OI-099 closed.

## P2-02 — complete

- `lmdv-math`: RaTeX parse → layout → raster, straight-alpha RGBA, exact ascent/descent in points, bounded LRU cache (entries + 32 MiB), embedded KaTeX fonts. 18 tests pass; PNG evidence in `/tmp/opencode/math-*.png`.

## P6-01 — complete (early)

- `lmdv-i18n`: six ported catalogs now at 413 keys (399 base + 14 labels), compile-time keys, single-brace placeholders, locale negotiation, numbers/lists; tests + doctests. Help pages copied. `Cargo.lock` includes `serde_json` for tests.

## P2-01 — complete

- `lmdv-md`: Rune-free parser/model with CommonMark + GFM tables/tasks/strike, footnotes, GitHub callouts, flat YAML front matter, heading ids/legacy/reserved, `[TOC]`, the exact GitHub math scanner with same-byte/same-line placeholders, HTML tagfilter + allowlist subset, autolinks, source maps, block/heading keys and diagnostics. 97 tests pass; clippy/format clean.

## P2-03 — complete

- `lmdv-render`: production Rune renderer over `MdDoc` with block/heading keys, selectable prose, code + syntax, lists/tasks, quotes, GFM tables with sticky headers, Mermaid, block/inline images, callouts, front matter, footnotes, TOC, HTML subset and localized labels. 34 tests pass; clippy/format clean.

## What works now

```bash
scripts/run-gates.sh
cargo run -p lmdv -- tests/fixtures/test-features.md --snap /tmp/opencode/lmdv-p2-features.png --size 1100x760 --scale 2
cargo run -p lmdv -- tests/fixtures/test-features.md --tree /tmp/opencode/lmdv-p2-features-tree.json
cargo run -p lmdv -- --snap /tmp/opencode/lmdv-p3-editor.png --scene editor --size 1100x760 --scale 2
cargo run -p lmdv -- --snap /tmp/opencode/lmdv-p3-block.png --scene editor-block --size 1280x900 --scale 2
cargo test -p lmdv-export
```

The viewer renders the full Markdown feature set; `Ctrl+E` opens the split editor with live preview, find/replace, format bar, slash menu, go-to-line, explicit save, drafts, watcher and close guards. HTML, DOCX and PDF export are libraries in `lmdv-export` awaiting the Share menu (P5-04).

## Open items

Every worker-reported limitation is tracked in [`OPEN_ITEMS.md`](OPEN_ITEMS.md) with a phase and status. P3-02 closed OI-006/008/011; the P3-03 scope (find/format bar, slash menu, go-to-line, block-mode UI, task ticks) is listed there with the remaining P3 items. Accepted gaps carry the reason and, where needed, the user's approval.

## Blockers / open questions

- The macOS-specific Rune test `text::macos::smoothing::tests::pipelines_never_share_masks` fails on Linux; it is unrelated to the patches and not part of the lmdv gate.
- PDF needs the app to supply a proportional sans + mono font (the crate default is bundled Libertinus Serif); font parsing should be cached in the export job.
- Product decisions still open: DOCX OMML vs images; rich-copy MathML; X-12 wide-table fitting in PDF; native Linux frame acceptance; cross-window drag scope; multi-cursor patch vs gap; editor threshold tuning.
- `typst` adds a large build graph (~37 s clean build here); keep an eye on CI/build time and binary size.
