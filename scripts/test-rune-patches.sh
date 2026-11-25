#!/usr/bin/env bash
# Run the proof tests for the numbered Rune patches.
#
# Run this after a vendor bump or patch edit, not on every workspace gate:
# it builds vendor/rune's own test binaries under vendor/rune/target.
# The vendored Rune workspace has its own dev-dependencies, so these run with
# its manifest, not as a member of the lmdv workspace.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"

cargo test --manifest-path vendor/rune/Cargo.toml -p rune \
    --test inline_image_layout \
    --test inline_image_scene
# Patch 0003: the accessibility text speaks an inline image's label.
cargo test --manifest-path vendor/rune/Cargo.toml -p rune --lib a11y::tests::inline_images
# Patch 0004: a highlight over an inline image keeps the picture's box.
cargo test --manifest-path vendor/rune/Cargo.toml -p rune-text \
    --lib a_range_over_an_inline_image_keeps_the_picture_box
# Patch 0005: the static Mermaid SVG serializer and its self-containment tests.
cargo test --manifest-path vendor/rune/Cargo.toml -p rune-mermaid --lib svg
