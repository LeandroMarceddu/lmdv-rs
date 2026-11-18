#!/usr/bin/env bash
# Check formatting for lmdv's own crates only.
#
# `cargo fmt --all` follows path dependencies into vendor/rune and would
# reformat the vendored framework. Keep this list in sync with the workspace
# members as crates land.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"

PACKAGES=(
    lmdv
    lmdv-core
    lmdv-io
    lmdv-app
    lmdv-md
    lmdv-math
    lmdv-i18n
    lmdv-render
    lmdv-edit
    lmdv-export
)
for pkg in "${PACKAGES[@]}"; do
    cargo fmt -p "$pkg" -- --check
done
