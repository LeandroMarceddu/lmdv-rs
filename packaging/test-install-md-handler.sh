#!/usr/bin/env bash
# Temp-HOME end-to-end test for scripts/install-md-handler.sh.
#
# Covers:
#   * S-08 -- the "New -> Markdown document" template in the XDG Templates
#     folder: creation, localized name, XDG_TEMPLATES_DIR, re-run, a changed
#     locale, an edited template, an unrecorded existing template, uninstall,
#     --no-template, --template-dir and the $HOME misconfiguration;
#   * OI-153 -- the icon fallback is the current application mark, and the
#     installed icon is assets/lmdv.svg when the checkout has it.
#
# Every write goes to a fresh temp HOME; the test never touches the real user
# configuration, needs no sudo and makes no network calls.
#
# Usage: packaging/test-install-md-handler.sh [--keep]
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo="$(cd -- "$here/.." && pwd -P)"
installer="$repo/scripts/install-md-handler.sh"
keep=0
for arg in "$@"; do
    case "$arg" in
        --keep) keep=1 ;;
        -h|--help) sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) printf 'unknown option: %s\n' "$arg" >&2; exit 2 ;;
    esac
done

[ -x "$installer" ] || { printf 'missing installer: %s\n' "$installer" >&2; exit 1; }
command -v sha256sum >/dev/null 2>&1 || { printf 'sha256sum is required\n' >&2; exit 1; }

base="${LMDV_TEST_TMP:-/tmp/opencode}"
mkdir -p -- "$base"
root="$(mktemp -d -- "$base/lmdv-handler-test.XXXXXX")"

cleanup() {
    local rc=$?
    if ((rc == 0)) && ((!keep)) && [ -z "${LMDV_TEST_KEEP:-}" ]; then
        rm -rf -- "$root"
    else
        printf 'temp root kept: %s\n' "$root"
    fi
}
trap cleanup EXIT

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
ok()   { printf 'ok: %s\n' "$*"; }

# run_installer HOME LANG [ARGS...]
run_installer() {
    local home="$1" lang="$2"
    shift 2
    env -i PATH="$PATH" HOME="$home" \
        XDG_CONFIG_HOME="$home/.config" \
        XDG_DATA_HOME="$home/.local/share" \
        XDG_STATE_HOME="$home/.local/state" \
        XDG_CACHE_HOME="$home/.cache" \
        LANG="$lang" XDG_CURRENT_DESKTOP=GNOME \
        bash "$installer" --no-mime --allow-missing "$@"
}

expect_empty() { # path
    [ -f "$1" ] || fail "missing file: $1"
    [ ! -s "$1" ] || fail "expected an empty template: $1 ($(wc -c <"$1") bytes)"
}

expect_no_file() { # path
    [ ! -e "$1" ] || fail "unexpected file: $1"
}

# --- 1. fresh HOME: localized template, empty, recorded -----------------------

home="$root/home-de"
mkdir -p "$home"
run_installer "$home" "de_DE.UTF-8" >"$root/install-de.log" 2>&1 ||
    { cat "$root/install-de.log" >&2; fail "install failed"; }
grep -Fq 'New -> Markdown document' "$root/install-de.log" ||
    fail "install does not report the template"
tpl="$home/Templates/Markdown-Dokument.md"
expect_empty "$tpl"
[ "$(stat -c %a -- "$tpl")" = "644" ] || fail "template mode is not 644: $(stat -c %a -- "$tpl")"
state="$home/.local/state/lmdv/install/template.tsv"
[ "$(sed -n '1p' "$state")" = "# lmdv-managed template v1" ] || fail "template state header"
[ "$(sed -n '2p' "$state")" = "$tpl" ] || fail "template state path"
ok "fresh install writes an empty localized template"

# Re-run: same file, no duplicate, still empty.
run_installer "$home" "de_DE.UTF-8" >"$root/reinstall-de.log" 2>&1 ||
    fail "re-install failed"
[ "$(find "$home/Templates" -maxdepth 1 -type f | wc -l)" -eq 1 ] ||
    fail "re-install created a second template"
expect_empty "$tpl"
ok "re-run is idempotent"

# A changed locale renames the template, but only an unedited empty one.
run_installer "$home" "it_IT.UTF-8" >"$root/install-it.log" 2>&1 ||
    fail "locale-change install failed"
