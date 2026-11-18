#!/usr/bin/env bash
# package-appimage.sh -- build lmdv-<version>-<arch>.AppImage from this checkout.
#
# Builds (or reuses) target/release/lmdv, stages the AppDir layout from
# packaging/ and assets/, validates the staged metadata, then hands the AppDir
# to linuxdeploy (preferred: it also deploys shared libraries) or appimagetool.
#
# This script performs NO downloads. Tools:
#   * linuxdeploy with its appimage output plugin
#       Arch/CachyOS: sudo pacman -S --needed linuxdeploy-appimage
#       elsewhere:    the linuxdeploy AppImage from github.com/linuxdeploy
#   * or appimagetool (set APPIMAGETOOL=/path/to/appimagetool)
# linuxdeploy's AppImage plugin may fetch the AppImage runtime once and cache
# it; pass --runtime-file FILE (or RUNTIME_FILE=...) for a fully offline run.
#
# Usage:
#   scripts/package-appimage.sh                  # release build + dist/lmdv-<v>-<arch>.AppImage
#   scripts/package-appimage.sh --dry-run        # print the plan, change nothing
#   scripts/package-appimage.sh --no-build       # reuse target/release/lmdv
#   scripts/package-appimage.sh --appdir-only    # stage + validate, skip the image
#   scripts/package-appimage.sh --binary ./target/debug/lmdv --out-dir /tmp/lmdv-appimage
#   scripts/package-appimage.sh --update-info 'zsync|https://example.invalid/lmdv.AppImage.zsync'
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
APP_NAME="lmdv"

OUT_DIR="$ROOT/dist"
APPDIR=""
BINARY=""
BUILD=1
APPDIR_ONLY=0
DRY_RUN=0
APP_VERSION=""
UPDATE_INFO=""
RUNTIME_FILE="${RUNTIME_FILE:-}"
ARCH="${ARCH:-$(uname -m)}"

say()  { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
usage: package-appimage.sh [options]

  --binary PATH        package this binary instead of building one
  --no-build           reuse target/release/lmdv; fail if it is missing
  --appdir-only        stage and validate the AppDir; do not create the image
  --out-dir DIR        output directory (default: <repo>/dist)
  --appdir DIR         AppDir path, must be inside --out-dir (default: <out-dir>/AppDir)
  --app-version V      override the version read from Cargo.toml
  --update-info URL    zsync update information embedded as UPDATE_INFORMATION
  --runtime-file FILE  AppImage runtime for linuxdeploy (avoids any download)
  --dry-run            print every step and exit without changing anything
  -h, --help           this text

Environment: ARCH overrides uname -m; APPIMAGETOOL points at appimagetool;
RUNTIME_FILE is the same as --runtime-file.
EOF
}

while (($#)); do
    case "$1" in
        --binary)       BINARY="${2:?--binary needs a path}"; shift ;;
        --no-build)     BUILD=0 ;;
        --appdir-only)  APPDIR_ONLY=1 ;;
        --out-dir)      OUT_DIR="${2:?--out-dir needs a path}"; shift ;;
        --appdir)       APPDIR="${2:?--appdir needs a path}"; shift ;;
        --app-version)  APP_VERSION="${2:?--app-version needs a value}"; shift ;;
        --update-info)  UPDATE_INFO="${2:?--update-info needs a value}"; shift ;;
        --runtime-file) RUNTIME_FILE="${2:?--runtime-file needs a path}"; shift ;;
        --dry-run)      DRY_RUN=1 ;;
        -h|--help)      usage; exit 0 ;;
        *)              die "unknown argument: $1 (try --help)" ;;
    esac
    shift
done

# Resolve paths from the caller's cwd, never from the repo root.
abs() { realpath -m -- "$1"; }
OUT_DIR="$(abs "$OUT_DIR")"
[ -n "$APPDIR" ] || APPDIR="$OUT_DIR/AppDir"
APPDIR="$(abs "$APPDIR")"

case "$OUT_DIR" in
    /) die "refusing to use / as the output directory" ;;
esac

case "$APPDIR" in
    / | "$ROOT" | "$OUT_DIR") die "refusing to use $APPDIR as the AppDir (it is the repository, the output directory or the filesystem root)" ;;
    "$OUT_DIR"/*) : ;;
    *) die "the AppDir must live inside the output directory ($OUT_DIR), got $APPDIR" ;;
esac

if [ -n "$BINARY" ]; then
    BINARY="$(abs "$BINARY")"
fi

manifest_version() {
    awk '
        /^\[workspace\.package\]/ { section = 1; next }
        /^\[/ { section = 0 }
        section && /^version[[:space:]]*=/ {
            value = $0
            sub(/^[^=]*=[[:space:]]*/, "", value)
            gsub(/["[:space:]]/, "", value)
            print value
            exit
        }
    ' "$ROOT/Cargo.toml"
}

if [ -z "$APP_VERSION" ]; then
    APP_VERSION="$(manifest_version)"
fi
[ -n "$APP_VERSION" ] || die "cannot read [workspace.package] version from $ROOT/Cargo.toml; pass --app-version"

