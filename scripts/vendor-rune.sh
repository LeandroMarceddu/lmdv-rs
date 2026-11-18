#!/usr/bin/env bash
# Vendor the pinned Rune workspace into vendor/rune.
#
# This is intentionally conservative: it refuses to overwrite an existing
# vendor/rune unless --force is given, because the local tree carries numbered
# patches. Re-vendoring is a deliberate rev-bump operation, not a sync.
#
# Usage:
#   scripts/vendor-rune.sh [--rev REV] [--from PATH] [--force] [--patches]
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
DEST="$ROOT/vendor/rune"
REF="$ROOT/docs/RUNE_PIN.md"
SRC="${LMDV_RUNE_SRC:-/tmp/opencode/rune}"
REV=""
FORCE=0
PATCH=0

while (($#)); do
    case "$1" in
        --rev) REV="${2:?--rev needs a value}"; shift ;;
        --from) SRC="${2:?--from needs a path}"; shift ;;
        --force) FORCE=1 ;;
        --patches) PATCH=1 ;;
        -h|--help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

if [ -z "$REV" ]; then
    REV="$(awk '/^- Snapshot commit:/ { print $4; exit }' "$REF")"
fi
[ -n "$REV" ] || { echo "error: no rev; pass --rev or set docs/RUNE_PIN.md" >&2; exit 2; }

if [ -e "$DEST" ] && (( ! FORCE )); then
    echo "error: $DEST already exists; re-vendoring would discard local patches." >&2
    echo "       Re-run with --force only when you intend to bump the rev." >&2
    exit 1
fi

if [ ! -d "$SRC/.git" ] && [ ! -f "$SRC/crates/rune/Cargo.toml" ]; then
    echo "cloning Rune into $SRC ..."
    git clone https://github.com/anomalyco/rune.git "$SRC"
fi

echo "checking out $REV in $SRC ..."
git -C "$SRC" fetch --all --tags --quiet || true
git -C "$SRC" checkout --quiet "$REV"

if (( FORCE )); then
    rm -rf "$DEST"
fi
mkdir -p "$DEST"
echo "copying Rune workspace (without .git/target/demo) ..."
rsync -a --exclude=.git --exclude=target --exclude=demo --exclude='*.mp4' --exclude='*.mov' \
    "$SRC/" "$DEST/"

# The vendored tree is formatted with the repository's rustfmt config so the
# numbered patches apply to a deterministic baseline. This is a managed
# divergence from upstream formatting, not a functional change.
cp "$ROOT/rustfmt.toml" "$DEST/rustfmt.toml"
cargo fmt --manifest-path "$DEST/Cargo.toml" --all

ACTUAL="$(git -C "$SRC" rev-parse HEAD)"
cat >"$DEST/UPSTREAM.md" <<EOF
# Vendored Rune

- Repository: https://github.com/anomalyco/rune
- Pinned rev: \`$ACTUAL\`
- Vendored: $(date -u +%FT%TZ)
- Toolchain: 1.99.1
- Source: \`$SRC\`

This tree is managed by \`scripts/vendor-rune.sh\`. Do not edit it by hand:
put changes in \`vendor/rune/patches/*.patch\` and apply them with
\`scripts/apply-rune-patches.sh\`. The root workspace repeats Rune's
\`[patch.crates-io]\` entries because a dependency's patches do not apply to
the dependent workspace.
EOF

echo "vendored $ACTUAL -> $DEST"
if (( PATCH )); then
    "$ROOT/scripts/apply-rune-patches.sh"
fi
echo "next: scripts/check-vendor.sh && cargo check --workspace"
