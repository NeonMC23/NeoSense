#!/usr/bin/env bash
# NeoSense unified Linux installer and diagnostics entry point.
#
# Self-locating: it can be run from the repository, an extracted release
# archive, another mounted partition, or any current working directory. It only
# changes the currently running system and never performs physical fan tests.
set -euo pipefail

SELF="$(readlink -f "$0" 2>/dev/null || printf '%s' "$0")"
ROOT_DIR="$(cd -- "$(dirname -- "${SELF}")" && pwd)"

# shellcheck source=installer/lib/common.sh
. "${ROOT_DIR}/installer/lib/common.sh"
# shellcheck source=installer/lib/detect.sh
. "${ROOT_DIR}/installer/lib/detect.sh"
# shellcheck source=installer/lib/pkg.sh
. "${ROOT_DIR}/installer/lib/pkg.sh"
# shellcheck source=installer/lib/deps.sh
. "${ROOT_DIR}/installer/lib/deps.sh"
# shellcheck source=installer/lib/backend.sh
. "${ROOT_DIR}/installer/lib/backend.sh"
# shellcheck source=installer/lib/verify.sh
. "${ROOT_DIR}/installer/lib/verify.sh"
# shellcheck source=installer/lib/cli.sh
. "${ROOT_DIR}/installer/lib/cli.sh"

installer_main "$@"
