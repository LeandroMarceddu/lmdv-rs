# Risks and spikes

Reviewed at every phase gate. A risk with no owner is not managed.

**Likelihood/Impact:** L/M/H. **Status:** open / mitigated / closed / accepted.

## 1. Risk register

| ID | Risk | L | I | Mitigation | Trigger / early warning | Status |
|---|---|---|---|---|---|---|
| RK-01 | **Inline math needs a Rune text patch**; no `Span::image` at the pinned rev, only a square `InlineIcon`. | H | H | P0 patch 0001 (`Span::image` + `Span::rise` + inline-box geometry); baseline test at 1×/2×; fallback is block-only with a documented gap, never a square formula. | Formula sits >0.5 pt off, wrong aspect, wraps badly | open |
| RK-02 | **Whole-document editing is slow**: ~12 ms/keystroke at 50 KB, ~46 ms at 200 KB. | H | H | Two-mode editor (single field ≤64 KiB/1500 lines, block mode above); measured threshold; global undo journal; typing p95 gate. | Typing p95 >16 ms at 64 KB or 1 MB | open |
| RK-03 | **Rune is pre-release and vendored**; a rev bump re-applies patches 0001/0003/0004/0005 and can break APIs. | H | M | Pin rev; `.patch` files; vendor checksum; bump runbook; goldens on every bump. | Patch conflict or API break on bump | open |
| RK-04 | **Shared document buffers across windows** create lease/undo/close races. | M | H | One canonical `Rc<Document>`; lease state machine; explicit takeover; close prompts; race tests. | Lost edit or stale view in a two-window script | open |
| RK-05 | **Markdown fork drifts** from upstream and accumulates bugs. | M | H | Fork only the module; provenance/diff log; reference fixtures; one owner (`lmdv-md`); fuzz no panic. | Fixture regression with no upstream change | open |
| RK-06 | **Native close has no veto**; unsaved-close confirmation impossible unpatched. | M | H | Upstream `on_close_request -> CloseResponse` since Rune `f4ec14e` (no local patch); programmatic close stays force-close; test Alt+F4/WM close. | Dirty window closes without prompt | mitigated |
| RK-07 | **Linux launch/focus**: Rune's single instance lacks cwd/paths; Wayland activation restrictions. | M | M | Own socket with cwd/line/activation token; route on UI thread; test second process; X11/Wayland focus. | Second launch opens wrong tab or no focus | open |
| RK-08 | **Export beyond Rune**: PDF/DOCX/math/pagination. | H | H | Shared `ExportDoc`; Typst PDF; OOXML DOCX; RaTeX images + best-effort OMML; PNG offscreen; goldens. | Clipped PDF, broken DOCX, missing math | open |
| RK-09 | **Cross-window tab drag on Wayland** has no global coordinates/capture. | H | M | X11 tear-out/drop; Wayland menu fallback; no data loss; document substitute. | Drop test misses windows or duplicates tabs | open |
| RK-10 | **Security/robustness**: HTML, paths, image bombs, watcher races, huge docs. | M | H | Allowlist, path policy, size caps, generation guards, security corpus/fuzz. | File outside root read, freeze on huge input | open |
| RK-11 | **Custom Linux titlebar** unsupported; borderless has no edge resize. | H | L | Native frame v1; themed toolbar; optional CSD patch. | User rejects native frame | accepted |
| RK-12 | **Wayland always-on-top** is a no-op. | H | L | Disable with notice on Wayland; X11 works. | Pin button does nothing | accepted |
| RK-13 | **Linux video/audio** is unsupported by Rune (`Player` fails). | H | M | Show a placeholder + “Open externally”; document `L-MEDIA` substitute. | Media fixture shows an error | accepted |
| RK-14 | **DOCX native equations** (OMML) are a large side quest. | M | L | Math images first; best-effort OMML; document gap. | Word shows images, not editable equations | open |
| RK-15 | **SVG diagram export** needs a vendored scene serializer. | M | L | PNG 2×/4× is the gate; SVG optional; vendor `rune-mermaid` + serializer if needed. | Menu missing or approximate text | open |
| RK-16 | **Mermaid subset**: 17 families, not every v11 type. | M | L | Code-fence fallback + notice; track missing types. | Sample diagram renders as code | accepted |
| RK-17 | **Fonts/CJK/emoji/serif** need bundling/fallback. | M | M | Bundle sans/serif/mono; Rune system fallback for CJK/emoji; test zh-CN/emoji/RTL. | Tofu or wrong serif | open |
| RK-18 | **Accessibility** of custom widgets/editor. | M | M | Roles/labels/focus/live regions; `--tree` assertions; Orca pass; canvas editor reports text/caret manually. | Unnamed button/tree or focus trap | open |
| RK-19 | **Single-instance socket races/stale files**. | M | M | `flock`; 0600 in 0700 dir; `SO_PEERCRED`; connect-before-unlink; kill -9 tests. | Launch hangs or two instances | open |
| RK-20 | **Scope creep** (342 behaviors, 152 P0, 155 P1, 35 P2). | H | M | Parity IDs, phase gates, P0/P1/P2 policy, explicit substitutes/gaps. | “While we're here” changes | open |
| RK-21 | **Rune prefs/store** rewrite synchronously; using it in hot paths stalls. | M | M | Own store thread (ADR-010); no `Prefs::set` in handlers; no file I/O in UI. | Frame hitch on scroll/edit | open |
| RK-22 | **Typst build time/binary size** may hurt iteration. | M | M | Feature-gate PDF; split helper binary if needed; measure P0/P5. | `cargo check` slows unacceptably | open |
| RK-23 | **Clipboard/rich copy** differs on Linux (no RTF). | L | L | Use text+HTML; document paste targets; test LibreOffice/Word. | Formatting lost in a target | accepted |
| RK-24 | **RTL/bidi UI** is not automatic in Rune. | M | L | Rune text is bidi; shell rows need `.dir(Rtl)`; defer full RTL UI to P6/P7. | Arabic/Hebrew UI mirrors wrong | open |

