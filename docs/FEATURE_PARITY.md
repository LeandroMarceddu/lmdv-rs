# Feature parity — implementation strategy

This document is the **bridge** between the behavior spec and the implementation plan. It does not duplicate the behavior spec.

| Document | Question it answers |
|---|---|
| [`BEHAVIOR_SPEC.md`](BEHAVIOR_SPEC.md) | What exactly does the app do? (342 behaviors, source refs, priorities, acceptance criteria, fixtures) |
| [`PARITY_CHECKLIST.md`](PARITY_CHECKLIST.md) | Which behaviors are done? (generated checkbox list, one per behavior) |
| **This file** | How will `lmdv` implement each area in Rune, in which phase, and where are the real gaps? |

**Rule:** every behavior in `BEHAVIOR_SPEC.md` is in scope. A behavior is either implemented, an approved `substitute`, or an `accepted-gap`. Nothing is silently dropped.

---

## 1. Status by area

Counts are generated from the behavior spec by `scripts/gen-parity-checklist.py`.

| Area | Prefix | Rows | P0 | P1 | P2 | Rune approach | Phase | Gate |
|---|---|---:|---:|---:|---:|---|---|---|
| Reading and viewer | R | 30 | 16 | 14 | 0 | reuse + build | 2, 4 | Golden reading fixture |
| Tabs and windows | T | 35 | 16 | 17 | 2 | reuse (TabStrip) + build | 1, 8 | Routing/drag matrix |
| Editor | E | 40 | 22 | 16 | 2 | build on `TextEdit` + patch | 3, 5 | Edit/save/draft matrix |
| Markdown rendering | M | 34 | 19 | 13 | 2 | reuse + patch | 2, 4, 6 | `test-features.md` / math fixture |
| Full-window preview | V | 18 | 2 | 15 | 1 | reuse (`ImageZoom`, `Table`) + build | 4 | Preview/table matrix |
| Share, print and export | X | 16 | 5 | 10 | 1 | build + Typst/OOXML | 7 | Export files open |
| File browser and folder search | F | 22 | 13 | 8 | 1 | reuse (`Tree`) + build | 2, 4 | Tree/search matrix |
| Outline, section line, history | O | 15 | 7 | 6 | 2 | build | 2, 4 | Scroll-spy/section line |
| Command palette | C | 13 | 5 | 8 | 0 | reuse (`Palette`) + build | 4 | Fuzzy/modes/modifiers |
| Links and media | L | 15 | 10 | 4 | 1 | build + `ImageCache`; external player for video/audio | 2, 4 | Link matrix |
| System, CLI and file integration | S | 27 | 15 | 6 | 6 | build + Rune prefs/dirs | 1, 10 | Double-click/CLI |
| Window chrome | W | 10 | 2 | 4 | 4 | substitute (native frame) | 8 | Resize/close/theme |
| Drag and drop | D | 8 | 2 | 5 | 1 | reuse + build + spike | 4, 8 | Drop/tab matrix |
| Help, start page, notices, rating | H | 13 | 3 | 4 | 6 | build | 2, 9 | Start/help/notices |
| Accessibility | A | 15 | 4 | 11 | 0 | reuse (AccessKit) + build | 9 | `--tree` + Orca |
| Internationalization | I | 11 | 4 | 5 | 2 | reuse (`rune-i18n`) + build | 9 | Six locales |
| Persistence | P | 20 | 7 | 9 | 4 | reuse (`AppDirs`) + build | 2, 3 | Restart/crash matrix |

**Totals:** 342 behaviors, 152 P0, 155 P1, 35 P2 (see the checklist for exact counts).

---

## 2. How Rune maps to the specification

### 2.1 Reading and Markdown (R, M, L)

**Reuse:** `rune-kit::markdown` already renders CommonMark, GFM tables, task lists, strikethrough, custom autolinks, syntax-highlighted code, images (block-level), native Mermaid, selectable text with browser-grade copy, and source-mapped find/highlight. This is a strong base for the viewer.

**Patch/extend:** the parser enables only strikethrough/tables/task lists (`parser.rs:466`). We need:

| Behavior | Approach |
|---|---|
| Footnotes | Enable `pulldown-cmark` footnotes in the vendored parser; add reference/definition blocks and backlinks. |
| GitHub callouts | Detect `> [!KIND]` at block-quote start; render per-kind card (icon/color/title). `rune_kit::notes::Note` exists but styles every kind the same. |
| YAML front matter | Strip before parse; render a collapsed block + table; exclude from print/copy/DOCX. |
| Heading anchors / legacy ids / reserved ids | Slugger in `lmdv-markdown`; attach keys/ids to heading blocks; keep legacy ids for old links/positions. |
| `[TOC]` | Build from parsed headings after the marker; real document content. |
| Raw HTML subset | `pulldown-cmark` currently drops HTML. Map a whitelist (`br`, `kbd`, `sup`, `details/summary`, `picture/source`, `img`, `video/audio`, safe `a/span`) to native elements; show tagfiltered tags as text; hide unknown tags. Full HTML/CSS is out of scope. |
| Inline images | Rune renders images as blocks; the specification lifts them too. Accept block-level, or use an inline image patch for edge cases. |
| Sticky table header | The Markdown table is one element; patch `markdown/table.rs` to emit a sticky header sibling, or build the table renderer in `lmdv-markdown`. |
| Wide tables / bottom scrollbar | Kit supports `scroll_wide`; add the proxy scrollbar / header translation on top. |
| Math | Not in Rune. RaTeX (pure Rust, KaTeX-compatible) → cached rasters; inline needs a Rune inline-box patch (see §3). |
| Mermaid | Reuse `rune-mermaid` (17 diagram families, much broader than the kit README says). Unsupported types fall back to a code block. |

**Media:** `ImageCache` handles PNG/JPEG/WebP/SVG (feature `svg`); relative path resolution and network-path refusal are app code. Rune's `Player` works on macOS/Windows only, so Linux shows a media placeholder + “Open externally” (`L-MEDIA`). GitHub light/dark image variants are app-side (`<picture>`/`#gh-dark-mode-only`).

**Selection/copy:** Rune's `selectable` + `copy_source` already covers cross-block selection, list markers, table tabs and source copy. Rich selection copy (HTML) may need an app-side serializer based on the block model; whole-document rich copy is straightforward.

### 2.2 Editor (E)

**Hard constraint:** Rune's `TextEdit` is excellent for a field but **not** for a whole large document. Measured: 12 ms/keystroke at 50 KB, 46 ms at 200 KB in one field; one field per block is 0.8 ms / 2.6 ms. Undo stores full snapshots (128 steps / 1 MiB). (`research/27-long-note-editing.md`.)

**Decision (ADR-016, implemented P3-04a):** two modes behind one `EditorSession`. Single-field while `len <= 64 KiB && lines <= 1500`; one `TextEdit` per blank-line block above that, virtualized with `El::virtual_rows`, a global journal so `Ctrl+Z` spans blocks, blank-line split/merge and Tab indent. The mode is chosen when the session starts (`Auto`/`Block` pref) and does not flip mid-session. Cross-block selection in block mode is a documented substitute. SP-03 typing benchmarks remain OI-064.

**Must build (not in Rune):**

