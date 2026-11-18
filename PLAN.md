# lmdv-rs — Master Plan

A native Linux-first Markdown viewer/editor in Rust with [Rune](https://github.com/anomalyco/rune).

**Current state:** implementation in progress: P0–P5 complete; P6/P7 in progress. This file is the execution plan; [`AGENTS.md`](AGENTS.md) is the operating contract; [`docs/BEHAVIOR_SPEC.md`](docs/BEHAVIOR_SPEC.md) is the 342-row behavior spec; [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) is the design; [`docs/STATUS.md`](docs/STATUS.md) is the live handoff.

> **Do not start a new phase without the user's explicit go-ahead.** See [`docs/STATUS.md`](docs/STATUS.md) for the current phase and open items.

---

## 1. Product definition

`lmdv` is a fast, offline, native Linux-first Markdown viewer and editor. It opens `.md` files from the file manager, a terminal, drag-and-drop or a folder browser; renders GitHub-flavored Markdown with tables, task lists, code highlighting, native Mermaid diagrams, math, images and callouts; edits with a source editor and live preview; remembers reading positions; exports/prints without a webview.

**Success**

- Every P0 and P1 row in [`docs/PARITY_CHECKLIST.md`](docs/PARITY_CHECKLIST.md) is implemented or has an approved substitute.
- The app is a daily driver on CachyOS: double-click a `.md`, read, edit, save, switch tabs, search, export.
- The `test-features.md` and `commonmark-math.md` fixtures are visual/behavioral regression fixtures.
- No webview, Node runtime, telemetry or network dependency.

**Non-goals for v1**

- A general IDE or a Markdown dialect beyond the specification and GitHub.
- Accounts, cloud sync, collaboration.
- Windows Store/MSIX or Windows shell integrations. Windows must keep compiling; macOS may follow later. Linux remains the primary product.
- Pixel-identical native titlebar on Linux (native frame is the v1 substitute).

---

## 2. Architecture in one page

```text
lmdv            bin: CLI, single-instance socket, app id, window bootstrap
lmdv-app        WindowState, views, commands, routing, overlays, close state machine
lmdv-core       pure domain: IDs, SourceBuffer, Document, registry, prefs, session
lmdv-io         XDG store, atomic writes, load/save/probe, watcher, socket, path policy
lmdv-md         Rune-free parser/model: GFM + footnotes, callouts, front matter, math,
                HTML subset, anchors, TOC, source maps
lmdv-render     Rune renderer for MdDoc + DocTheme + sticky tables + math/images
lmdv-math       RaTeX bridge, metrics, SVG/PNG cache
lmdv-edit       TextEdit sessions, syntax, find/replace, line ops, block mode, drafts
lmdv-export     ExportDoc, self-contained HTML, DOCX OOXML, Typst PDF, offscreen PNG
lmdv-i18n       rune-i18n catalogs and typed keys
lmdv-testkit    fixtures, scenes, tree/golden helpers (dev only)
```

- One UI thread per process; one canonical `Rc<Document>` per file; one editor lease per document.
- Workers never touch app state; they post `JobMsg` to a per-window mailbox and wake the UI.
- No file I/O in handlers or views.
- Rune is vendored whole at the pinned rev; five numbered patches cover inline math, close veto, optional text hit, optional window theme and deferred Linux CSD.
- Markdown is parsed by a fork of Rune's parser into a Rune-free `MdDoc`, then rendered by a fork of Rune's renderer.

Full detail: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md); long-form rationale: [`docs/ARCHITECTURE_DETAIL.md`](docs/ARCHITECTURE_DETAIL.md).

---

## 3. Priority and parity policy

| Priority | Meaning | Release rule |
|---|---|---|
| **P0** | Core reading/opening/tabs/rendering/save/themes. The app is unusable without it. | Must pass. |
| **P1** | The rest of the specified feature set on Linux: editor tools, export, search, i18n, a11y, multi-window, math. | Required for “complete”. |
| **P2** | Niche or platform-specific. | Implement if time; otherwise documented substitute/gap. |

No behavior in [`docs/BEHAVIOR_SPEC.md`](docs/BEHAVIOR_SPEC.md) is silently dropped. A P0 gap blocks release; a P1 gap needs the user's explicit acceptance. The status lives in [`docs/PARITY_CHECKLIST.md`](docs/PARITY_CHECKLIST.md).

---

## 4. Phase plan

Gates are run by the orchestrator, not trusted from a worker. A phase never advances on a red gate.

### P0 — Bootstrap and Rune proof

**Goal:** a buildable workspace that proves the two highest-risk Rune integrations before app work.

