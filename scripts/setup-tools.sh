#!/usr/bin/env bash
set -euo pipefail
root="$(dirname "$(dirname "$(realpath "${BASH_SOURCE[0]}")")")"
npm ci --prefix "$root/tools" --ignore-scripts --no-audit --no-fund
bash "$root/scripts/setup-decompiler.sh" "$@"
node --input-type=module - "$root" <<'JS'
import fs from 'node:fs';
import path from 'node:path';
const [root] = process.argv.slice(2);
const local = path.join(root, '.local');
fs.mkdirSync(local, {recursive: true, mode: 0o700});
console.log('REA is configured for the local Ghidra/JDK 21 installation.');
JS
