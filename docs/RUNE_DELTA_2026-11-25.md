# Rune upstream delta — 2026-11-25

Research against the pin in [`RUNE_PIN.md`](RUNE_PIN.md). Supersedes
[`RUNE_DELTA_2026-11-22.md`](RUNE_DELTA_2026-11-22.md).

Two independent read-only probes were run: a source survey of the new APIs
against our open rows, and a measured patch-port probe (`git apply --check`,
reject counts, moved-file mapping). No build was run for this report; every
API claim below was read in the upstream source at the analysed head.

| | |
|---|---|
| Pinned | `cca5df6fafb4721d96fe105f126ebfd2ec351bb6` (2026-11-17) |
| Last researched | `63ad0209edc9a3a7d2208b7f6c66f9e353da315d` (2026-11-22) |
| Analysed head | `f4ec14ed32256ae0c36e73476825f9245437de5b` (2026-11-25 16:07 UTC) |
| Live head at finish | `c94de5574743478f19b87c3ee1c6e4b40d234d3d` (16:22 UTC) — 5 commits later |
| Branch | `dev` is the default branch; there is no `main` (`production` is separate) |
| Commits since pin | **927** at `f4ec14e`, **932** at `c94de55` |
| Commits since last head | **764** at `f4ec14e`, **769** at `c94de55` (0 merges) |
| Churn since last head | 2,644 files, **+427,349 / −16,546** |
| Toolchain | unchanged: Rust **1.99.1**, edition 2024, resolver 3 |

The cadence is now roughly **250 commits/day**, up from ~33/day when the
2026-11-22 file was written. The head moved five commits during the probe.
Pin the target rev at the start of any work; "current" is not a stable target.

## What changed since 2026-11-22

Only lmdv-relevant items. The full 764-commit survey is in the research
notes; commit subjects are the source unless a file is cited.

### Text, layout and editing

- **Inline child elements**: `El::inline(at, InlineBox, child)`
  (`crates/rune/src/el/pills.rs:207`), `InlineBox`/`InlineAlign`/
  `measure_inline` (`crates/rune-text/src/layout/span.rs:231-314`). An atomic
  inline box with a real baseline; the child keeps paint, hit test, focus and
  an a11y node. This is the inline-picture wrapper V-02 lacked.
- **Key preview**: `El::on_key_preview`, outermost-first and before the
  focused field (`el/focus.rs:185`, `runtime/input/keys.rs:439-460`). Return
  `true` and the field never sees the key. This is the lever for Alt+arrows
  and for palette chords during IME composition.
- **IME state**: `TextEdit::is_composing()` and the preedit accessors
  (`rune-edit/src/lib.rs:630`). `KeyEvent` still carries only key+mods, so
  C-01 is closable app-side, not by a `KeyEvent` flag.
- **Editor geometry**: `Cx::field_selection_rects`, `text_offset_at`,
  `Tx::text_line`, `on_input_selection`, `on_input_copy`,
  `TextEdit::selected_with_pills`, `reveal_caret_on_focus`, enforced
  `read_only`, `line_home_end`, `Mark::Color`.
- **Fonts**: runtime font registration (`add_font`, `with_fonts`,
  `installed_families`), and the mono slot now defaults to `ui-monospace`.
- **Performance**: link/decoration hit-testing now lays out only candidates
  under the pointer; spans and glyph-cache indices widened to `u32`.

### SVG, images and inline layout

- **`Path::from_svg`** (full SVG 2 path data), `Shape::fill_svg`,
  `fill_svg_with`, `stroke_svg` (`crates/rune-core/src/shape.rs:228,243,258,
  426`), compound fills with `FillRule::{NonZero, EvenOdd}`, `Area` and
  `Path::polygon` (`shape.rs:504`), `El::shape` (`el/visuals.rs:203`).
  Together with `El::inline`, this is the L-12 vector-shape unlock.
- Image: `Pixels::with_rgba`, scaled decode, `intrinsic_size`, tint/nearest,
  ICC → sRGB, copy-image APIs, atlas residency fixes.

### Tabs, windows and `rune-kit`

- **Close veto is upstream**: `CloseResponse::{Close, Defer}`,
  `App::on_close_request` (`app.rs:65,914`), host routing in
  `window/host/lifecycle.rs`. Patch 0002 is obsolete.
- `QuitResponse`/`App::on_quit_request`; `on_quit` now runs when the last
  main window closes; Cmd+W goes through the close gate.
