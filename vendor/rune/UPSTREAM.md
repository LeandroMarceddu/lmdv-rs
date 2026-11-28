# Vendored Rune

- Repository: https://github.com/anomalyco/rune
- Pinned rev: `86df731b7171f52c9918f288784dda6ed0327289`
- Vendored: 2026-11-28T12:39:20Z
- Toolchain: 1.99.1
- Source: `/tmp/opencode/rune-delta-port`

This tree is managed by `scripts/vendor-rune.sh`. Do not edit it by hand:
put changes in `vendor/rune/patches/*.patch` and apply them with
`scripts/apply-rune-patches.sh`. The root workspace repeats Rune's
`[patch.crates-io]` entries because a dependency's patches do not apply to
the dependent workspace.