- Gutter/line numbers synced to wrapped lines (or hard lines first).
- Find/replace engine (Rune's Markdown find is read-only; `TextEdit` has selection/insert).
- Multi-cursor (no support; model + overlay + one undo group; spiky).
- Linux spellcheck (Hunspell; Rune's platform spellcheck is Windows/macOS only).
- Format bar, `/` menu, line commands, comment toggle, go-to-line.
- Preview scroll sync and double-click-to-source.
- Drafts, EOL/BOM preservation, conflict guards, watcher integration.
- Rich paste/drop images into `assets/`.

**Reuse:** IME, undo groups, selection, rich clipboard, span overlays, pills (useful for mentions, not Markdown), text measurement.

### 2.3 Tabs, windows and shell (T, W, D)

**Reuse:** `rune-kit::shell::TabStrip` (reorder, tear-out, handoff, overflow, hover card), `SplitView`, `Sidebar`, `Toolbar`, `Page`, `Tree`, `Table`, `Palette`, `Menu`, `Dialog`.

**Build:** document registry shared across windows, launch routing, tab state, close guards, reopen stack, cross-window moves.

**Linux limits found in Rune (must be product decisions):**

| Behavior | Rune reality | Plan |
|---|---|---|
| Custom themed titlebar | Custom frame is macOS/Windows only (`window/chrome.rs:52-58`); Linux keeps the system frame. Borderless has no interactive edge resize. | **Substitute** for v1: native frame + themed toolbar. Optional Rune patch/CSD spike later. W-01/02/03/05/06 are `substitute`/`accepted-gap` on Linux. |
| Tear-out under pointer | `TabTearOut::screen_point` is `None` on Wayland; winit cannot position windows there. X11 works. | T-20: **P4-02c** release-only X11 (window appears on release), cross-window drop via the in-process transfer; Wayland/headless disable the gesture and use the tab menu (substitute). Live follow and native placement validation remain OI-125. |
| Cross-window tab drop | Needs pointer across windows; no cross-window capture. `TabTearMove` + window placements can hit-test on X11; Wayland has no window positions. | T-22: **P4-02b/c** in-process transfer with clean-merge/dirty-refuse, hover slot and X11 hit-test; Wayland uses Move tab to window…/Move to new window (substitute). |
| Always-on-top | `always_on_top` is a no-op on Wayland. | W-07: X11 works; Wayland `accepted-gap`. |
| Drag-out to other apps | Unsupported on Wayland/X11. | D/README drag-out: `n/a`; use Save/Export. (The app itself does not drag files out.) |
| Single instance | Rune already has a per-user Unix socket, but it forwards only `scheme://` links to `App::on_open_url`; bare file paths are not forwarded. | Either own socket (planned) or wrap launches as `lmdv://` and use Rune's instance. Resolve with the Linux integration design (SP-06). |

### 2.4 Export, print and preview (X, V)

**No PDF/print/DOCX/SVG in Rune.** `--html` is an internal first-paint export of the laid-out tree, not semantic Markdown, and raw-pixel images (math/diagrams) are dropped.

**Plan:**

- **HTML (P5-01, done):** `lmdv-export::export_html` builds `ExportDoc` from `MdDoc` and writes one self-contained file: semantic tables/code/quotes/anchors, data-URI images and formulas (2× PNG, TeX in `alt`), `file:///` links, TOC, print CSS, no scripts. Reader theme/size never reach it (X-10). **Accepted substitute (user, 2026-11-13):** formulas are 2× PNG rather than KaTeX/MathML HTML; selectable/searchable equations and rich-copy MathML remain follow-ups (OI-066). Remaining: MathML for rich copy, DOCX OMML, app Share wiring.
- **DOCX (P5-02, done):** deterministic OOXML from the same `ExportDoc`: real Heading1–6 styles, lists/numbering, GFM tables, callouts, code, hyperlinks, embedded images and diagrams, math 2× PNG with TeX alt, A4 print style. OMML (`X-08` native equations), `footnotes.xml` and a Word TOC field remain open (OI-066/077/078).
- **Share app wiring (P5-04a/b, done):** toolbar/`Ctrl+Shift+E` menu, one background export job, atomic save dialogs and notices, PDF fonts from the system, Print = temp PDF + system viewer, whole-document rich copy with Markdown fallback. Selection copy (X-03), table TSV (X-16), PNG/SVG and Mermaid raster remain.
- **PDF (P5-03, done):** Typst 0.15 in-process A4 layout: header title · localized date, `page / pages`, h1 page breaks, tables with repeated headers, callouts, code, embedded images/math with tagged `/Alt`, subset fonts, deterministic output. Print on Linux = this PDF + the system viewer. Gaps: app must supply sans/mono fonts, X-12 wide-table fitting, raw HTML blocks (OI-085–088).
- **PNG:** reuse Rune's headless renderer (`Gpu::headless`, `Offscreen`, `write_png`) for diagram/image capture and goldens.
- **SVG:** vendor `rune-mermaid` and add a scene serializer for diagrams; document SVG export is a spike.
- **Rich copy:** own HTML serializer; plain Markdown/TSV are easy.
- **Preview:** `rune-kit::image_zoom` is image-only; diagrams need a custom `Drawing`-based pan/zoom stage (`render_drawing` gives size/surface). Table preview uses kit `Table` + filter/sort.

### 2.5 Platform, persistence, i18n and accessibility (S, P, I, A)

**Reuse:**

- `rune::AppDirs` (XDG config/data/cache/state/logs) for path resolution; our own store thread handles the JSON files (Rune's `Prefs` rewrites synchronously and is not used for hot paths).
- AccessKit/AT-SPI, `El::role/label/...`, `Runtime::accessibility_json`, `--tree`.
- `rune-i18n` for six catalogs; system language detection + override.
- Rune clipboard (text + HTML on Linux, images), file dialogs via XDG portal, drop-files, open-url (`http/https/mailto`), tray/global shortcuts optionally.
- System fonts with CJK/emoji fallback; bundle a serif reading face (Rune has no serif slot; use `Family::Named`).

**Build:** recents/positions/drafts/session/tree state (Rune has no document store), file watchers (notify), single-instance file routing, portable mode, Linux MIME/.desktop/launcher, spellcheck, font picker (query fontconfig; Rune has no public font list), path rules (POSIX, case-sensitive, refuse network/device paths).

**Windows-only items:** S-07/08/09/11/15/25, W-03/05/06/08, X-05, I-05, S-16/17 — record as `n/a` or Linux substitutes in the checklist.

---

## 3. The genuinely hard items

| # | Item | Why hard | Decision needed |
|---|---|---|---|
| 1 | Inline math | Rune's only inline image is a **square** icon (skips selection/copy); correct formula aspect/baseline needs a Rune text patch (`text/pill_chips.rs`, `runtime/paint/content.rs`, `text.rs`). | SP-01: patch Rune vs block-only fallback. |
| 2 | Long-document editing | Whole-document `TextEdit` is 12–46 ms/keystroke at 50–200 KB, and undo is full snapshots. | SP-03/SP-08: block editor vs Rune layout/undo patch. |
| 3 | PDF/print | No page model in Rune; a second layout engine is mandatory. | SP-02: Typst in-process; accept non-pixel-identical output. |
| 4 | HTML subset | Rune has no HTML/CSS engine; only a whitelist mapped to native elements is realistic. | Define the supported subset in `BEHAVIOR_SPEC` terms. |
| 5 | Cross-window tab drag | No cross-window pointer capture; Wayland has no window positions/screen points. | SP-04: X11 spike; Wayland fallback. |
| 6 | Custom Linux titlebar | Rune keeps the native frame; borderless has no edge resize. | ADR-007: native frame for v1. |
| 7 | SVG export | Rune has no public display list; `rune-mermaid`'s scene is private. | Vendor `rune-mermaid` + serializer, or PNG only. |
| 8 | Linux spellcheck | Rune's checker is a stub on Linux. | `lmdv-edit::spell` (Spellbook) with system/user dictionaries and underline spans; the app suggestion menu is pending (OI-174). |
| 9 | Multi-cursor | `TextEdit` has one anchor/focus; secondary carets need overlays. | SP-03: model/overlays; can land after v1. |
| 10 | Rune pre-release | Pinned rev, no PRs, five `[patch.crates-io]` vendor dirs must be repeated in our workspace. | Patch discipline (ADR-002). |

---

## 4. Substitutions and accepted gaps

| Behavior | Specified | Planned | Status |
|---|---|---|---|
| W-01/02/03/05/06/08 | Themed draggable titlebar, caption buttons, Snap zones | Native Linux frame + themed toolbar; system controls | **substitute**, v1 |
| T-20 / T-22 | Tear-off placed under pointer; drop on another window | X11 exact if spike works; Wayland new window compositor-placed / menu move | **spike**, partial gap |
| W-07 | Always-on-top | X11 works; Wayland no-op | **accepted-gap** on Wayland |
| X-04 | System print dialog | PDF + system viewer/print | **substitute** |
| X-08 | Native Word equations (OMML) | Images first; OMML optional | **proposed gap** |
| X-08 / V-08 | SVG export | PNG first; SVG spike | **proposed gap** |
| S-07/08/09/25 | Windows associations, New template, recent/jump list, alias | XDG MIME, `~/.local/bin`, app recents; `.desktop` actions optional; Settings → **Make default…** applies the XDG association in one press (`lmdv_io::assoc::linux`) | **substitute** |
| S-11/15/16/17, W-03/05/06/08, I-05 | Store/MSIX/WebView2-native behavior | Not applicable | **n/a** |
| M-12 | Full Mermaid 11 | 17 native families; unsupported → code block | **substitute** |
| R-20/X-03 | Rich copy of a selection | Whole-doc rich copy done (P5-04b); selection needs a block-range serializer | **build** |
| R-07 | Space pages the article even with a control focused | Keyboard focus on an interactive control keeps Space activation (accessibility-first); PageDown always pages and Space pages with the article/block focused | **substitute** (OI-161, user 2026-11-20) |
| R-19 | Click-to-source centring and landing flash | Owning block as the hit proxy, 40 pt horizontal reveal margin, static 1.2 s landing band | **substitute** (OI-163, user 2026-11-20) |
| T-17 | Tab strip wheel translation and overflow timing | Horizontal/Shift wheel, physical digits and clamped moves; plain vertical wheel and exact ⌄ timing need kit APIs | **accepted-gap** (OI-166) |
| E-32 / S-16 | Native spelling suggestions menu | Underlines and engine suggestions; Rune's field menu has no app hook and a custom menu needs a text-hit API | **accepted-gap** (OI-174) |
| P-11 | Flat `editor-spell` key | Stored as `prefs.json` `editor.spellcheck`; off survives restart | **substitute** (OI-175) |
| Section line / sticky table | Per-frame focus query and measured header height | Poll-time focus tracking for keyed focusables; the sticky band uses the block top | **substitute** (OI-141, user 2026-11-20) |
| F-20 | Drag the inner pane edge; arrows on the grip; double-click resets | Drag, double-click reset and persisted widths work; the kit builds its grip `focusable(false)`, so the arrow keys live on the focused tree (`Ctrl`+arrows) plus eight palette commands | **substitute** (OI-179) |
| E-14 | Opening another document while editing a dirty buffer asks “You have unsaved changes. Discard them?” | No prompt: each tab keeps its own editor session, undo and crash draft, so switching documents discards nothing. Every destructive path (tab/window/native close, quit) still asks Save / Don't save / Cancel, and a save in flight defers the close. | **substitute** (user, 2026-11-18) |
| T-SHARED | A file may be edited independently in two windows (CodeMirror buffers + conflict detection) | One canonical buffer + one editor lease; other windows are live viewers; explicit takeover | **substitute** (ADR-004/023) |
| T-28 | No tab restoration across a full restart (only window geometry) | Session restore is an intentional extension; editor text comes from drafts | **extension** |
| E-MULTI | Multi-cursor (`Ctrl+D`, add cursor above/below) | Single selection in v1; `Ctrl+D` selects next occurrence; full model needs a Rune caret-rect patch | **gap until P7** |
| E-SPELL | WebView2 spellcheck follows the UI language | Spellbook (Hunspell) engine in `lmdv-edit`, system/user dictionaries, underline spans in both editor modes, toggle hidden without a dictionary; the right-click suggestion menu is still open (OI-174) | **implemented (Linux)** |
| L-MEDIA | Video/audio playback in the document | Rune's `Player` fails on Linux; show a placeholder + “Open externally” | **substitute** |
| X-SVG | Copy/save diagram SVG | PNG 2×/4× is the gate; SVG needs a vendored mermaid scene serializer | **gap / optional** |
| S-RECENT | Windows Recent items + jump list | In-app recents; optional XDG recent-manager integration later | **substitute/gap** |
| H-RATE | Store rating prompt | Not applicable on Linux | **n/a** |

Any new gap must be added here and to the checklist with the user's approval. P0 gaps block release.

---

## 5. Phase mapping

| Phase | Parity areas/IDs |
|---|---|
| 1 — Linux shell/open | S-01–S-06, S-12, S-18–S-24, T-16, T-29/30 |
| 2 — Reading | R-01–R-30, M-01–M-10/13/14/21/25, T-01–T-12/26/33/34, N/outline O-01–O-11, F-01–F-18, L-01–L-15, P-01–P-09 |
| 3 — Editing | E-01–E-19/21/24–E-40, M-34, X-02 (selection), P-06/P-19 |
| 4 — Reader completeness | M-11/12/15–M-20/M-22–M-33, V-01–V-18, B/table X-16, T-13–T-15/17–T-25, D-01–D-08, O-12–O-15 |
| 5 — Editor completeness | E-20/E-22/E-23, multi-cursor E-29, spellcheck E-32, format bar/slash |
| 6 — Math | M-15–M-18/M-33, math export |
| 7 — Export/print | X-01–X-15 |
| 8 — Windows/tabs/shell | T-20–T-28/T-31/32/35, W-01–W-10, D-04–D-08 |
| 9 — i18n/a11y/polish | A-01–A-15, I-01–I-11, H-01–H-13, S-26, P-10–P-20 |
| 10 — Packaging/audit | S-07–S-09/S-25, all checklist rows |

Some IDs appear in two phases because their first slice lands earlier and their full behavior later (e.g., editing basics vs format bar).

---

## 6. Definition of done

A behavior row is done when:

1. Its acceptance criterion in `BEHAVIOR_SPEC.md` passes on Linux.
2. The checklist line is `[x]`.
3. Evidence is linked from the checklist or the phase status: unit test, `--snap` golden, `--tree` assertion, scripted interaction, or a manual note with the environment.
4. If it is a substitute/gap, this file says so and the user has accepted it.

At each phase gate:

- Count checklist rows by area/priority.
- Compare against the phase mapping.
- Run the golden/tree/perf gates in `docs/TESTING.md`.
- Do not start the next phase with an unexplained P0 gap.
