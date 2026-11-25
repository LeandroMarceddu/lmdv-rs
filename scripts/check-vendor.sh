#!/usr/bin/env bash
# Check that vendor/rune is the pinned tree, that the root workspace repeats
# Rune's patched crates, and that release headless runs keep Rune's tooling.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
DEST="$ROOT/vendor/rune"
REF="$ROOT/docs/RUNE_PIN.md"
rc=0

fail() { echo "FAIL: $*" >&2; rc=1; }

[ -f "$DEST/Cargo.toml" ] || fail "vendor/rune/Cargo.toml is missing"
[ -f "$DEST/crates/rune/Cargo.toml" ] || fail "vendor/rune/crates/rune is missing"
[ -f "$DEST/vendor/wgpu-hal/Cargo.toml" ] || fail "vendor/rune/vendor/wgpu-hal is missing"
[ -f "$DEST/vendor/naga/Cargo.toml" ] || fail "vendor/rune/vendor/naga is missing"
[ -f "$DEST/vendor/winit/Cargo.toml" ] || fail "vendor/rune/vendor/winit is missing"
[ -f "$DEST/vendor/sctk-adwaita/Cargo.toml" ] || fail "vendor/rune/vendor/sctk-adwaita is missing"
[ -f "$DEST/vendor/parley_engine/Cargo.toml" ] || fail "vendor/rune/vendor/parley_engine is missing"
[ -f "$DEST/vendor/accesskit_windows/Cargo.toml" ] || fail "vendor/rune/vendor/accesskit_windows is missing"
[ -f "$DEST/vendor/accesskit_atspi_common/Cargo.toml" ] || fail "vendor/rune/vendor/accesskit_atspi_common is missing"
[ -f "$DEST/vendor/accesskit_macos/Cargo.toml" ] || fail "vendor/rune/vendor/accesskit_macos is missing"
[ -f "$DEST/vendor/tiny-skia-path/Cargo.toml" ] || fail "vendor/rune/vendor/tiny-skia-path is missing"

want="$(sed -n 's/^- Snapshot commit: `\([0-9a-f]*\)`.*/\1/p' "$REF" | head -1)"
have="$(sed -n 's/^- Pinned rev: `\([0-9a-f]*\)`.*/\1/p' "$DEST/UPSTREAM.md" 2>/dev/null | head -1)"
if [ -n "$want" ] && [ "$have" != "$want" ]; then
    fail "vendored rev '$have' != reference rev '$want'"
fi

for crate in wgpu-hal naga winit sctk-adwaita parley_engine accesskit_windows accesskit_atspi_common accesskit_macos; do
    grep -q "path = \"vendor/rune/vendor/$crate\"" "$ROOT/Cargo.toml" \
        || fail "root Cargo.toml does not patch $crate to vendor/rune/vendor/$crate"
done

# `tiny-skia-path` is a path dependency of Rune's own manifest, not a patch, so
# the root workspace must not redirect resvg's copy of the published crate.
grep -q 'path = "../../vendor/tiny-skia-path"' "$DEST/crates/rune-text/Cargo.toml" \
    || fail "vendor/rune/crates/rune-text does not use the vendored tiny-skia-path"
if grep -qE '^tiny-skia-path[[:space:]]*=' "$ROOT/Cargo.toml"; then
    fail "root Cargo.toml patches tiny-skia-path; Rune keeps it a path dependency only"
fi

# Release builds only keep `--snap`, `--tree` and `--script` with `tooling`.
grep -q '"tooling"' "$ROOT/Cargo.toml" \
    || fail "root Cargo.toml does not enable Rune's tooling feature for release headless runs"

patches="$(find "$DEST/patches" -maxdepth 1 -name '*.patch' -type f 2>/dev/null | wc -l)"
echo "vendor: ${have:-unknown} (reference: ${want:-unknown}), patches: $patches"
if (( rc == 0 )); then echo "vendor check: OK"; else exit 1; fi
