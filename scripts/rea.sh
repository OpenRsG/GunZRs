#!/usr/bin/env bash
set -euo pipefail
root="$(dirname "$(dirname "$(realpath "${BASH_SOURCE[0]}")")")"
export REA_ANALYSIS_PROVIDER=ghidra
IFS= read -r GHIDRA_INSTALL_DIR < "$root/.local/ghidra-dir"
IFS= read -r JAVA_HOME < "$root/.local/java-home"
export GHIDRA_INSTALL_DIR JAVA_HOME
export PATH="$JAVA_HOME/bin:$PATH"
exec "$root/tools/node_modules/.bin/rea" "$@"
