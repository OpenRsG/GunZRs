#!/usr/bin/env bash
set -euo pipefail
root="$(dirname "$(dirname "$(realpath "${BASH_SOURCE[0]}")")")"
npm ci --prefix "$root/tools" --ignore-scripts --no-audit --no-fund
bash "$root/scripts/setup-decompiler.sh" "$@"
node --input-type=module - "$root" <<'JS'
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
const [root] = process.argv.slice(2);
const local = path.join(root, '.local');
fs.mkdirSync(local, {recursive: true, mode: 0o700});
const profile = path.join(os.homedir(), '.omp', 'profiles', 'reverse-engineering', 'agent');
fs.mkdirSync(profile, {recursive: true, mode: 0o700});
const profileFile = path.join(profile, 'mcp.json');
const config = fs.existsSync(profileFile)
  ? JSON.parse(fs.readFileSync(profileFile, 'utf8'))
  : {mcpServers: {}};
config.disabledServers = [...new Set([...(config.disabledServers ?? []), 'aisandbox'])];
fs.writeFileSync(profileFile, JSON.stringify(config, null, 2) + '\n');
console.log(`Reverse-engineering profile configured without AISandbox: ${profileFile}`);
console.log('REA is configured for the local Ghidra/JDK 21 installation.');
JS
