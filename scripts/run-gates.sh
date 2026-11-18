#!/usr/bin/env bash
# The workspace gate. Run from the repository root.
#
# This is the gate the orchestrator runs; a worker's "it passes" is not
# evidence. It intentionally does not build vendor/rune as a workspace member.
#
# The vendor patch proof tests are separate: they build vendor/rune's own huge
# test binaries. Run scripts/test-rune-patches.sh after a vendor bump or patch
# edit, or set LMDV_RUNE_PATCH_TESTS=1 for a full gate that includes them.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"

echo "== parity checklist =="
python3 scripts/gen-parity-checklist.py --check

echo "== vendor =="
scripts/check-vendor.sh

echo "== fmt =="
scripts/fmt-check.sh

echo "== check =="
cargo check --workspace

if [ "${LMDV_RUNE_PATCH_TESTS:-0}" = "1" ]; then
    echo "== rune patches =="
    scripts/test-rune-patches.sh
fi

echo "== test =="
cargo test --workspace

echo "== clippy =="
cargo clippy --workspace --all-targets

echo "== gate: OK =="
