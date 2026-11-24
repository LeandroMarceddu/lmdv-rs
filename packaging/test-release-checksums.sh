#!/usr/bin/env bash
# Verify the release workflow's YAML and its checksum contract.
#
# Checks:
#   * .github/workflows/release.yml parses and has the linux/windows/publish jobs;
#   * the publish job generates SHA256SUMS after downloading the artifacts and
#     before creating the release;
#   * that step, run against a fake dist/ holding the assets the workflow
#     publishes (the four binaries plus the public release key), writes one
#     sorted sha256sum line per file -- exactly dist/*,
#     with no AppDir directory, no SHA256SUMS row and no extra rows -- and the
#     result verifies with `sha256sum -c` and is stable across runs;
#   * the signing step exists after the checksum and uses the GPG secrets, and
#     the release step publishes dist/* (so SHA256SUMS and SHA256SUMS.asc upload).
#
# Requires python3 + PyYAML. Without them it prints SKIP and exits 0; install
# with `python3 -m pip install --user pyyaml` (or the distro package).
#
# Usage: packaging/test-release-checksums.sh [--keep]
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo="$(cd -- "$here/.." && pwd -P)"
workflow="$repo/.github/workflows/release.yml"
keep=0
for arg in "$@"; do
    case "$arg" in
        --keep) keep=1 ;;
        -h|--help) sed -n '2,19p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) printf 'unknown option: %s\n' "$arg" >&2; exit 2 ;;
    esac
done

base="${LMDV_TEST_TMP:-/tmp/opencode}"
mkdir -p -- "$base"
root="$(mktemp -d -- "$base/lmdv-release-test.XXXXXX")"

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

if ! command -v python3 >/dev/null 2>&1; then
    printf 'SKIP: python3 is not installed\n'
    exit 0
fi
if ! python3 -c 'import yaml' >/dev/null 2>&1; then
    printf 'SKIP: PyYAML is not installed (python3 -m pip install --user pyyaml)\n'
    exit 0
fi

python3 - "$workflow" "$root/checksum-step.sh" <<'PY' || exit 1
import sys
import yaml

path, out = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as handle:
    doc = yaml.safe_load(handle)
if not isinstance(doc, dict) or not isinstance(doc.get("jobs"), dict):
    raise SystemExit("FAIL: workflow has no jobs mapping")
jobs = doc["jobs"]
for name in ("linux", "windows", "publish"):
    if name not in jobs:
        raise SystemExit(f"FAIL: missing job {name}")
steps = jobs["publish"].get("steps")
if not isinstance(steps, list):
    raise SystemExit("FAIL: publish has no steps")
names = [step.get("name", "") for step in steps]
for wanted in ("Download artifacts", "Generate SHA256SUMS", "Sign SHA256SUMS", "Create release"):
    if wanted not in names:
        raise SystemExit(f"FAIL: publish has no step named {wanted!r}")
download, generate, sign, create = (names.index(n) for n in
                                    ("Download artifacts", "Generate SHA256SUMS", "Sign SHA256SUMS", "Create release"))
if not download < generate < sign < create:
    raise SystemExit("FAIL: checksum/signing steps are not between download and create")
run = steps[generate].get("run", "")
for token in ("sha256sum", "SHA256SUMS", "-maxdepth 1"):
    if token not in run:
        raise SystemExit(f"FAIL: checksum step does not mention {token!r}")
files = steps[create].get("with", {}).get("files")
if files != "dist/*":
    raise SystemExit(f"FAIL: release files is {files!r}, expected 'dist/*'")
sign_run = steps[sign].get("run", "")
for token in ("SHA256SUMS.asc", "gpg", "--detach-sign"):
    if token not in sign_run:
        raise SystemExit(f"FAIL: signing step does not mention {token!r}")
if not steps[sign].get("env", {}).get("GPG_PRIVATE_KEY"):
    raise SystemExit("FAIL: signing step does not read GPG_PRIVATE_KEY")
with open(out, "w", encoding="utf-8") as handle:
    handle.write(run)
print(f"ok: workflow parses; checksum step {generate + 1} and signing step {sign + 1} of {len(steps)}")
PY

# --- simulate the publish job's dist/ -----------------------------------------

work="$root/work"
runner="$root/runner"
mkdir -p "$work/dist/AppDir/usr/bin" "$runner"
# The names follow the current release shape; the checksum step is version-agnostic.
assets=(
    "lmdv-0.10.0-x86_64"
    "lmdv-0.10.0-x86_64.AppImage"
    "lmdv-0.10.0-installer.exe"
    "lmdv-0.10.0-windows-x86_64.exe"
    "lmdv-release-key.asc"
)
for asset in "${assets[@]}"; do
    printf 'contents of %s\n' "$asset" >"$work/dist/$asset"
done
printf 'staged AppDir is not a release asset\n' >"$work/dist/AppDir/usr/bin/lmdv"

(
    cd "$work"
    RUNNER_TEMP="$runner" bash "$root/checksum-step.sh" >/dev/null
) || fail "the checksum step failed on a fake dist"

[ -f "$work/dist/SHA256SUMS" ] || fail "no SHA256SUMS was written"
[ "$(wc -l <"$work/dist/SHA256SUMS")" -eq "${#assets[@]}" ] ||
    fail "SHA256SUMS has $(wc -l <"$work/dist/SHA256SUMS") rows, expected ${#assets[@]}"

printf '%s\n' "${assets[@]}" | LC_ALL=C sort >"$root/expected"
awk '{ print $2 }' "$work/dist/SHA256SUMS" | LC_ALL=C sort >"$root/actual"
cmp -s "$root/expected" "$root/actual" ||
    { diff -u "$root/expected" "$root/actual" >&2; fail "SHA256SUMS rows are not the dist files"; }
if grep -Fq 'AppDir' "$work/dist/SHA256SUMS"; then
    fail "SHA256SUMS includes the staged AppDir"
fi
if grep -Fq 'SHA256SUMS' "$work/dist/SHA256SUMS"; then
    fail "SHA256SUMS includes itself"
fi
(cd "$work/dist" && sha256sum -c SHA256SUMS >/dev/null) || fail "sha256sum -c failed"
ok "SHA256SUMS covers exactly dist/* (${#assets[@]} files) and verifies"

cp "$work/dist/SHA256SUMS" "$root/first"
(
    cd "$work"
    RUNNER_TEMP="$runner" bash "$root/checksum-step.sh" >/dev/null
) || fail "the second checksum run failed"
cmp -s "$root/first" "$work/dist/SHA256SUMS" || fail "SHA256SUMS is not stable across runs"
ok "SHA256SUMS is stable across runs"

sed -n '1,2p' "$work/dist/SHA256SUMS"
printf '\nall release checksum checks passed\n'
