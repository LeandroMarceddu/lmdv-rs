#!/usr/bin/env bash
# install-md-handler.sh -- register the current lmdv build as the Markdown handler.
#
# Per-user only: never needs sudo, never writes outside $HOME / the XDG dirs.
# Safe to re-run: every file it writes carries a marker and is replaced whole.
#
# Examples:
#   scripts/install-md-handler.sh --release
#   scripts/install-md-handler.sh --dev --dry-run
#   scripts/install-md-handler.sh --binary ./target/debug/lmdv
#   scripts/install-md-handler.sh --uninstall
set -euo pipefail

DESKTOP_ID="lmdv.desktop"
MIME_TYPES=(text/markdown text/x-markdown application/x-markdown)
MARKER="X-LMDV-Managed=true"
WRAPPER_MARKER="# lmdv-launcher v1"

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
APP_DIR="$DATA_HOME/applications"
MIME_DIR="$DATA_HOME/mime/packages"
ICON_DIR="$DATA_HOME/icons/hicolor/scalable/apps"
BIN_DIR="${LMDV_BIN_DIR:-$HOME/.local/bin}"
STATE_DIR="$STATE_HOME/lmdv/install"
PREV_DEFAULTS="$STATE_DIR/previous-defaults.tsv"
WRAPPER="$BIN_DIR/lmdv"
DESKTOP_FILE="$APP_DIR/$DESKTOP_ID"
MIME_FILE="$MIME_DIR/lmdv.xml"
ICON_FILE="$ICON_DIR/lmdv.svg"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"

MODE="install"
DRY_RUN=0
DO_MIME=1
USE_WRAPPER=1
FORCE=0
PURGE=0
PROFILE="auto"
BINARY=""
BINARY_EXPLICIT=0
ALLOW_MISSING=0

say()  { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
usage: install-md-handler.sh [options]

  --dev               default the launcher to target/debug/lmdv
  --release           default the launcher to target/release/lmdv
                      (without either, the newest of the two wins at launch)
  --binary PATH       use this exact binary (LMDV_BIN still overrides)
  --repo PATH         project root (default: parent of this script)
  --prefix DIR        where the `lmdv` launcher goes (default: ~/.local/bin)
  --no-wrapper        point the desktop entry straight at the binary
  --no-mime           install the desktop entry but do not claim .md files
  --allow-missing     install even if no build exists yet
  --uninstall         remove everything this script installed, restore the
                      previous default handler
  --purge             with --uninstall, also delete ~/.local/state/lmdv
  --force             overwrite files that are not marked as ours
  --dry-run           print what would happen, change nothing
  -h, --help          this text
EOF
}

while (($#)); do
    case "$1" in
        --dev)           PROFILE="debug" ;;
        --release)       PROFILE="release" ;;
        --binary)        BINARY="${2:?--binary needs a path}"; BINARY_EXPLICIT=1; shift ;;
        --repo)          REPO_DIR="${2:?--repo needs a path}"; shift ;;
        --prefix)        BIN_DIR="${2:?--prefix needs a path}"; shift ;;
        --no-wrapper)    USE_WRAPPER=0 ;;
        --no-mime)       DO_MIME=0 ;;
        --allow-missing) ALLOW_MISSING=1 ;;
        --uninstall)     MODE="uninstall" ;;
        --purge)         PURGE=1 ;;
        --force)         FORCE=1 ;;
        --dry-run)       DRY_RUN=1 ;;
        -h|--help)       usage; exit 0 ;;
        *)               die "unknown option: $1 (try --help)" ;;
    esac
    shift
done

REPO_DIR="$(realpath -m -- "$REPO_DIR")"
BIN_DIR="$(realpath -m -- "$BIN_DIR")"
WRAPPER="$BIN_DIR/lmdv"

# --- helpers -----------------------------------------------------------------

run() {
    if ((DRY_RUN)); then
        printf '[dry-run] %s\n' "$*"
    else
        "$@"
    fi
}

