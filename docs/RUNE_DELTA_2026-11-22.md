# Rune upstream delta — 2026-11-22

> Superseded by [`RUNE_DELTA_2026-11-25.md`](RUNE_DELTA_2026-11-25.md).
> The upstream head moved 764 commits in three days; this file is kept for
> the patch mapping as it stood at `63ad0209`.

Research against the pin in [`RUNE_PIN.md`](RUNE_PIN.md). Supersedes
[`RUNE_DELTA_2026-11-20.md`](RUNE_DELTA_2026-11-20.md).

| | |
|---|---|
| Pinned | `cca5df6fafb4721d96fe105f126ebfd2ec351bb6` (2026-11-17) |
| Last researched | `7a707454fbfb8d3fd200723c65778fb7b98fd31e` (2026-11-20) |
| Upstream head | `63ad0209edc9a3a7d2208b7f6c66f9e353da315d` (2026-11-22 19:05 UTC) |
| Branch | `dev`, which is also the default branch (`production` is an ancestor) |
| Commits since pin | **163** |
| Commits since last head | **33** |
| Churn since last head | 198 files, +7869/−541 (~1.4k lines are the Windows winit patch) |
| Toolchain | unchanged, 1.99.1 |

## What changed since 2026-11-20

The structural refactor is still the dominant delta (see the 2026-11-20 file);
these 33 commits are mostly behavior and kit work on top of it.

### Math

- `b8470260` — inline math on the text baseline, plus `El::inline_image` and
  `rune_text::Replace`.
- `dd672fc8` — inline math found in one pass over Markdown text.
- `16418bbc`, `8b877aa2`, `b39e1b4d` — optional kit display math: sandboxed
  wasm rendering, top-edge placement, and wakes/rebuilds only the rows a
  finished render changes.

### Text, layout and kit look

- `af4d9cda` — native code-block soft wrapping (`MarkdownStyle::wrap_code`,
  `Markdown::code_wrap`).
- `ff1fe4bc` — paint copies text nodes whose word reveal caught up.
- `86b903de` — exact 1× hairline alpha, CSS shadow layer order, tabular list
  markers, and `TextHighlight` radius/vertical padding.
- `44920b1f` — `rune::hairline` / `El::hairline` sub-pixel dividers.
- `21e5342b` — unconditional 16 pt linear fade on emphasized/hovered tab
  titles.
- `4bd55ea5` — Windows grayscale text weight follows Chromium.

### Focus and kit state

- `935d040a` — `Kit::forced` keeps field rest/focus/invalid apart in a
  forced-colors theme.
- `6e200e63` — `El::focus_ring_for` draws a ring on an overlay.
- `8283dca5` — focus `Entry` preserved across commits while the node stays.
- `4c72ab57` — anchored-overlay suppression, raised-anchor lift, RTL
  `help_side`.

### Scroll

- `7286a3c5` — `Tx::scroll_key_center_at`, device-pixel-snapped, range-centered
  placement.
- `f05046ac` — explicit piecewise-linear `ScrollFade` stops (max three interior
  stops, one curve per window).
- `c6df83fe`, `14b59a34` — scrollbar reveal on overflow and post-scroll hold.
- `b30fa169` — sticky fade bottom snapped to the pixel grid.

### Images and performance

- `4a10c707` — image decoders run on the shared task threads.
- `cf7b4bf4` — SVG nesting capped at 512, 8 MiB decoder stack.
- `8d81d770` — mounted nodes looked up in a dense table bounded by tree size.

### Windows

- `818de1d2`, `52f85828` — a launch waits for an ending instance and keeps the
  launch id/retry.
- `3f0d9973`, `8f7e1c42`, `63ad0209` — bare `WM_CHAR`/`VK_PACKET` typing,
  surrogate pairs, and no double character from a posted key.

### Kit / Luau

- `623b87df` — `Preview::on_key` hears kit image-preview keys before zoom.
- `3e28b51a`, `a52c5dce`, `f52aeaee`, `63ccfc47` — Luau tooling; unused here.

No new build requirement beyond the optional `rune-kit` `math` deps.

## Patch status against the new head

The four ported patches still fail `git apply --check`. A dry run against a
`7a707454` worktree has the **same hunk counts**, so the last 33 commits
changed the rework content, not which hunks fail.

| Patch | Applies | Failing hunks | Moved files / note |
|---|---|---|---|
| 0001 inline objects | no | 16/19 + 1 misapplied | `text.rs` → `rune-text/src/lib.rs` + `layout/shape.rs`; `text/{span,hash,pill_chips}.rs` → `rune-text/src/layout/`; paragraph hashing → `rune-text/src/paragraphs/keying.rs`. Hunk 8 lands in the 11-line re-export file. Paint now meets `[f32;2]`/`Replace` |
| 0002 close veto | no | 8/15 | `window/lifecycle.rs` → `window/host/lifecycle.rs`; hook storage must move into generic `Shared<S>`; `spec.rs` forwards via `forward_app_builders!`; `handle.rs::close` is a one-liner |
| 0003 a11y labels | no | 14/21 | `a11y/text.rs` split into `text/runs.rs` + `text/long_field.rs`; `a11y/node.rs` context |
| 0004 highlight box | no | 2/2 unplaceable | `text/cursor.rs` → `rune-text/src/layout/cursor.rs`; `text/tests.rs` → `rune-text/src/tests.rs` |
| 0005 Mermaid SVG | new, applies on the patched tree | — | `rune-mermaid` static `render_svg`/`Drawing::to_svg`/`DrawingModel::to_svg`; export/lightbox use it. Port after 0004; ~1–2 h. |