## 2. Spikes

Each spike is timeboxed, produces a written result, and ends with **go / no-go / fallback**. A spike may create a scratch crate under `spikes/` but must not become production code without review.

### SP-01 — Inline math via Rune text patch

**Question:** Can patch 0001 place a non-square RaTeX formula inline with correct baseline/ascent/descent, wrapping and RTL, at 1×/2×?

**Timebox:** 1–2 days (P0-01).

**Do:**

- Add `Span::image(InlineImage)` + `Span::rise(f32)` with a U+FFFC placeholder; extend inline-box geometry in `text.rs`, `text/span.rs`, `text/pill_chips.rs`, `runtime/paint/content.rs`.
- Render 10 `commonmark-math.md` formulas with RaTeX; embed inline; compare baseline to surrounding text at 14/16/20 pt, 1×/2×.
- Verify selection/copy skips the formula (or copies the TeX source), wrap-whole behavior, RTL, theme color, transparency, cache.
- Measure first/cached render; binary size.

**Acceptance:** baseline within 0.5 pt; correct aspect; no line-height jumps; selection behavior documented; 200 cached formulas do not stall a scroll frame.

**Fallback:** block math only; record an inline `gap` and do not claim parity.

### SP-02 — Native close veto

**Question:** Can a Rune window ask Save/Discard/Cancel before closing, including WM close and Alt+F4?

**Timebox:** 0.5 day (P0-01).

**Do:**

- Use upstream `App::on_close_request -> CloseResponse` (Rune `f4ec14e`); return `Defer` while dirty and keep programmatic close force-close.
- Test dirty tab, multiple dirty tabs, Cancel, Save failure, window manager close, Alt+F4, last window.

**Acceptance:** dirty window stays open on Cancel; Save writes and closes; no zombie window after quit.

**Fallback:** none for parity; a native close guard is mandatory.

### SP-03 — Two-mode editor and undo journal

**Question:** Does block mode keep typing under 16 ms p95 at 1 MB, with a global undo that spans blocks?

**Timebox:** 1 day (P3).

**Do:**

- Implement `EditorSession` with single-field mode and block mode; block = blank-line separated, virtualized.
- Global journal: record `(block index, TextEdit undo)` or snapshot deltas; `Ctrl+Z` across blocks.
- Test typing at 64 KB/1 MB, Return/Backspace, paste, IME, selection, focus movement, cross-block find.
- Measure p95 commit and memory.

**Acceptance:** typing p95 <16 ms at 64 KB and 1 MB; undo spans blocks; no lost caret on parse; threshold auto-switch stable.

**Fallback:** single-field with a lower threshold + “large file read-only” notice; record the gap.

### SP-04 — PDF/Typst and DOCX OOXML

**Question:** Can `ExportDoc` produce a correct PDF with Typst and a DOCX that opens in LibreOffice/Word, at acceptable cost?

