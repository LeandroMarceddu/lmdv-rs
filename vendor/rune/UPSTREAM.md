# Vendored Rune

- Repository: https://github.com/anomalyco/rune
- Pinned rev: `f4ec14ed32256ae0c36e73476825f9245437de5b`
- Vendored: 2026-11-25T16:59:47Z
- Toolchain: 1.99.1
- Source: `/tmp/opencode/rune-delta-port`

This tree is managed by `scripts/vendor-rune.sh`. Do not edit it by hand:
put changes in `vendor/rune/patches/*.patch` and apply them with
`scripts/apply-rune-patches.sh`. The root workspace repeats Rune's
`[patch.crates-io]` entries because a dependency's patches do not apply to
the dependent workspace.
