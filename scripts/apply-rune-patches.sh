#!/usr/bin/env bash
# Apply (or check) the numbered patches in vendor/rune/patches/.
#
# Usage:
#   scripts/apply-rune-patches.sh [--check] [--reverse]
#
# Patches are `diff -ruN`/git-style with paths relative to vendor/rune
# (e.g. a/crates/rune/src/text/span.rs). The script is idempotent: a patch
# already applied is skipped.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
DEST="$ROOT/vendor/rune"
DIR="$DEST/patches"
MODE="apply"

while (($#)); do
    case "$1" in
        --check) MODE="check" ;;
        --reverse) MODE="reverse" ;;
        -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

[ -d "$DEST" ] || { echo "error: $DEST is missing; run scripts/vendor-rune.sh" >&2; exit 1; }
[ -d "$DIR" ] || { echo "no patches directory; nothing to do"; exit 0; }

mapfile -t PATCHES < <(find "$DIR" -maxdepth 1 -name '*.patch' -type f | sort)
(( ${#PATCHES[@]} )) || { echo "no patches; nothing to do"; exit 0; }

rc=0
for p in "${PATCHES[@]}"; do
    name="$(basename "$p")"
    if patch -p1 -d "$DEST" --dry-run -R --silent <"$p" >/dev/null 2>&1; then
        echo "already applied: $name"
        if [ "$MODE" = "reverse" ]; then
            patch -p1 -d "$DEST" -R <"$p"
            echo "reversed: $name"
        fi
        continue
    fi
    case "$MODE" in
        check)
            if patch -p1 -d "$DEST" --dry-run --silent <"$p" >/dev/null 2>&1; then
                echo "would apply: $name"
            else
                echo "FAILS: $name" >&2
                rc=1
            fi
            ;;
        apply)
            echo "applying: $name"
            if ! patch -p1 -d "$DEST" <"$p"; then
                echo "error: $name failed" >&2
                exit 1
            fi
            ;;
        reverse)
            echo "not applied: $name"
            ;;
    esac
done
exit "$rc"