- `TabStrip::breaks` (drag stop elements) — additive only. **No** per-tab
  width, Saved status or slop change.
- `SegmentedControl::as_tabs()` gives a ready `tablist`/`tab` + `controls` +
  `description` pattern; `SidebarToggle::labels` localizes the toggle.
- `MarkdownStyle::prose_width`, `inline_code_text`, block margins,
  **`code_block_radius` default 6.0**, `wrap_anywhere`, `on_link_hover`,
  custom emoji.
- `Canvas`/`ZoomPan` preview, `ScrollSpec::stick_bottom`, `stop_scroll`,
  `VirtualItems`, `ScrollSource`, same-frame `on_visible_rows`.

### Input, a11y and platform

- `on_key_preview`, drag-cancel Up, `Tx::cancel_pointer`,
  `Tx::copy_image_then`, `clipboard::revision()`.
- A11y: `El::controls`, `owns`, `aria_disabled`, `multiselectable`,
  `required`, `has_popup`, `orientation`, `auto_complete`, `on_a11y_action`,
  `on_set_value`, `live_range_value`, per-level `size_of_set`.
- Windows: posted-key/dead-key/AltGr fixes, titlebar drag, DPI, UIA focus,
  DX12/DirectComposition fallback, vblank pacing, `Titlebar::controls(false)`.
- Linux: non-blocking portal at launch, CPU-first frame → GPU handover,
  Vulkan ICD/adapter fallback, layer-shell IME/keys, native file drops,
  activation tokens, reduced motion, `org.freedesktop.Application`.
- `Tx::open_path`/`reveal_path` is now public (`ShellExecuteW`, portal,
  `NSWorkspace`); single instance stays crate-private; no file-identity API.

## New APIs vs the open gaps

| Gap | At `f4ec14e` | Evidence | What closes it |
|---|---|---|---|
| **T-01** flexible toolbar centre, per-name tab width | Still missing | `Toolbar::center` is an out-of-flow row pinned to the band (`shell/toolbar.rs:38-42`); tabs share width equally; no public per-tab width | Upstream kit patch (~0.5–1 d) |
| **T-03** Saved status in the close slot | Still missing | `TabItemStatus` is still `Busy/Attention/Unread/Failed` (`shell/tab_strip.rs:101-110`) | Kit patch (~2 h) or keep the app-side flash |
| **O-14** browser back/forward keys; Alt+arrows | Partly | No `Key` browser variants; mouse side buttons exist. Alt+Left is still word-left in a field, but `on_key_preview` can consume it first | Keys: our Rune patch (~1–2 h). Alt+arrows: app-side. Fragment: app-side (`lmdv-io/src/launch.rs` has no fragment) |
| **C-01** palette during IME composition | Equivalent exists | `TextEdit::is_composing()` (`rune-edit/src/lib.rs:630`); runtime gates Enter/Escape only | App-side gate on the focused field, or `on_key_preview` |
| **L-11** middle-drag link suppression; fragment | Link: missing. Fragment: app-side | `link_up` activates on release over the same link with no press-origin check (`runtime/selection/links.rs:439-445`) | Rune patch recording the press point (~2 h); fragment in `Launch` |
| **D-04** drag slop 5 pt | Still missing | `const SLOP: f32 = 4.0` (`shell/tab_strip/drag.rs:12`) | Kit patch (~1 h) or accept 4 pt |
| **I-10** pane grip labels | Grip missing; toggle fixed | Grip still hardcodes `"Resize sidebar"`/`"Resize inspector"` (`shell/split.rs:733`); `SidebarToggle::labels` exists | Kit patch (~1 h) |
| **A-05** tablist/tab ARIA | Partly closable app-side | `role("tablist")`/`role("tab")` exist; `TabItem::decorate` + `El::description`/`controls` (`el/a11y.rs:247,505`) let the app attach reading time and `aria-controls` | App-side |
| **A-06** tree ARIA | Partly closable app-side | `position_in_set` (`el/a11y.rs:562`) and `owns` (`:525`) exist; no `aria-current` setter | App-side (own tree in `browser.rs`), `aria-current` via label or a small patch |
| **D-08** drag-cancel signal | Mostly provided | `Drag::cancelled()`; Escape/pointer-cancel ends with a cancelled Up (`#1536`); `Tx::cancel_pointer` | App-side assertions; kit callback only if cross-window handoff needs it |
| **E-29** multi-cursor | Still missing | `TextEdit` has one anchor/focus pair; no multi-caret paint/hit/edit API | Large Rune patch or fork (L), or accept |
| **V-02** pinch; inline hover hint | Pinch provided; hint app-side | `El::on_magnify` + `MagnifyEvent` (`input/canvas_input.rs:72-85`); `El::on_link_hover`, `El::inline` | App-side wiring (OI-182) |
| **L-12 / OI-195** SVG vector shapes | **Now provided** | `Path::from_svg`, `Shape::fill_svg`/`stroke_svg`, `Area`, `El::shape`, `El::inline` | App-side shape mapping (~1–2 d); labelled-group substitute no longer needed |
| **OI-155** Windows identity/instance/open | Partly | `Tx::open_path`/`reveal_path` public; single instance crate-private; no volume-serial/file-index API | Keep lmdv's mutex/pipe and `GetFileInformationByHandle`; optionally adopt `open_path` |

