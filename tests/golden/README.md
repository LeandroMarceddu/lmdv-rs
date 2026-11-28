# Rendered golden corpus (OI-055)

This is the committed headless corpus for the `docs/TESTING.md` §4 layers. It
was assembled from existing deterministic `AppSpec` scenes; it is intentionally
small — one representative frame per main surface — and grows when a scene
lands, not when a machine happens to be free.

| Scene | PNG | Tree | Surface |
|---|---|---|---|
| `reader` | `reader.png` | `../trees/reader.json` | article, headings, tables in the reading column |
| `table-preview` | `table-preview.png` | `../trees/table-preview.json` | table grid, sticky header, frozen column, pinned scroll bars (OI-044) |
| `start` | `start.png` | `../trees/start.json` | start page and recents surface |
| `editor` | `editor.png` | `../trees/editor.json` | source editor and format bar |
| `reader-outline` | `reader-outline.png` | `../trees/reader-outline.json` | outline/selection rail |

## Capture recipe

```bash
cargo build -p lmdv
for scene in reader table-preview start editor reader-outline; do
  ./target/debug/lmdv \
    --snap tests/golden/$scene.png \
    --tree tests/trees/$scene.json \
    --scene $scene --size 1280x900 --scale 2 --theme light
done
```

The same command pair is the regeneration path; the binary already consumes
Rune's `--snap` / `--tree` / `--scene` / `--size` / `--scale` / `--theme` flags
(`crates/lmdv/src/main.rs:15-21`). Headless frames parse and decode inline, so
one frame is deterministic (`docs/TESTING.md` §3).

## Tolerance — compare, do not diff bytes

- **PNG:** perceptual, not exact. A frame passes at **SSIM ≥ 0.995**, or at
  **MAE ≤ 2/255** when an SSIM implementation is unavailable. Text AA, GPU
  rasterisation and font hinting differ across drivers; structure must not.
  A failure is either an intended visual change (regenerate with a written
  reason in the commit) or a regression (fix the code). **Never** widen a
  tolerance globally to hide a regression; use a per-image exception with a
  comment naming the driver and the change.
- **Tree JSON:** keys, roles, labels, focus, selection, sort/expanded state and
  live-region flags are compared **exactly** — this is the primary a11y gate.
  `bounds` / `rect` and animation timestamps are recorded for context but are
  not asserted: they depend on the capture scale and the compositor. The kit's
  scroll-bar tracks are `a11y_hidden`, so they appear in `elements[]` but not
  in `accessibility`; that is expected and is asserted in
  `tables_tests::the_preview_scroll_bars_stay_pinned_while_the_grid_scrolls`.

The corpus holds *debug* captures at 2×. A release/perf capture may differ in
AA; that is why the PNG comparison is perceptual and the tree comparison is
structural.
