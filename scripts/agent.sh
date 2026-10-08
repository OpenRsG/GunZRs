#!/usr/bin/env bash
set -euo pipefail
root="$(dirname "$(dirname "$(realpath "${BASH_SOURCE[0]}")")")"
export OMP_PROFILE=reverse-engineering
exec omp --profile reverse-engineering --cwd "$root" "$@"