# write_file PATH < content
write_file() {
    local path="$1"
    if ((DRY_RUN)); then
        printf '[dry-run] write %s\n' "$path"
        cat >/dev/null
        return 0
    fi
    mkdir -p -- "$(dirname -- "$path")"
    cat >"$path"
}

desktop_quote() {
    local p="$1"
    # A literal % is a field-code introducer and cannot appear in the program
    # path; $ and ` make desktop-file-validate and GLib disagree; quotes and
    # backslashes have two competing escape rules. Refuse the lot: the default
    # Exec path is ~/.local/bin/lmdv, so this only affects --no-wrapper.
    case "$p" in
        *'"'*|*'\'*|*'$'*|*'`'*|*'%'*|*$'\n'*) return 1 ;;
    esac
    # Leave a simple path unquoted: xdg-mime's desktop-file reader does not
    # strip the quotes from Exec, so a quoted path makes `xdg-mime query
    # default` fall through to another handler even though the desktop entry
    # is valid. Quote only when the path actually needs it.
    case "$p" in
        *' '*) printf '"%s"' "$p" ;;
        *) printf '%s' "$p" ;;
    esac
}

managed_file() { # path, marker
    [ -f "$1" ] && grep -Fq -- "$2" "$1" 2>/dev/null
}

require_ours_or_absent() { # path, marker-regex
    if [ -e "$1" ] && ! managed_file "$1" "$2"; then
        if ((FORCE)); then
            warn "overwriting unmanaged file: $1"
        else
            die "$1 exists and was not written by this script; re-run with --force to replace it"
        fi
    fi
}

pick_binary() {
    if [ -n "$BINARY" ]; then
        BINARY="$(realpath -m -- "$BINARY")"
        [ -x "$BINARY" ] || die "not executable: $BINARY"
        return 0
    fi
    local rel="$REPO_DIR/target/release/lmdv" dbg="$REPO_DIR/target/debug/lmdv"
    case "$PROFILE" in
        release) [ -x "$rel" ] && { BINARY="$rel"; return 0; } ;;
        debug)   [ -x "$dbg" ] && { BINARY="$dbg"; return 0; } ;;
    esac
    if [ -x "$rel" ] && [ -x "$dbg" ]; then
        if [ "$dbg" -nt "$rel" ]; then BINARY="$dbg"; else BINARY="$rel"; fi
        return 0
    fi
    [ -x "$rel" ] && { BINARY="$rel"; return 0; }
    [ -x "$dbg" ] && { BINARY="$dbg"; return 0; }
    return 1
}

