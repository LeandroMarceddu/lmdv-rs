# Vendored Rune

- Repository: https://github.com/anomalyco/rune
- Pinned rev: `af6b1fdd9537a3f278f9c85e0f0893f1e773302a`
- Vendored: 2026-11-28T08:12:30Z
- Toolchain: 1.99.1
- Source: `/tmp/opencode/rune-head-patches-research`

This tree is managed by `scripts/vendor-rune.sh`. Do not edit it by hand:
put changes in `vendor/rune/patches/*.patch` and apply them with
`scripts/apply-rune-patches.sh`. The root workspace repeats Rune's
`[patch.crates-io]` entries because a dependency's patches do not apply to
the dependent workspace.