OUTPUT="$OUT_DIR/$APP_NAME-$APP_VERSION-$ARCH.AppImage"
BUILD_DIR="$ROOT/target/release"

command_string() {
    local out="" arg
    for arg in "$@"; do
        printf -v arg '%q' "$arg"
        out+="$arg "
    done
    printf '%s' "${out% }"
}

step() { say ""; say "== $*"; }

run() {
    if ((DRY_RUN)); then
        say "  + $(command_string "$@")"
    else
        "$@"
    fi
}

have() { command -v "$1" >/dev/null 2>&1; }
tool_path() { command -v "$1" 2>/dev/null || true; }
have_appimagetool() { [ -n "$APPIMAGETOOL" ] && [ -x "$APPIMAGETOOL" ]; }

# linuxdeploy ships its appimage output plugin in the packaged AppImage; detect
# it rather than assuming, because a distro build can omit plugins.
linuxdeploy_has_appimage() {
    [ -n "$1" ] || return 1
    "$1" --list-plugins 2>/dev/null | grep -qE '^appimage:'
}

# ---------------------------------------------------------------- plan header
step "$APP_NAME $APP_VERSION ($ARCH)"
say "  repository: $ROOT"
say "  out dir:    $OUT_DIR"
say "  AppDir:     $APPDIR"
say "  image:      $OUTPUT"
((DRY_RUN)) && say "  mode:       dry run (nothing will be written)"
((APPDIR_ONLY)) && say "  mode:       AppDir only (no .AppImage)"

# --------------------------------------------------------------- the binary
step "binary"
if [ -n "$BINARY" ]; then
    if [ -x "$BINARY" ]; then
        say "  using --binary $BINARY"
    elif ((DRY_RUN)); then
        warn "--binary $BINARY does not exist (dry run: continuing with the plan)"
    else
        die "--binary is not an executable file: $BINARY"
    fi
elif ((BUILD)); then
    say "  build: cargo build --release -p lmdv --locked (target dir: $ROOT/target)"
    run cargo build --manifest-path "$ROOT/Cargo.toml" --release -p lmdv --locked
    BINARY="$BUILD_DIR/lmdv"
    if [ ! -x "$BINARY" ] && ! ((DRY_RUN)); then
        die "cargo did not produce $BINARY"
    fi
    if ((DRY_RUN)); then
        say "  would build: $BINARY"
    else
        say "  built: $BINARY"
    fi
else
    BINARY="$BUILD_DIR/lmdv"
    if [ ! -x "$BINARY" ] && ! ((DRY_RUN)); then
        die "--no-build was given but $BINARY does not exist; run without --no-build"
    fi
    say "  reuse: $BINARY"
fi

# ------------------------------------------------------------------- staging
step "stage AppDir"
if ((DRY_RUN)); then
    say "  + rm -rf $(command_string "$APPDIR")"
else
    rm -rf -- "$APPDIR"
    mkdir -p -- "$APPDIR"
fi

stage() { # src dest mode
    local src="$1" dest="$2" mode="$3"
    if [ ! -e "$src" ] && ! ((DRY_RUN)); then
        die "missing packaging input: $src"
    fi
    run install -Dm"$mode" "$src" "$APPDIR/$dest"
}

stage "$BINARY" "usr/bin/lmdv" 755
stage "$ROOT/packaging/lmdv.desktop" "usr/share/applications/lmdv.desktop" 644
# Integration tools rewrite Exec to the absolute .AppImage path but generally
# leave TryExec alone; a stale TryExec=lmdv hides the entry in launchers that
# enforce it. Drop it from the image copy only.
run sed -i '/^TryExec=/d' "$APPDIR/usr/share/applications/lmdv.desktop"
stage "$ROOT/assets/lmdv.svg" "usr/share/icons/hicolor/scalable/apps/lmdv.svg" 644
stage "$ROOT/packaging/lmdv.xml" "usr/share/mime/packages/lmdv.xml" 644
stage "$ROOT/packaging/lmdv.metainfo.xml" "usr/share/metainfo/md.lmdv.metainfo.xml" 644

# AppRun: the only entry point the AppImage runtime executes.
write_apprun() {
    local target="$APPDIR/AppRun"
    if ((DRY_RUN)); then
        say "  + write $target"
        return 0
    fi
    cat >"$target" <<'APPRUN'
#!/bin/sh
# lmdv AppRun -- generated by scripts/package-appimage.sh.
# Derive APPDIR from this script's location: the AppImage runtime runs
# $APPDIR/AppRun, and running the staged AppDir directly works the same way.
# Do not trust an inherited APPDIR (a shell can already be inside another
# AppImage's mount).
set -eu
APPDIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
export APPDIR
PATH="$APPDIR/usr/bin:$PATH"
export PATH
exec "$APPDIR/usr/bin/lmdv" "$@"
APPRUN
    chmod 755 "$target"
}
step "AppRun"
write_apprun
((DRY_RUN)) || say "  wrote $APPDIR/AppRun"

