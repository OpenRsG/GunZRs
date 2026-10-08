#!/usr/bin/env bash
set -euo pipefail
root="$(dirname "$(dirname "$(realpath "${BASH_SOURCE[0]}")")")"
exec node "$root/tools/query.mjs" "$@"