# Remove `app` from every ;-list of a mimeapps.list, dropping empty lines.
mimeapps_remove() { # file, app-id
    local file="$1" app="$2" tmp
    [ -f "$file" ] || return 0
    grep -Fq -- "$app" "$file" || return 0
    tmp="$(mktemp)"
    awk -v drop="$app" '
        /^\[/ { sect = $0; print; next }
        {
            if (sect == "[Default Applications]" || sect == "[Added Associations]") {
                eq = index($0, "=")
                if (eq > 0) {
                    key = substr($0, 1, eq - 1)
                    n = split(substr($0, eq + 1), a, ";")
                    out = ""; found = 0
                    for (i = 1; i <= n; i++) {
                        if (a[i] == "") continue
                        if (a[i] == drop) { found = 1; continue }
                        out = out a[i] ";"
                    }
                    if (found) {
                        if (out != "") print key "=" out
                        next
                    }
                }
            }
            print
        }
    ' "$file" >"$tmp"
    if ((DRY_RUN)); then
        printf '[dry-run] prune %s from %s\n' "$app" "$file"
        rm -f -- "$tmp"
    else
        cat "$tmp" >"$file"
        rm -f -- "$tmp"
    fi
}

mimeapps_set() { # file, mime, app-id -- fallback when no xdg-mime/gio exist
    local file="$1" mime="$2" app="$3" old_d old_a new_d new_a
    if ((DRY_RUN)); then
        printf '[dry-run] set %s=%s in %s\n' "$mime" "$app" "$file"
        return 0
    fi
    mkdir -p -- "$(dirname -- "$file")"
    touch "$file"

    old_d="$(awk -v m="$mime" '
        /^\[Default Applications\]/ { s = 1; next }
        /^\[/ { s = 0 }
        s && index($0, m "=") == 1 { print substr($0, length(m) + 2); exit }' "$file")"
    old_a="$(awk -v m="$mime" '
        /^\[Added Associations\]/ { s = 1; next }
        /^\[/ { s = 0 }
        s && index($0, m "=") == 1 { print substr($0, length(m) + 2); exit }' "$file")"

    new_d="$(merge_list "$old_d" "$app" "")"
    new_a="$(merge_list "$old_a" "$app" ";")"

    awk -v m="$mime" -v dv="$new_d" -v av="$new_a" '
        /^\[Default Applications\]$/ { print; print m "=" dv; d = 1; next }
        /^\[Added Associations\]$/   { print; print m "=" av; a = 1; next }
        /^\[/                         { print; next }
        index($0, m "=") == 1          { next }
        { print }
        END {
            if (!d) { print ""; print "[Default Applications]"; print m "=" dv }
            if (!a) { print ""; print "[Added Associations]";   print m "=" av }
        }' "$file" >"$file.tmp.$$"
    mv -- "$file.tmp.$$" "$file"
}

merge_list() { # existing list, app, trailing ";" (or "")
    local old="$1" app="$2" trail="$3" part out
    local -a parts=()
    out="$app"
    IFS=';' read -r -a parts <<<"$old"
    for part in "${parts[@]}"; do
        [ -n "$part" ] || continue
        [ "$part" = "$app" ] && continue
        out="$out;$part"
    done
    if [ -n "$trail" ]; then out="$out;"; fi
    printf '%s' "$out"
}

set_default() { # mime
    local mime="$1"
    if ((DRY_RUN)); then
        printf '[dry-run] xdg-mime default %s %s\n' "$DESKTOP_ID" "$mime"
        return 0
    fi
    local set_ok=0
    if command -v xdg-mime >/dev/null 2>&1; then
        xdg-mime default "$DESKTOP_ID" "$mime" || true
        # Some xdg-mime builds silently refuse a type it cannot resolve in the
        # user's MIME database. Verify the choice; fall back to the file.
        if [ "$(xdg-mime query default "$mime" 2>/dev/null || true)" = "$DESKTOP_ID" ]; then
            set_ok=1
        fi
    fi
    if (( ! set_ok )) && command -v gio >/dev/null 2>&1; then
        if gio mime "$mime" "$DESKTOP_ID" >/dev/null 2>&1; then
            set_ok=1
        fi
    fi
    if (( ! set_ok )); then
        warn "could not set $mime with xdg-mime/gio; editing mimeapps.list directly"
        mimeapps_set "$CONFIG_HOME/mimeapps.list" "$mime" "$DESKTOP_ID"
    fi
}

current_default() { # mime -> desktop id or empty
    local mime="$1"
    if command -v xdg-mime >/dev/null 2>&1; then
        xdg-mime query default "$mime" 2>/dev/null || true
    elif command -v gio >/dev/null 2>&1; then
        gio mime "$mime" 2>/dev/null | sed -n 's/^Default application for .*: //p' | head -1
    else
        awk -v m="$mime" '
            /^\[Default Applications\]/ { s = 1; next }
            /^\[/ { s = 0 }
            s && index($0, m "=") == 1 { print substr($0, length(m) + 2); exit }' \
            "$CONFIG_HOME/mimeapps.list" 2>/dev/null || true
    fi
}

restore_default() { # mime
    local mime="$1" prev="" current=""
    if [ -f "$PREV_DEFAULTS" ]; then
        prev="$(awk -F'\t' -v m="$mime" '$1 == m { print $2; exit }' "$PREV_DEFAULTS")"
    fi
    current="$(current_default "$mime")"
    if [ -n "$current" ] && [ "$current" != "$DESKTOP_ID" ]; then
        mimeapps_remove "$CONFIG_HOME/mimeapps.list" "$DESKTOP_ID"
        mimeapps_remove "$DATA_HOME/applications/mimeapps.list" "$DESKTOP_ID"
        say "  leaving $mime -> $current (handler changed since install)"
        return 0
    fi
    if ((DRY_RUN)); then
        printf '[dry-run] restore %s -> %s\n' "$mime" "${prev:-<none>}"
        return 0
    fi
    if [ -n "$prev" ] && [ "$prev" != "$DESKTOP_ID" ]; then
        if command -v xdg-mime >/dev/null 2>&1; then
            xdg-mime default "$prev" "$mime" || warn "could not restore $mime to $prev"
        else
            mimeapps_set "$CONFIG_HOME/mimeapps.list" "$mime" "$prev"
        fi
        say "  restored $mime -> $prev"
    else
        local f
        for f in "$CONFIG_HOME/mimeapps.list" "$DATA_HOME/applications/mimeapps.list"; do
            mimeapps_remove "$f" "$DESKTOP_ID"
        done
        # Drop a now-empty key line left behind by xdg-mime.
        for f in "$CONFIG_HOME/mimeapps.list" "$DATA_HOME/applications/mimeapps.list"; do
            [ -f "$f" ] || continue
            sed -i -e '/^[A-Za-z0-9.+-]*\/[A-Za-z0-9.+-]*=$/d' -- "$f"
        done
        say "  cleared $mime"
    fi
}

refresh_caches() {
    if ! command -v update-mime-database >/dev/null 2>&1; then
        warn "update-mime-database not found (install shared-mime-info); .md detection may lag"
    elif [ -d "$DATA_HOME/mime" ]; then
        run update-mime-database "$DATA_HOME/mime" || warn "update-mime-database failed"
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        run update-desktop-database "$APP_DIR" || warn "update-desktop-database failed"
    else
        warn "update-desktop-database not found (install desktop-file-utils); Open With may not list lmdv"
    fi
    if [ -f "$DATA_HOME/icons/hicolor/index.theme" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
        run gtk-update-icon-cache -f -t "$DATA_HOME/icons/hicolor" >/dev/null 2>&1 || true
    fi
    # KDE reads desktop files through ksycoca; without a rebuild the new entry
    # can be missing from Dolphin's Open With until the next login.
    if [[ "${XDG_CURRENT_DESKTOP:-}" == *KDE* ]] && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
        if command -v kbuildsycoca6 >/dev/null 2>&1; then
            run timeout 30 kbuildsycoca6 --noincremental >/dev/null 2>&1 || warn "kbuildsycoca6 failed"
        elif command -v kbuildsycoca5 >/dev/null 2>&1; then
            run timeout 30 kbuildsycoca5 --noincremental >/dev/null 2>&1 || true
        fi
    fi
}

# --- uninstall ---------------------------------------------------------------

if [ "$MODE" = "uninstall" ]; then
    say "Removing the user-level lmdv Markdown handler"
    # Restore first, while the desktop file still exists: xdg-mime resolves the
    # current default through the desktop entry, and after deleting it the
    # query would fall through to another handler and look like a user change.
    if ((DO_MIME)); then
        for m in "${MIME_TYPES[@]}"; do
            restore_default "$m"
        done
    fi
    if managed_file "$WRAPPER" "$WRAPPER_MARKER"; then
        run rm -f -- "$WRAPPER"; say "  removed $WRAPPER"
    elif [ -e "$WRAPPER" ]; then
        warn "not removing $WRAPPER (not managed by this script; use --force to delete)"
        if ((FORCE)); then
            run rm -f -- "$WRAPPER"
            say "  removed $WRAPPER (forced)"
        fi
    fi
    if managed_file "$DESKTOP_FILE" "$MARKER"; then
        run rm -f -- "$DESKTOP_FILE"; say "  removed $DESKTOP_FILE"
    elif [ -e "$DESKTOP_FILE" ]; then
        warn "not removing $DESKTOP_FILE (not managed by this script)"
        if ((FORCE)); then
            run rm -f -- "$DESKTOP_FILE"
            say "  removed $DESKTOP_FILE (forced)"
        fi
    fi
    if managed_file "$MIME_FILE" "lmdv-managed"; then
        run rm -f -- "$MIME_FILE"; say "  removed $MIME_FILE"
    fi
    if managed_file "$ICON_FILE" "lmdv-managed"; then
        run rm -f -- "$ICON_FILE"; say "  removed $ICON_FILE"
    fi

    if ((DO_MIME)); then
        # Drop any stale lmdv entry from the added/default lists now that the
        # desktop file is gone; restore_default may have left it in "added".
        mimeapps_remove "$CONFIG_HOME/mimeapps.list" "$DESKTOP_ID"
        mimeapps_remove "$DATA_HOME/applications/mimeapps.list" "$DESKTOP_ID"
    fi
    refresh_caches
    if ((PURGE)); then
        run rm -rf -- "$STATE_HOME/lmdv"
        say "  removed $STATE_HOME/lmdv (--purge)"
    else
        say "  kept $STATE_DIR (use --purge to delete it)"
    fi
    say
    say "Verification:"
    for m in "${MIME_TYPES[@]}"; do
        printf '  %-24s -> %s\n' "$m" "$(command -v xdg-mime >/dev/null 2>&1 && xdg-mime query default "$m" 2>/dev/null || echo '(xdg-mime missing)')"
    done
    exit 0
fi

# --- install -----------------------------------------------------------------

((DRY_RUN)) || mkdir -p -- "$BIN_DIR" "$APP_DIR" "$STATE_DIR"

if ! pick_binary; then
    if ((DRY_RUN || ALLOW_MISSING)); then
        warn "no build found under $REPO_DIR/target; the launcher will work once you build"
        BINARY=""
    else
        die "no lmdv binary found. Build first: cargo build  (or cargo build --release), or pass --binary PATH"
    fi
fi

if ((USE_WRAPPER)); then
    EXEC_PATH="$WRAPPER"
else
    [ -n "$BINARY" ] || die "--no-wrapper needs a real binary; build first or pass --binary"
    EXEC_PATH="$BINARY"
fi

# Validate the Exec path before writing anything: some characters cannot be
# represented in a desktop entry in a way that both the validator and GLib accept.
if ! desktop_prog="$(desktop_quote "$EXEC_PATH")"; then
    die "Exec path contains one of \" \\ \$ \` % or a newline, which desktop entries cannot represent reliably: $EXEC_PATH (use the wrapper, or move the binary to a simple path)"
fi

say "Installing per-user Markdown handler for lmdv"
say "  repo:    $REPO_DIR"
say "  binary:  ${BINARY:-(none yet; launcher decides at run time)}"
say "  exec:    $EXEC_PATH"
((DRY_RUN)) && say "  mode:    dry run (nothing will be written)"

# 1. MIME package: merge aliases + globs into text/markdown at user level.
if ((DO_MIME)); then
    require_ours_or_absent "$MIME_FILE" "lmdv-managed"
    write_file "$MIME_FILE" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<!-- lmdv-managed: user-level Markdown type registration -->
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="text/markdown">
    <comment>Markdown document</comment>
    <glob pattern="*.md" weight="60"/>
    <glob pattern="*.markdown" weight="58"/>
    <glob pattern="*.mdown" weight="57"/>
    <glob pattern="*.mkd" weight="57"/>
    <glob pattern="*.mkdn" weight="57"/>
    <glob pattern="*.mdwn" weight="57"/>
    <alias type="text/x-markdown"/>
    <alias type="application/x-markdown"/>
  </mime-type>
</mime-info>
XML
    if ((DRY_RUN)); then say "  would write $MIME_FILE"; else say "  wrote $MIME_FILE"; fi
fi

# 2. Icon (copy the repo's if present, else a generated fallback).
require_ours_or_absent "$ICON_FILE" "lmdv-managed"
if [ -f "$REPO_DIR/assets/lmdv.svg" ]; then
    if ((DRY_RUN)); then
        say "[dry-run] copy $REPO_DIR/assets/lmdv.svg -> $ICON_FILE"
    else
        mkdir -p -- "$ICON_DIR"
        { printf '<!-- lmdv-managed -->\n'; cat -- "$REPO_DIR/assets/lmdv.svg"; } >"$ICON_FILE"
        say "  wrote $ICON_FILE (from assets/lmdv.svg)"
    fi
else
    write_file "$ICON_FILE" <<'SVG'
<!-- lmdv-managed -->
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="128" height="128">
  <rect x="10" y="10" width="108" height="108" rx="22" fill="#1f6feb"/>
  <path d="M36 28h38l18 18v54a6 6 0 0 1-6 6H36a6 6 0 0 1-6-6V34a6 6 0 0 1 6-6z"
        fill="#f6f8fa"/>
  <path d="M74 28l18 18H80a6 6 0 0 1-6-6z" fill="#c9d1d9"/>
  <path d="M44 88V58h8l8 12 8-12h8v30h-8V70l-8 12-8-12v18z" fill="#1f6feb"/>
</svg>
SVG
    if ((DRY_RUN)); then say "  would write $ICON_FILE (generated)"; else say "  wrote $ICON_FILE (generated)"; fi
fi

# 3. Desktop entry.
require_ours_or_absent "$DESKTOP_FILE" "$MARKER"

desktop_exec="$desktop_prog %f"
if ((DRY_RUN)); then
    say "[dry-run] write $DESKTOP_FILE (Exec=$desktop_exec)"
else
    mkdir -p -- "$APP_DIR"
    cat >"$DESKTOP_FILE" <<DESKTOP
[Desktop Entry]
Type=Application
Version=1.5
Name=lmdv
GenericName=Markdown Viewer
Comment=View and edit Markdown documents
Exec=$desktop_exec
TryExec=$EXEC_PATH
Icon=lmdv
Terminal=false
StartupNotify=false
StartupWMClass=lmdv
SingleMainWindow=true
DBusActivatable=false
NoDisplay=false
Categories=Utility;TextTools;
Keywords=markdown;md;view;preview;text;
MimeType=text/markdown;text/x-markdown;application/x-markdown;
$MARKER
DESKTOP
    if ((DRY_RUN)); then say "  would write $DESKTOP_FILE"; else say "  wrote $DESKTOP_FILE"; fi
fi

# 4. Launcher wrapper.
if ((USE_WRAPPER)); then
    require_ours_or_absent "$WRAPPER" "$WRAPPER_MARKER"
    WRAPPER_TEMPLATE=$(cat <<'WRAPPER_EOF'
#!/bin/bash
# lmdv-launcher v1 -- generated by install-md-handler.sh
# Managed file: re-running the installer overwrites it.
#
# Picks target/debug or target/release, absolutizes file arguments, logs to
# $XDG_STATE_HOME/lmdv/launch.log, never opens a terminal. Single-instance
# forwarding lives in the lmdv binary, not here.
set -u

LMDV_REPO="@LMDV_REPO@"
LMDV_PREFERRED_PROFILE="@LMDV_PROFILE@"
LMDV_FIXED_BIN="@LMDV_FIXED_BIN@"

SELF="$(readlink -f -- "${BASH_SOURCE[0]}" 2>/dev/null || printf '%s' "${BASH_SOURCE[0]}")"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
LOG_DIR="$STATE_HOME/lmdv"
LOG_FILE="$LOG_DIR/launch.log"
MAX_LOG_BYTES=262144

mkdir -p -- "$LOG_DIR" 2>/dev/null || true

log() {
    [ -d "$LOG_DIR" ] || return 0
    if [ -f "$LOG_FILE" ] && [ "$(stat -c %s -- "$LOG_FILE" 2>/dev/null || echo 0)" -gt "$MAX_LOG_BYTES" ]; then
        mv -f -- "$LOG_FILE" "$LOG_FILE.1" 2>/dev/null || true
    fi
    printf '%s [%s] %s\n' "$(date -u +%FT%TZ)" "$$" "$*" >>"$LOG_FILE" 2>/dev/null || true
}

gui_error() {
    local msg="$1"
    log "ERROR: $msg"
    [ -n "${LMDV_NO_DIALOG:-}" ] && return 0
    if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
        if command -v kdialog >/dev/null 2>&1 && [ "${XDG_CURRENT_DESKTOP:-}" != "GNOME" ]; then
            timeout 30 kdialog --title "lmdv" --error "$msg" >/dev/null 2>&1 && return 0
        fi
        if command -v zenity >/dev/null 2>&1; then
            timeout 30 zenity --error --title="lmdv" --text="$msg" >/dev/null 2>&1 && return 0
        fi
    fi
    if command -v notify-send >/dev/null 2>&1 && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
        notify-send -a lmdv -u critical "lmdv" "$msg" >/dev/null 2>&1 || true
    fi
    printf 'lmdv: %s\n' "$msg" >&2
}

normalize_args() {
    local -a src=("$@") out=()
    local a expect_value=0 end_of_opts=0
    for a in "${src[@]}"; do
        if [ "$end_of_opts" -eq 1 ]; then
            if [ -e "$a" ]; then out+=("$(realpath -m -- "$a")"); else out+=("$a"); fi
            continue
        fi
        if [ "$expect_value" -eq 1 ]; then out+=("$a"); expect_value=0; continue; fi
        case "$a" in
            --) out+=("$a"); end_of_opts=1 ;;
            --line) out+=("$a"); expect_value=1 ;;
            --line=*|-*|"") out+=("$a") ;;
            *) if [ -e "$a" ]; then out+=("$(realpath -m -- "$a")"); else out+=("$a"); fi ;;
        esac
    done
    printf '%s\0' "${out[@]}"
}

pick_bin() {
    local rel="$LMDV_REPO/target/release/lmdv"
    local dbg="$LMDV_REPO/target/debug/lmdv"
    local profile="${LMDV_PROFILE:-$LMDV_PREFERRED_PROFILE}"

    if [ -n "${LMDV_BIN:-}" ]; then printf '%s' "$LMDV_BIN"; return 0; fi
    if [ -n "$LMDV_FIXED_BIN" ] && [ -x "$LMDV_FIXED_BIN" ]; then printf '%s' "$LMDV_FIXED_BIN"; return 0; fi

    case "$profile" in
        release) [ -x "$rel" ] && { printf '%s' "$rel"; return 0; } ;;
        debug)   [ -x "$dbg" ] && { printf '%s' "$dbg"; return 0; } ;;
    esac
    if [ -x "$rel" ] && [ -x "$dbg" ]; then
        if [ "$dbg" -nt "$rel" ]; then printf '%s' "$dbg"; else printf '%s' "$rel"; fi
        return 0
    fi
    [ -x "$rel" ] && { printf '%s' "$rel"; return 0; }
    [ -x "$dbg" ] && { printf '%s' "$dbg"; return 0; }
    return 1
}

main() {
    local bin profile
    profile="${LMDV_PROFILE:-$LMDV_PREFERRED_PROFILE}"

    if ! bin="$(pick_bin)"; then
        gui_error "No lmdv binary found. Build it with 'cargo build' (debug) or 'cargo build --release', or set LMDV_BIN."
        exit 127
    fi
    if [ ! -x "$bin" ]; then
        gui_error "Selected binary is not executable: $bin"
        exit 126
    fi
    if [ "$(readlink -f -- "$bin" 2>/dev/null)" = "$SELF" ]; then
        gui_error "Refusing to launch the launcher itself ($bin). Set LMDV_BIN to the real binary."
        exit 126
    fi

    local -a args=()
    mapfile -d '' -t args < <(normalize_args "$@")

    log "profile=$profile bin=$bin argc=${#args[@]} args=$(printf '[%s]' "${args[@]}")"

    # Cargo rewrites target/debug/lmdv in place; a concurrent launch can hit
    # ETXTBSY (exit 126). Retry briefly instead of showing an error.
    local rc attempt=0
    while :; do
        "$bin" "${args[@]}"
        rc=$?
        if [ "$rc" -eq 126 ] && [ "$attempt" -lt 10 ]; then
            sleep 0.1
            attempt=$((attempt + 1))
            continue
        fi
        break
    done

    [ "$rc" -ne 0 ] && log "exit=$rc bin=$bin"
    exit "$rc"
}

main "$@"
WRAPPER_EOF
)
    WRAPPER_TEMPLATE="${WRAPPER_TEMPLATE//@LMDV_REPO@/$REPO_DIR}"
    WRAPPER_TEMPLATE="${WRAPPER_TEMPLATE//@LMDV_PROFILE@/$PROFILE}"
    FIXED_BIN=""
    if ((BINARY_EXPLICIT)); then FIXED_BIN="$BINARY"; fi
    WRAPPER_TEMPLATE="${WRAPPER_TEMPLATE//@LMDV_FIXED_BIN@/$FIXED_BIN}"

    if ((DRY_RUN)); then
        say "[dry-run] write $WRAPPER"
    else
        mkdir -p -- "$BIN_DIR"
        printf '%s\n' "$WRAPPER_TEMPLATE" >"$WRAPPER"
        chmod 0755 -- "$WRAPPER"
        say "  wrote $WRAPPER"
    fi
