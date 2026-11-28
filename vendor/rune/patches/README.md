# Rune patches

No local patches. All four previously carried behaviors — inline objects with
fractional metrics/rise/labels, a11y label substitution, inline-image highlight
boxes, and static Mermaid SVG — landed upstream in Rune `07bf8c5d3` (#2743).

`scripts/vendor-rune.sh` applies every `*.patch` found here; the directory is
kept empty on purpose so re-vendoring is a plain upstream copy. The upstream
feature tests are run by `scripts/test-rune-patches.sh`.