# --------------------------------------------------------------- validation
validate_appdir() {
    say ""
    say "== validate staged metadata"
    if have desktop-file-validate; then
        run desktop-file-validate "$APPDIR/usr/share/applications/lmdv.desktop"
    else
        warn "desktop-file-validate not found (desktop-file-utils); skipping"
    fi
    if have xmllint; then
        run xmllint --noout \
            "$APPDIR/usr/share/mime/packages/lmdv.xml" \
            "$APPDIR/usr/share/metainfo/md.lmdv.metainfo.xml" \
            "$APPDIR/usr/share/icons/hicolor/scalable/apps/lmdv.svg"
    else
        warn "xmllint not found (libxml2); skipping XML well-formedness checks"
    fi
    if have appstreamcli; then
        # Informational only: md.lmdv trips cid-desktopapp-is-not-rdns until the
        # project picks a three-component id (packaging/README.md).
        say "  appstreamcli validate (informational):"
        if ((DRY_RUN)); then
            say "  + appstreamcli validate --no-net $APPDIR/usr/share/metainfo/md.lmdv.metainfo.xml"
        else
            appstreamcli validate --no-net "$APPDIR/usr/share/metainfo/md.lmdv.metainfo.xml" 2>&1 |
                sed 's/^/    /' || true
        fi
    fi
}

if ((APPDIR_ONLY)); then
    validate_appdir
    if ((DRY_RUN)); then
        say ""
        say "dry run: would stop after staging and validating the AppDir"
    else
        say ""
        say "AppDir ready: $APPDIR"
        say "run it with: $APPDIR/AppRun --help   (or install the image with linuxdeploy)"
    fi
    exit 0
fi

# ------------------------------------------------------------ build the image
step "tools"
LINUXDEPLOY="$(tool_path linuxdeploy)"
APPIMAGETOOL="${APPIMAGETOOL:-$(tool_path appimagetool)}"
say "  linuxdeploy:   ${LINUXDEPLOY:-MISSING}"
say "  appimagetool:  ${APPIMAGETOOL:-MISSING}"
if [ -n "$LINUXDEPLOY" ]; then
    if linuxdeploy_has_appimage "$LINUXDEPLOY"; then
        say "  appimage plugin: yes"
    else
        say "  appimage plugin: NO"
    fi
fi
if [ -n "$RUNTIME_FILE" ]; then
    say "  runtime file:  $RUNTIME_FILE"
fi

build_image() {
    step "create $OUTPUT"
    run mkdir -p -- "$OUT_DIR"

    if [ -n "$LINUXDEPLOY" ] && linuxdeploy_has_appimage "$LINUXDEPLOY"; then
        local -a cmd=(env "ARCH=$ARCH" "VERSION=$APP_VERSION" "OUTPUT=$OUTPUT")
        [ -n "$UPDATE_INFO" ] && cmd+=("UPDATE_INFORMATION=$UPDATE_INFO")
        [ -n "$RUNTIME_FILE" ] && cmd+=("RUNTIME_FILE=$RUNTIME_FILE")
        cmd+=("$LINUXDEPLOY"
            --appdir "$APPDIR"
            --executable "$APPDIR/usr/bin/lmdv"
            --desktop-file "$APPDIR/usr/share/applications/lmdv.desktop"
            --icon-file "$APPDIR/usr/share/icons/hicolor/scalable/apps/lmdv.svg"
            --icon-filename lmdv
            --output appimage)
        say "  deploying shared libraries and building the image with linuxdeploy"
        run "${cmd[@]}"
    elif have_appimagetool; then
        say "  building the image with appimagetool (shared libraries are NOT bundled)"
        if [ -n "$UPDATE_INFO" ]; then
            run env "ARCH=$ARCH" "$APPIMAGETOOL" -u "$UPDATE_INFO" "$APPDIR" "$OUTPUT"
        else
            run env "ARCH=$ARCH" "$APPIMAGETOOL" "$APPDIR" "$OUTPUT"
        fi
    else
        die "no AppImage tool found. Install linuxdeploy with its appimage plugin (Arch: linuxdeploy-appimage) or appimagetool, or pass APPIMAGETOOL. Re-run with --dry-run to inspect the plan, or with --appdir-only to stage the AppDir without building an image."
    fi
}

if ((DRY_RUN)); then
    if [ -n "$LINUXDEPLOY" ] && linuxdeploy_has_appimage "$LINUXDEPLOY"; then
        build_image
    elif have_appimagetool; then
        build_image
    else
        say ""
        say "dry run: no AppImage tool installed; a real run would stop after staging with:"
        say "  error: no AppImage tool found. Install linuxdeploy with its appimage plugin"
        say "         (Arch: linuxdeploy-appimage) or appimagetool, or pass APPIMAGETOOL."
        say "         Use --appdir-only to stage the AppDir without an image."
    fi
    say ""
    say "dry run: nothing was written"
    exit 0
fi

validate_appdir
build_image

if [ -x "$OUTPUT" ]; then
    say ""
    say "built: $OUTPUT"
    say "integrate it with AppImageLauncher/appimaged, or install the staged desktop"
    say "file with its Exec rewritten to the absolute .AppImage path"
    say "(docs/LINUX_INTEGRATION.md §7.4)."
else
    say ""
    warn "the tool reported success but $OUTPUT is missing; check its output above"
fi