Steps:

- [x] `git init`, `.gitignore`, root `Cargo.toml`/`rust-toolchain.toml`/`rustfmt.toml`/`clippy.toml`.
- [x] Vendor the full pinned Rune workspace under `vendor/rune/`; record rev in `vendor/rune/UPSTREAM.md`; repeat Rune's four `[patch.crates-io]` paths.
- [x] Add `scripts/vendor-rune.sh`, `scripts/apply-rune-patches.sh`, `scripts/check-vendor.sh`.
- [x] Patch `0001-text-inline-object` (Span::image + rise) and `0002-close-request-veto`; the proof app draws an inline image on the text baseline; the close-veto tests prove a dirty native close stays open.
- [x] Bootstrap the crate skeleton; `cargo check --workspace` passes.
- [x] Wire the headless snapshot/tree harness; `--snap` and `--tree` both work on the proof scenes.
- [x] Add the reference samples to `tests/fixtures/`; record fixture provenance.
- [x] Install `scripts/install-md-handler.sh` dry-run; the launcher/desktop path is tested end-to-end in a temp HOME.
- [x] `docs/STATUS.md` and OpenChamber plan updated.

**Gate (passed 2026-11-13):** `scripts/run-gates.sh` green; inline-image layout/scene tests pass; close-veto tests pass; dirty Alt+F4 proof; `--snap` PNG and `--tree` JSON produced; vendor rev check passes.

### P1 — Core, IO, viewer shell

**Goal:** open files into a tabbed window with a correct domain/store/socket foundation.

Steps:

- [x] `lmdv-core`: IDs, `SourceBuffer`, `Document`, `DocumentRegistry`, `EditorLease`, prefs/session types, tests.
- [x] `lmdv-io`: XDG/portable paths, atomic store writer, load/save/probe, watcher, CLI parser, single-instance socket with cwd/activation.
- [x] `lmdv-app`: `AppShared`, `WindowState`, per-window mailboxes, command registry, shell, tab strip, start page, open file.
- [x] `lmdv` binary: CLI, instance claim, initial window(s), app id, title.
- [x] First viewer: temporary `rune-kit::markdown` reader (P2 replaces it with the fork), outline/anchors, reading positions, recents.
- [x] Linux handler: desktop entry, MIME, launcher; double-click opens a tab in the running instance.
- [x] Headless scenes for viewer/start page/help.

**Gate (passed 2026-11-13):** 72 app tests, 53 core tests, 68 io tests; second launch routes via the Unix socket; positions persist/restore; dirty close veto; reader tree has heading roles; double-click a `.md` with spaces/unicode; `--snap`/`--tree` both work.

### P2 — Markdown parity

**Goal:** every reading/rendering behavior in the spec.

Steps:

- [ ] `lmdv-md` model/parser: CommonMark + GFM, footnotes, callouts, front matter, anchors/legacy/reserved ids, `[TOC]`, HTML subset, math scan, source maps, find.
- [ ] `lmdv-render`: block/inline renderer, code highlighting, images/srcset/picture, tables/sticky header, footnotes/callouts/TOC, Mermaid, math placement hooks.
- [ ] `lmdv-math`: RaTeX parse/layout/render, SVG/PNG cache, theme/size/scale keys, error fallback.
- [ ] Outline/scroll-spy, section line, wide tables, task checkboxes live in viewer, media, links/link resolution, missing-file/error states.
- [ ] Fixture corpus from `BEHAVIOR_SPEC.md` §9 and the reference samples; golden parser/render/tree tests.
- [ ] Performance: parse 500 KB <80 ms; scroll budget; math/diagram cache.

**Gate:** F-MARKDOWN/F-TABLE/F-MATH fixtures render; `commonmark-math.md` semantics; sticky table; TOC; click/double-click to source; goldens/tree; budgets.

### P3 — Editor and file safety

**Goal:** edit/save/recover with no lost work.

Steps:

- [x] `lmdv-edit` core: `EditorSession` + lease, single-field mode, syntax spans, find/replace, line/formatting commands.
- [x] Format bar, `/` menu, go-to-line UI, find/replace bar and keyboard map. — *P3-03: inline panel (not caret-anchored) and app-side active state; OI-061/081–084.*
- [x] Block-mode UI for large documents + global journal wiring; threshold benchmark. — *P3-04a: virtualized fields, global journal undo, split/merge, Tab indent; SP-03 benchmark still open (OI-064).*
- [x] Drafts, EOL/BOM preservation, save conflict, Save As, own-write suppression, watcher reload/rename/delete, close guards.
- [x] Task-tick write-back with Undo. — *P3-04b: viewer ticks write through a save job, Undo notice, conflict prompt; renderer task-control wire is OI-091.*
- [x] Live preview, last-good preview on render failure, one-way editor→preview sync with interpolation and caret reveal. — *P3-04c2; exact wrapped-line/header geometry OI-100/101.*
- [x] Double-click-to-caret (E-35) and Edit from here (E-04). — *P3-04c1; padding/keyless-text gaps OI-095.*
- [x] Editor headless scenes and scripts.

