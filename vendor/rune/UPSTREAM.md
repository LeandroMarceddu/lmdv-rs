# Vendored Rune

- Repository: https://github.com/anomalyco/rune
- Pinned rev: `07bf8c5d3b04d2291f5493392aee8e0694096ce5`
- Vendored: 2026-11-28T10:42:33Z
- Toolchain: 1.99.1
- Source: `/tmp/opencode/rune-delta-port`

This tree is managed by `scripts/vendor-rune.sh`. Do not edit it by hand:
put changes in `vendor/rune/patches/*.patch` and apply them with
`scripts/apply-rune-patches.sh`. The root workspace repeats Rune's
`[patch.crates-io]` entries because a dependency's patches do not apply to
the dependent workspace.