**Net effect on P0:** the bump closes **none** of T-01, T-03, D-04, I-10 or
the browser-key half of O-14 by itself. It makes O-14's Alt+arrows, C-01,
A-05/A-06, D-08 and V-02 closable app-side, and it unlocks L-12. Patch 0002
disappears. E-29 remains a large patch or an accepted gap.

## Our patches at the new head

Measured with `git apply --check --verbose` on a clean `f4ec14e` tree, and
with the vendor pipeline's `rustfmt` baseline where noted. The control: on a
rustfmt-formatted **pin** tree all four original patches apply cleanly, so
the failures below are moved/changed code, not formatting.

| Patch | Applies | Failing hunks | Rework | Effort |
|---|---|---|---|---|
| **0001** inline objects | no | 17/19 | `rune-text/src/layout/span.rs` fields + `hash_layout`/`affects_layout`; `layout/shape.rs`; `pill_chips` walker; paint loop; **new box tag bit**; two `[[test]]` entries | 10–13 h |
| **0002** close veto | no | — | **Obsolete**: upstream `CloseResponse`; delete patch + test, migrate one app function | 0.5–1 h |
| **0003** a11y labels | no | 13/21 (8 apply) | `Line.spoken` in `a11y/text/runs.rs`; `a11y_spoken_text`; call sites; label hash in `node.rs` | 2.5–3.5 h |
| **0004** highlight box | no | 2/2 | re-anchor `range_boxes` in `rune-text/src/layout/cursor.rs`; new vertical/pill branches | 1–1.5 h |
| **0005** Mermaid SVG | no | 8/11 | target files moved into `canvas/`; make `TITLE_GAP`/`title_font` crate-visible; un-gate `Scene.ops`; `Drawing` scene/style; rune-kit re-export | 2.5–4 h |

### 0001 traps

- `IMAGE_BOX = 1 << 61` now collides with upstream `SPAN_BOX`
  (`rune-text/src/layout/span.rs:209`); `1 << 60` is `INLINE_BOX`. Use
  `1 << 59`.
- Layout hashing is gated by `Span::affects_layout`
  (`span.rs:65`, `layout/hash.rs:41`, `paragraphs/keying.rs:65`). Add
  `image.is_some() || rise != 0.0` there as well as in `hash_layout`, or a
  changed formula silently reuses a cached layout.
- Paint now collects pills/images at `runtime/paint/content.rs:532-556` and
  draws through `ImageCache::place_at`, with visibility and wake-time
  handling. The old paint hunk cannot be applied; the walker must be rebuilt
  beside `pill_chips`.
- Do not adopt upstream's element-level `El::inline_image` for formulas: it
  rounds width/ascent/descent to whole device pixels
  (`rune-text/src/layout/shape.rs:108`) and has no `Span::rise`. Keep 0001.

### 0002 removal

Upstream has the same semantics we patched, plus programmatic-close and
second-request tests. `crates/lmdv-app/src/app.rs:356` already registers
`close_request`; change its return type from `bool` to `CloseResponse` and
return `Defer` where it returned `true`, then delete
`vendor/rune/patches/0002-*`, `tests/close_request_veto.rs` and the script
line.

## What the bump buys

Even without closing a P0, the bump is worth doing:

- New app-side routes for L-12, A-05/A-06, C-01, O-14's Alt+arrows, V-02,
  D-08 (all cheaper than waiting for kit work that is not happening).
