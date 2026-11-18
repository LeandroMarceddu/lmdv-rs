# Vendored Rune

- Repository: https://github.com/anomalyco/rune
- Pinned rev: `cca5df6fafb4721d96fe105f126ebfd2ec351bb6`
- Vendored: 2026-11-17T14:19:48Z
- Toolchain: 1.99.1
- Source: `/tmp/opencode/rune-scout`

This tree is managed by `scripts/vendor-rune.sh`. Do not edit it by hand:
put changes in `vendor/rune/patches/*.patch` and apply them with
`scripts/apply-rune-patches.sh`. The root workspace repeats Rune's
`[patch.crates-io]` entries because a dependency's patches do not apply to
the dependent workspace.