**Timebox:** 1–2 days (P5).

**Do:**

- Build `ExportDoc` for `test-features.md`, a math/diagram sample and a wide table.
- Typst: A4, margins, header/date, page numbers, H1 breaks, table scaling, embedded math/diagram SVG/PNG; measure compile time/binary size.
- DOCX: headings/lists/tables/images/code/links; math as image first, OMML best-effort; unzip/XML tests; open in LibreOffice.

**Acceptance:** PDF no clipped tables, correct pages/header; DOCX opens and preserves structure; compile <5 s for 50 pages; binary delta acceptable.

**Fallback:** HTML + system browser print; DOCX image-only math; record gaps.

### SP-05 — Cross-window tab drag and Linux frame

**Question:** Can a torn-out tab be dropped on another window on X11, and how bad is the native frame?

**Timebox:** 1 day (P4).

**Do:**

- Read `TabTearOut`/`TabTearMove` screen points and `WindowHandle::placement()`.
- Shared window registry; hit-test pointer; `TabOffer`/`TabAccepted`; drop hint; undo/lease preserved.
- Test tear-out, drop, cancel, dirty duplicate, same-file merge, X11 and Wayland.
- Snapshot native frame + themed toolbar vs the specification.

**Acceptance:** X11 drop works with no data loss; tear-out creates a window; Wayland fallback menu works; native frame accepted.

**Fallback:** no cross-window drop; menu “Move to new window”; native frame only.

### SP-06 — Linux handler, launcher, socket

**Question:** Does the tested install/launcher/socket design work for double-click, debug/release switching, paths with spaces/unicode, second instance and uninstall?

**Timebox:** 0.5 day (already largely tested by the design report).

**Do:**

- Run `scripts/install-md-handler.sh --dev --dry-run`, then `--dev`; `xdg-mime query default`; `gio open` a test file; second launch with `--line`; kill -9; uninstall restore.
- Test KDE/GNOME/Thunar where available; check `desktop-file-validate`.
- Confirm the wrapper picks the newest debug/release and logs to `$XDG_STATE_HOME/lmdv/launch.log`.

**Acceptance:** all launch paths reach the intended instance; no stale socket; uninstall restores previous handler; debug/release switch works.

**Fallback:** always start a new process; document the loss of single-instance routing.

### SP-07 — Markdown fork extension surface

**Question:** How localized can the parser/renderer fork be while adding footnotes, callouts, front matter, anchors, TOC, HTML, math and sticky tables?

**Timebox:** 1 day (P2).

**Do:**

- Vendor the parser/renderer with provenance; add one custom block (callout) and one inline object (math placeholder) end-to-end.
- Run upstream `rune-kit` markdown tests against the fork.
- Measure diff size and source-map stability.

**Acceptance:** upstream tests pass; diff localized and documented; no private API leakage; source maps survive edits.

**Fallback:** pre/post-process where possible; fork only the parser or only the renderer.

### SP-08 — Vendor/patch runbook

**Question:** Can a Rune rev bump be mechanical and verifiable?

**Timebox:** 0.5 day (P0).

**Do:**

- `scripts/vendor-rune.sh` copies the pinned workspace; `scripts/check-vendor.sh` verifies checksums; `scripts/apply-rune-patches.sh` applies 0001–0005.
- Simulate a bump to a newer rev: re-apply patches, run goldens/tree, record failures.

**Acceptance:** bump runbook passes on a clean checkout; failures are explicit.

**Fallback:** stay on the pinned rev; patch only the needed files.

### SP-09 — Performance baseline

**Question:** What are the real budgets for the README's worst cases on the target machine?

**Timebox:** 0.5 day after P2/P3.

**Do:**

- Measure launch, open 100 KB/1 MB/5 MB, parse, typing single/block, scroll 2k blocks, 10k-row table, 200 Mermaid, 200 formulas, 100 images, HTML/PDF export.
- Record CPU p50/p95/max, memory, idle frames.

**Acceptance:** budgets in `docs/TESTING.md` §9 hold or have a documented exception; no unbounded growth.

**Fallback:** cap rendering/virtualize/defer; show a notice for oversized files.

## 3. Risk review checklist

- [ ] Any risk changed likelihood/impact?
- [ ] Any spike result invalidates an ADR?
- [ ] Any new dependency with a security/portability cost?
- [ ] Any parity row downgraded without the user's approval?
- [ ] Are the top three risks still the top three?