- Close veto replaces a patch; `on_quit_request` adds correct quit handling.
- Linux launch/first-frame/GPU fallback, native drops, layer-shell IME/keys,
  activation tokens, reduced motion.
- Window input correctness on Windows; UIA focus; titlebar drag.
- Large-document hit-testing cost (link/pill/decor), wider span limits.
- Headless `--settle`, richer `--tree`, p99 summary.
- Font registration, `MarkdownStyle` controls, `El::inline`, `on_key_preview`.

## Behavior changes to expect

1. `App::on_close_request` returns `CloseResponse`; `on_quit` now runs when
   the last main window closes.
2. `MarkdownStyle::code_block_radius` defaults to **6.0**; set `0.0` or
   re-baseline the goldens.
3. Mono now defaults to `ui-monospace` and loads lazily; code metrics may
   shift.
4. Word movement and Windows double-click boundaries changed; editor
   selection tests and goldens may move.
5. `TextSpec` is back to 224 bytes; check literal construction.
6. `on_drag` no longer fires `on_press` after its own slop; a held drag ends
   with a cancelled Up when the element goes. Re-run tab-strip tests.
7. Upstream publishes a sibling `Role::Image` node for *labelled inline
   icons*, not for `InlineImage`. Do not speak both it and our text-value
   patch for the same formula.
8. `El::w_fit`/`self_auto` and taffy fixes can shift layout by a point or
   two; keep snapshot tolerance.

## Build and vendor

- The root `[patch.crates-io]` grows from 5 to **8** entries: add `naga`,
  `sctk-adwaita`, `accesskit_atspi_common`; refresh the five old vendored
  crates. `vendor/` grows by ~132k lines, mostly naga.
- `tiny-skia-path` moved to `crates/rune-text` as package
  `rune-tiny-skia-path`; `scripts/check-vendor.sh` hard-codes the old path
  and fails until fixed.
- `rune` default features now include `hunspell`. lmdv builds with
  `default-features = false`, so `spellbook` stays out; do not enable
  defaults.
- `rune-luau` needs a C++ compiler, but lmdv does not build it.
- `crates/rune` and `crates/rune-kit` keep `autotests = false`; re-ported
  test files need explicit `[[test]]` entries.

## Port order and estimate

| # | Work | Depends on | Hours |
|---|---|---|---|
| 1 | Re-vendor `f4ec14e`; add three patch entries; refresh five crates; fix `check-vendor.sh`; update `RUNE_PIN`/`UPSTREAM`; unpatched `cargo check -p rune -p rune-kit` | — | 2–4 |
| 2 | Fix app-side API drift; migrate `close_request`; delete 0002 and its test | 1 | 3–7 |
| 3 | **0001** span fields, tag bit, `affects_layout`/`hash_layout`, shaping, paint, tests | 1 | 10–13 |
| 4 | **0004** `range_boxes` and tests | 3 | 1–1.5 |
| 5 | **0003** `runs.rs`, `node.rs`, tests | 3 (label field) | 2.5–3.5 |
| 6 | **0005** modular `rune-mermaid` port + tests | 1 (parallel to 3) | 2.5–4 |
| 7 | Goldens/trees/scenes, regenerate patches, update docs/OI-189 | 2–6 | 4–6 |

Track A (0001 → 0004 → 0003) and track B (0005) touch disjoint crates and can
run in parallel. 0002 is orchestrator work.

**Total: 25–39 h ≈ 3–5 focused days; plan 4, with a 5th if the first
workspace `cargo check` finds real app API drift.** The earlier 2.5–3 day
figure excluded that drift and under-budgeted 0005.

## Recommendation

**Bump to a pinned `f4ec14e` (or the head at the moment work starts) before
re-porting the P0 patches.** Patch 0002 disappears, the other four must be
re-ported anyway, and the delta only grows at ~250 commits/day. Port once,
regenerate the patches against the chosen rev, and stop tracking head.

After the P0 gate, the same bump makes L-12 (SVG vectors), A-05/A-06,
C-01, O-14's Alt+arrows, V-02 and D-08 app-side work rather than Rune
patches. E-29 stays a large patch or an accepted gap; T-01, T-03, D-04 and
I-10 still need kit patches.

Stop conditions: do not enable `rune-kit` `math` (keep `lmdv-math`), do not
adopt kit `Markdown`, do not enable `rune` defaults. If the `naga` patch
cannot be vendored, stop — shader validation failures are not debuggable
from the app side.
