#!/usr/bin/env bash
set -euo pipefail
root="$(dirname "$(dirname "$(realpath "${BASH_SOURCE[0]}")")")"
bash "$root/scripts/check-decompiler.sh"
node "$root/tools/check-query.mjs"