fi

refresh_caches

# 5. Default handler, remembering what it was the first time.
if ((DO_MIME)); then
    if ((DRY_RUN)); then
        say "[dry-run] save previous handlers in $PREV_DEFAULTS"
    else
        mkdir -p -- "$STATE_DIR"
        if [ ! -f "$PREV_DEFAULTS" ]; then
            for m in "${MIME_TYPES[@]}"; do
                prev=""
                if command -v xdg-mime >/dev/null 2>&1; then
                    prev="$(xdg-mime query default "$m" 2>/dev/null || true)"
                elif command -v gio >/dev/null 2>&1; then
                    prev="$(gio mime "$m" 2>/dev/null | sed -n 's/^Default application for .*: //p' | head -1)"
                fi
                if [ "$prev" = "$DESKTOP_ID" ]; then prev=""; fi
                printf '%s\t%s\n' "$m" "$prev"
            done >"$PREV_DEFAULTS"
            say "  saved previous handlers in $PREV_DEFAULTS"
        fi
    fi
    for m in "${MIME_TYPES[@]}"; do
        set_default "$m"
    done
fi

# 6. Verification checklist.
say
say "Installed. Verify with:"
printf '  %-24s %s\n' "desktop file:" "$DESKTOP_FILE"
printf '  %-24s %s\n' "launcher:" "$EXEC_PATH"
printf '  %-24s %s\n' "icon:" "$ICON_FILE"
if command -v desktop-file-validate >/dev/null 2>&1 && [ -f "$DESKTOP_FILE" ]; then
    if desktop-file-validate "$DESKTOP_FILE"; then
        printf '  %-24s %s\n' "desktop-file-validate:" "clean"
    else
        printf '  %-24s %s\n' "desktop-file-validate:" "WARNINGS (see above)"
    fi
fi
for m in "${MIME_TYPES[@]}"; do
    if command -v xdg-mime >/dev/null 2>&1; then
        printf '  default %-16s -> %s\n' "$m" "$(xdg-mime query default "$m" 2>/dev/null || echo '?')"
    fi
done
cat <<'CHECK'

Double-click a .md file, or run:
  printf '# hello\n' > "/tmp/lmdv test ünïcode.md"
  gio open "/tmp/lmdv test ünïcode.md"
  xdg-mime query filetype "/tmp/lmdv test ünïcode.md"
  tail -5 "${XDG_STATE_HOME:-$HOME/.local/state}/lmdv/launch.log"
CHECK
