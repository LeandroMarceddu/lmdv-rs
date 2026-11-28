#!/usr/bin/env bash
# Run the upstream feature tests that replaced lmdv's four Rune patches.
#
# There are no local patches as of Rune 07bf8c5d3 (#2743). These are the
# upstream tests for the features we used to carry: inline image metrics/rise/
# labels, a11y label substitution, highlight boxes, and Mermaid SVG export.
# Run after a vendor bump; it builds vendor/rune's own test binaries under
# vendor/rune/target, so it is not part of every workspace gate.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"

cargo test --manifest-path vendor/rune/Cargo.toml -p rune-text --lib layout::cursor::tests
cargo test --manifest-path vendor/rune/Cargo.toml -p rune --lib el::pills::
cargo test --manifest-path vendor/rune/Cargo.toml -p rune --lib a11y::
cargo test --manifest-path vendor/rune/Cargo.toml -p rune-mermaid --lib
cargo test --manifest-path vendor/rune/Cargo.toml -p rune-kit --lib --features mermaid mermaid::tests