expect_no_file "$tpl"
expect_empty "$home/Templates/Documento Markdown.md"
ok "a locale change replaces the empty template"

# --- 2. an edited template is the user's document ----------------------------

edited="$home/Templates/Documento Markdown.md"
printf '# belonging to the user\n' >"$edited"
run_installer "$home" "it_IT.UTF-8" >"$root/reinstall-edited.log" 2>&1 ||
    fail "install over an edited template failed"
grep -Fq 'leaving non-empty template' "$root/reinstall-edited.log" ||
    fail "install did not warn about the edited template"
grep -Fq 'belonging to the user' "$edited" || fail "edited template was overwritten"
run_installer "$home" "it_IT.UTF-8" --uninstall >"$root/uninstall-edited.log" 2>&1 ||
    fail "uninstall over an edited template failed"
[ -f "$edited" ] || fail "uninstall deleted an edited template"
grep -Fq 'not removing' "$root/uninstall-edited.log" || fail "uninstall did not warn"
[ ! -f "$state" ] || fail "uninstall kept the template state after leaving an edited file"
ok "an edited template survives re-install and uninstall"

# Emptying it does not let a later install claim it: uninstall dropped the
# record, so the file is unmanaged until --force says otherwise.
: >"$edited"
run_installer "$home" "it_IT.UTF-8" >"$root/install-again.log" 2>&1 ||
    fail "install after emptying the template failed"
grep -Fq 'not recorded' "$root/install-again.log" ||
    fail "install claimed an unrecorded template with an empty file"
[ ! -f "$state" ] || fail "re-install recorded an unrecorded template"
run_installer "$home" "it_IT.UTF-8" --force >"$root/install-force.log" 2>&1 ||
    fail "--force install failed"
[ -f "$state" ] || fail "--force did not record the template"
run_installer "$home" "it_IT.UTF-8" --uninstall >"$root/uninstall.log" 2>&1 ||
    fail "uninstall failed"
expect_no_file "$edited"
[ -d "$home/Templates" ] || fail "uninstall removed the Templates folder"
ok "uninstall removes only the empty template and keeps the folder"

# --- 3. --no-template and --template-dir -------------------------------------

home="$root/home-notemplate"
mkdir -p "$home"
run_installer "$home" "en_US.UTF-8" --no-template >"$root/no-template.log" 2>&1 ||
    fail "--no-template install failed"
grep -Fq 'skipped the new-document template' "$root/no-template.log" ||
    fail "--no-template said nothing"
expect_no_file "$home/Templates/Markdown document.md"
[ ! -f "$home/.local/state/lmdv/install/template.tsv" ] || fail "--no-template recorded state"

home="$root/home-templatedir"
mkdir -p "$home"
run_installer "$home" "en_US.UTF-8" --template-dir "$home/custom templates" \
    >"$root/template-dir.log" 2>&1 || fail "--template-dir install failed"
custom="$home/custom templates/Markdown document.md"
expect_empty "$custom"
expect_no_file "$home/Templates/Markdown document.md"
ok "--no-template and --template-dir are honored"

# --template-dir must refuse $HOME itself (writing there would scatter a file
# among the user's own).
if run_installer "$home" "en_US.UTF-8" --template-dir "$home" >"$root/home-dir.log" 2>&1; then
    fail "--template-dir $HOME was accepted"
fi
grep -Fq 'must not be the home folder itself' "$root/home-dir.log" ||
    fail "--template-dir $HOME failed without the expected message"
ok "--template-dir refuses the home folder"

# --- 4. XDG_TEMPLATES_DIR and the broken $HOME setting -----------------------

home="$root/home-userdirs"
mkdir -p "$home/.config"
# shellcheck disable=SC2016 # user-dirs.dirs stores a literal $HOME
printf 'XDG_TEMPLATES_DIR="$HOME/Vorlagen"\n' >"$home/.config/user-dirs.dirs"
run_installer "$home" "de_DE.UTF-8" >"$root/userdirs.log" 2>&1 ||
    fail "user-dirs.dirs install failed"
expect_empty "$home/Vorlagen/Markdown-Dokument.md"
ok "XDG_TEMPLATES_DIR from user-dirs.dirs is used"