A useful simplification for 0001: hashing now lives in
`Span::hash_layout`/`affects_layout`, so the old `hash.rs`/`paragraphs.rs`
hunks are obsolete — add `image`/`rise` there instead.

`autotests = false` remains on `rune`/`rune-kit`, so the three patch test files
need explicit `[[test]]` entries in the package manifest or
`scripts/test-rune-patches.sh` will silently run nothing.

## Inline math: it does not replace `lmdv-math`

Two layers landed:

1. **`El::inline_image(range, rune::InlineImage)`** — element-level. It replaces
   a text range's bytes with a baseline image; the bytes remain for selection,
   copy and search. It is built on `rune_text::Replace`/`BareIcon`, hidden-size
   label shaping and rectangular pill boxes. It exposes **no** `Span::image`,
   no `Span::rise` and no accessible label: a screen reader still reads the raw
   bytes.
2. **`rune-kit` math** (`math` feature, default off) — pulldown-cmark rules,
   `ratex-math.wasm` inside `wasmi` (native), host rasterisation. Display math
   is an image element; only the fence/collection helpers are public.

`lmdv-math` already uses native `ratex-*` with embedded fonts and exact
ascent/descent. Adopting kit math would mean adopting `rune-kit::Markdown`,
adding wasmi and ~566 KiB of assets, and changing text metrics for no benefit.
**Keep `lmdv-math` and patch 0001.**

## Do the four patch reasons survive?

| Reason | Native replacement? |
|---|---|
| Inline objects and labels | **Partial** — element-level `El::inline_image`; no span image, rise or label |
| Close veto | **No** — `CloseRequested` still hides first and runs `on_close` after |
| Accessible inline objects | **No** — no a11y change; raw bytes are still spoken |
| Highlight over an inline image | **No** — `TextHighlight` excludes pill/replace boxes |

All five patches remain ours; 0005 is additive on the patched tree.

## New APIs vs the open gaps

- **O-14/O-03:** `Tx::scroll_key_center_at` centers an element or keyed
  `TextHighlight` (device-pixel snapped), useful for find/landing. It does not
  expose `BrowserBack`/`BrowserForward`, and Alt+arrows are still consumed by
  focused fields (OI-150 stays).
- **T-01/T-03/T-19 (OI-190):** only the tab-title fade improves. No per-name tab
  width, no Saved status (`TabItemStatus` is still Busy/Attention/Unread/
  Failed), drag slop is still 4.0.
- **C-01/IME:** focus-ring and focus-`Entry` preservation help ring stability;
  there is still no IME composing state.
- **Frame rate:** no present-mode or frame-rate-cap API; no settings row is planned (the scroll speedup was accepted as the fix).
- **OI-155:** Windows key/instance fixes only; no path identity, modes or
  association work.
- **Images/scroll:** task-thread image decoders (recheck headless `wait`
  scenes) and a dense node index (large trees no longer grow with retired
  paths).

## Recommended port order and estimate

1. Re-vendor `63ad0209`, update `RUNE_PIN.md`/`UPSTREAM.md`, confirm the
   unpatched `cargo check -p rune`.
2. **0002 close veto** first (independent): `Shared<S>` hook,
   `Runtime::close_request`, `window/host/lifecycle.rs`, `spec.rs` macro list,
   handle doc, test.
3. **0001 inline objects**: `layout/span.rs` fields +
   `affects_layout`/`hash_layout`, `layout/shape.rs`, `pill_chips`, paint loop,
   tests.
4. **0004 highlight box**: `layout/cursor.rs` (recheck the new
   radius/vertical-padding geometry) and `rune-text/src/tests.rs`.
5. **0003 a11y labels**: `a11y/text.rs`, `text/runs.rs`,
   `text/long_field.rs`, `a11y/node.rs`.
6. Goldens, trees, math a11y and image scenes; add the three `[[test]]`
   entries.

**Estimate: ~2.5–3 days** (0001 1.25–1.5 d, 0002 3–4 h, 0003 3–4 h, 0004
1–1.5 h, verification 3–5 h, re-vendor/build 2–3 h). About 2–4 h over the
previous estimate, all in 0001 and verification.

**Risks:** two public `InlineImage` types after patching (`rune::` vs
`rune::text::`); unregistered patch tests under `autotests = false`;
`Replace`/`PILL_BOX` coexisting with our `IMAGE_BOX` around hidden-run shaping
(watch the `text_spec` shape-cache flake); moved highlight geometry;
task-thread image decode timing; one `ScrollFade` stop curve per window; do not
enable kit `math`.

## Decision

The pin stays `cca5df6` until the port. Port to `63ad0209` is scheduled as
[OI-189](OPEN_ITEMS.md); it is not a drop-in, and it is not started yet.
