# Rune upstream delta — 2026-11-20

Research against the pin in [`RUNE_PIN.md`](RUNE_PIN.md).

| | |
|---|---|
| Pinned | `cca5df6fafb4721d96fe105f126ebfd2ec351bb6` (2026-11-17) |
| Upstream head | `7a707454fbfb8d3fd200723c65778fb7b98fd31e` (2026-11-20 14:26 −0600) |
| Branch | `dev`, which is also the default branch (`production` is an ancestor) |
| Commits since the pin | 130 |
| Toolchain | unchanged, 1.99.1 |

## What changed

The window is dominated by a **one-time structural refactor**, not new behavior:

- **Crate extraction:** `rune-text` (`259acd75`), `rune-core` (`f1f9949f`), `rune-gpu`, `rune-image`, `rune-spell`, `rune-media`, `rune-edit`; type-erased `RuntimeCore`/`window::Host`/`overlay::Overlay`; `a11y/text.rs` split into `text/runs.rs` + `text/long_field.rs`; mermaid reorganized by capability.
- **Text/layout:** colour-only spans leave the shaping key (`f0d32388`, ink resolved at glyph emission), per-line code shaping (`698ad260`), layout-ahead cache, whole-pixel list text.
- **Accessibility:** Linux AT-SPI field panic fixed (`d170ff6e`), one focus ring, canonical role table, tooltip focus rules.
- **Window/kit:** unified `App`/`Runtime` config and `WindowSpec` forwarders, tab strip/table/tree/popup built once per state (`04ce0f1b`), markdown memoisation, Wayland corners, scrollbar wheel.
- **Build:** workspace deps/default-members, zbus 5, objc2 0.6, stricter lints, and **`autotests = false`** (new `[[test]]` entries are mandatory).

## Impact on lmdv

- Every `rune::`/`rune_kit::` symbol lmdv imports still resolves; `WindowSpec` builders survive via `forward_app_builders!`.
- No upstream `InlineImage`, `Span::image`/`rise`, U+FFFC handling, or close-request hook — **all four patches remain ours**.
- `ElementInfo::element_is_secure` was removed (unused).
- The `text_spec` "a text changed after its shape key was taken" assertion is intact. `f0d32388` is the relevant change: the port must add `image`/`rise` to `Span::affects_layout`/`hash_layout`, the single key-building site now.

## Patch status against head (throwaway copy)

| Patch | Applies | Needs rework |
|---|---|---|
| 0001 inline objects | 3 hunks + tests | 8 hunks + 8 unplaceable (`text/span.rs`, `hash.rs`, `paragraphs.rs`, `pill_chips.rs` moved to `crates/rune-text/src/layout/`), plus a paint-hook rewrite |
| 0002 close veto | 8 hunks | 4 (`app.rs`, `app/requests.rs` with the `Shared`/`SharedCore` split, `window/spec.rs`) + 3 moved |
| 0003 a11y labels | 12 hunks | 9 (substitution now lives in `a11y/text/runs.rs`/`long_field.rs`) |
| 0004 highlight box | 0 | 2 moved to `rune-text/src/layout/cursor.rs` |

Unpatched head compiles cleanly (`cargo check -p rune`, 24 s). The mechanically patched tree does not (11 errors).

## Recommendation

**Bump — scheduled, not drop-in.** The refactor has already moved our patch sites; staying on the pin only defers the port against more drift and keeps two relevant bugs (Linux field a11y panic, reshape on highlight changes).

Estimated cost: 0001 ~1 day, 0002 ~3–4 h, 0003 ~3–4 h, 0004 ~1 h, goldens ~2–4 h, plus a full vendored rebuild and `Cargo.lock` refresh. The patch tests must be registered as `[[test]]` entries because of `autotests = false`, or they silently never run.

Land the current gate first, then port onto that day's head.