home="$root/home-broken-userdirs"
mkdir -p "$home/.config"
# shellcheck disable=SC2016 # the broken KDE-style value this test reproduces
printf 'XDG_TEMPLATES_DIR="$HOME/"\n' >"$home/.config/user-dirs.dirs"
run_installer "$home" "en_US.UTF-8" >"$root/broken-userdirs.log" 2>&1 ||
    fail "the broken XDG_TEMPLATES_DIR made the install fail"
grep -Fq 'skipping the new-document template' "$root/broken-userdirs.log" ||
    fail "the broken XDG_TEMPLATES_DIR did not warn"
[ -z "$(find "$home" -maxdepth 1 -type f -name '*.md')" ] ||
    fail "the broken XDG_TEMPLATES_DIR wrote a file into home"
ok "a templates folder pointing at \$HOME is refused"

# --- 5. an existing unrecorded template is never claimed ---------------------

home="$root/home-existing"
mkdir -p "$home/Templates"
: >"$home/Templates/Markdown document.md"
run_installer "$home" "en_US.UTF-8" >"$root/existing.log" 2>&1 ||
    fail "install with an existing template failed"
[ ! -f "$home/.local/state/lmdv/install/template.tsv" ] ||
    fail "install claimed a template it did not create"
run_installer "$home" "en_US.UTF-8" --uninstall >/dev/null 2>&1 || fail "uninstall failed"
[ -f "$home/Templates/Markdown document.md" ] || fail "uninstall deleted an unrecorded template"
ok "an existing unrecorded template is left alone"

# --- 6. --dry-run writes nothing ---------------------------------------------

home="$root/home-dry"
mkdir -p "$home"
run_installer "$home" "en_US.UTF-8" --dry-run >"$root/dry.log" 2>&1 || fail "dry run failed"
expect_no_file "$home/Templates/Markdown document.md"
[ ! -f "$home/.local/state/lmdv/install/template.tsv" ] || fail "dry run wrote state"
grep -Fq '[dry-run] write' "$root/dry.log" || fail "dry run listed no plan"
ok "dry run writes nothing"

# --- 7. icon: the checkout asset, and the current fallback -------------------

home="$root/home-icon"
mkdir -p "$home"
run_installer "$home" "en_US.UTF-8" >/dev/null 2>&1 || fail "icon install failed"
icon="$home/.local/share/icons/hicolor/scalable/apps/lmdv.svg"
[ -f "$icon" ] || fail "no icon installed"
tail -n +2 "$icon" >"$root/icon-body.svg"
cmp -s "$root/icon-body.svg" "$repo/assets/lmdv.svg" ||
    fail "the installed icon is not assets/lmdv.svg"
ok "the installed icon is the checkout asset"

# A copied installer with no assets must fall back to the same mark, not the
# old blue placeholder.
fake="$root/fake-repo"
mkdir -p "$fake/scripts"
cp -- "$installer" "$fake/scripts/install-md-handler.sh"
home="$root/home-fallback"
mkdir -p "$home"
env -i PATH="$PATH" HOME="$home" \
    XDG_CONFIG_HOME="$home/.config" \
    XDG_DATA_HOME="$home/.local/share" \
    XDG_STATE_HOME="$home/.local/state" \
    XDG_CACHE_HOME="$home/.cache" \
    LANG="en_US.UTF-8" XDG_CURRENT_DESKTOP=GNOME \
    bash "$fake/scripts/install-md-handler.sh" --no-mime --allow-missing \
    >"$root/fallback.log" 2>&1 || { cat "$root/fallback.log" >&2; fail "fallback install failed"; }
icon="$home/.local/share/icons/hicolor/scalable/apps/lmdv.svg"
[ -f "$icon" ] || fail "no fallback icon installed"
grep -Fq '#c98f4a' "$icon" || fail "fallback icon has no ochre bookmark"
grep -Fq '#191b1f' "$icon" || fail "fallback icon has no ink tile"
[ "$(grep -Fc '#1f6feb' "$icon" || true)" -eq 0 ] || fail "fallback icon is the old blue mark"
if command -v xmllint >/dev/null 2>&1; then
    xmllint --noout "$icon" || fail "fallback icon is not valid XML"
fi
ok "the fallback icon is the current mark"

printf '\nall installer checks passed\n'
