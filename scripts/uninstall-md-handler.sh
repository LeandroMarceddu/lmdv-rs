#!/usr/bin/env bash
# uninstall-md-handler.sh -- remove the per-user lmdv Markdown handler and
# restore the handler that was default before installation.
#
# This is a thin wrapper: all logic (and all flags) live in
# install-md-handler.sh. Pass --dry-run to preview, --purge to delete the
# saved state, --force to remove files that the installer did not create.
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
exec bash "$here/install-md-handler.sh" --uninstall "$@"