**Gate:** E-01–E-20, P-04–P-06; save/reload/conflict/draft flows; typing p95 <16 ms at 64 KB and 1 MB block mode; no `std::fs` in `lmdv-app`.

### P4 — Multi-window, tabs, session

**Goal:** the browser-like window model.

Steps:

- [ ] Shared document registry/lease behavior; tab transfer, tear-out, cross-window `TabOffer`/`TabAccepted`, reopen, close others/right. — *P4-01/02a/02b: registry/lease, close guards and in-process transfer/menu done; X11 tear-out, reopen and close others/right remain (OI-114/119).*
- [x] Launch routing; focus order; window cascade; `Ctrl+Shift+N/W`; title/dirty dot; prefs/theme sync across windows. — *P4-01: runtime `open_window`, registry/WakeSet, routing matrix, `Ctrl+Shift+N`, headless/desktop gates; dirty broadcast remains OI-074.*
- [x] Session restore; crash flag; portable mode. — *P4-02d: fragments/debounce, factory restore, clean/crash flag, pasted drafts; optional `--restore-session` remains OI-124.*
- [x] X11 cross-window drag; Wayland fallback menu. — *P4-02c release-only tear-out + transfer; live follow/T-21 substitutes OI-125/126.*

**Gate:** two windows same doc; move tab with undo/lease; close prompts; routing matrix; X11 drag; Wayland no data loss; restore.

### P5 — Export, print, media

**Goal:** everything that leaves the app.

Steps:

- [x] `lmdv-export::ExportDoc` from `MdDoc` + asset resolver.
- [x] Self-contained HTML (semantic tables/code/quotes, embedded fonts/images/math SVG/diagram PNG). — *P5-01: images/math are data URIs; math is 2× PNG, not SVG; fonts stay system.*
- [x] DOCX OOXML (headings/lists/tables/images/code/links; math images first, best-effort OMML). — *P5-02: real Heading1–6 styles, tables, numbering, callouts, images, math PNG with TeX alt, A4; OMML and `footnotes.xml` still open (OI-066/077).*
- [x] PDF via Typst (page model, header/date, page numbers, H1 page breaks, table fitting, math/diagram embedding). — *P5-03: A4, header/footer, H1 breaks, tagged alt text, subset fonts; wide-table fitting (X-12) and PDF/UA remain (OI-085–088).*
- [ ] Print = PDF + system viewer/print; share menu; rich/plain copy; table TSV; PNG 2×/4×; SVG diagram spike. — *P5-04a/b: Share menu, HTML/DOCX/PDF jobs, fonts, Print, whole-document rich copy done; selection copy, TSV, PNG/SVG and Mermaid raster remain.*
- [ ] Export scenes/tests: HTML opens offline, DOCX unzip+XML, PDF text/pages, PNG dims.

**Gate:** X-01–X-16; no network in output; reader theme/size never affects export; PDF page numbers/headers; DOCX opens.

### P6 — Parity extras

**Goal:** the long tail that makes it feel complete.

Steps:

- [ ] File browser (tree, watch, collapse, keyboard), folder search, table preview/filter/sort/freeze, lightbox/diagram zoom/pan, palette, menus, back/forward, help/keys, notices. — *P6-F/T/L/S/C/H: browser/search, table preview/copy, media lightbox, section line/progress, palette and history done; menus, a11y and pane extras remain.*
- [ ] Six locales + help, number/date formats, English command keywords.
- [ ] A11y roles/names/focus/live regions; Orca pass; reduced motion; contrast.
- [ ] Media playback; Linux substitutes for drag-out/always-on-top/titlebar where needed.
- [ ] Spellcheck (Hunspell) and multi-cursor if the patches landed; otherwise documented gaps.
- [ ] Full keyboard matrix.

**Gate:** F/O/C/V/A/I/H rows; Orca; six locales; keyboard-only; no unexplained P0/P1 gaps.

### P7 — Hardening and packaging

**Goal:** installable, reproducible, honest.

Steps:

- [x] Security corpus/fuzz: HTML, paths, image bombs, socket, huge tables/docs. — *P7-SEC parser corpus + multibyte math fix; image/socket fuzz still partial.*
- [ ] Performance budgets on the target machine; memory; idle.
- [ ] Wayland/X11, NVIDIA/Mesa, scaling, IME, multi-window manual matrix.
- [x] Packaging: AppImage and/or Flatpak, AUR PKGBUILD optional, portable tarball; icon/desktop/MIME; install/uninstall restore. — *P7-PKG recipes + icon; signed/end-to-end release remains OI-153.*
- [ ] Full parity audit; every checklist row has evidence or an accepted substitute/gap.
- [ ] README/help/known gaps; close OpenChamber plan.

**Gate:** clean CachyOS install → double-click `.md` → read/edit/export → uninstall restores previous handler; budgets; security; parity audit.

---

## 5. Dependency graph

```text
P0 Bootstrap/Rune proof
 ├─> P1 Core/IO/viewer shell
 │    ├─> P2 Markdown parity
 │    │    ├─> P3 Editor/file safety
 │    │    │    ├─> P4 Multi-window/session
 │    │    │    ├─> P5 Export/print
 │    │    │    └─> P6 Parity extras
 │    │    └─> P6 (reader extras)
 │    └─> P4 (window shell)
 └─> P7 Hardening/packaging (after all)
```

Parallelism is allowed only after an interface freeze: P1 core/IO/app shell; P2 parser/renderer/math; P3 editor/file-safety; P5 HTML/DOCX/PDF. Never parallelize shared manifests or domain types.

---

## 6. First five tasks (after go-ahead)

| # | Task | Owner | Produces |
|---|---|---|---|
| 1 | P0-01 vendor Rune, root manifests, `[patch]`, patch 0001/0002, proof app | orchestrator + Rune patch worker | buildable workspace + `--snap` + inline math/close-veto proof |
| 2 | P0-02 `lmdv-core` IDs, `SourceBuffer`, `Document`, registry, tests | core worker | frozen domain API |
| 3 | P1-01 `lmdv-io`: XDG/store/atomic/load/save/socket/CLI | io worker | open/save/launch primitives |
| 4 | P1-02 `lmdv-app` skeleton: `AppShared`, `WindowState`, mailboxes, commands, shell/tabs/start | app worker | visible window, tabs, open file |
| 5 | P1-03 viewer integration + outline/anchors/scenes + P1 gate | app/render worker | first end-to-end viewer |

The five after the gate: parser model, renderer base, math service, editor, file safety.

---

## 7. Orchestration summary

- The main agent is the orchestrator. It freezes interfaces, owns shared manifests/docs/wiring, spawns workers, reads every diff, reruns every gate, re-prompts with exact defects, or implements the fix itself.
- **One file, one owner.** No two workers edit the same file. Shared-file changes are orchestrator-owned.
- Every worker gets a context capsule (phase, gate, existing types, frozen signatures, spec `file:line`, known failures, do-not-touch, next).
- Research is read-only; reviewers are read-only; workers report commands/evidence.
- A phase never advances on “the worker says it passes”.
- Full contract: [`AGENTS.md`](AGENTS.md) §5.

---

## 8. Top risks

1. **Inline math** needs a Rune text patch; `Span::image` does not exist at the pinned rev. P0 proves it.
2. **Whole-document editing** is 12–46 ms/keystroke at 50–200 KB; two-mode editor is mandatory.
3. **Rune is pre-release/vendored**; five patches must be re-applied on every rev bump.
4. **Shared buffers across windows** need a lease/undo/close state machine.
5. **Markdown fork drift**; provenance/diff + fixture corpus.
6. **Close veto** is impossible unpatched.
7. **Linux launch/focus**: own socket; Wayland activation restrictions.
8. **Export beyond Rune**: Typst/PDF, DOCX/OMML, math assets.
9. **Cross-window tab drag on Wayland** has no global coordinates/capture; fallback.
10. **Security/robustness**: HTML, paths, image bombs, watcher races, huge docs.

Details/spikes: [`docs/RISKS_AND_SPIKES.md`](docs/RISKS_AND_SPIKES.md).

---

## 9. Definition of “ready to start”

- [x] Behavior spec (342 rows) copied and referenced.
- [x] Parity checklist generated.
- [x] Architecture/crate/domain/ownership frozen in docs.
- [x] Rune capability map and patch set recorded.
- [x] Linux integration design + tested install/uninstall scripts in `scripts/`.
- [x] Risks/spikes and test gates documented.
- [x] OpenChamber plan/notes/todos mirrored.
- [ ] **User's explicit go-ahead.**

When the user says go, start P0-01. Do not skip P0 to “get something on screen”.
