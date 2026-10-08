#!/usr/bin/env bash
set -euo pipefail
root="$(dirname "$(dirname "$(realpath "${BASH_SOURCE[0]}")")")"
ghidra="${1:-${GHIDRA_INSTALL_DIR:-$HOME/tools/ghidra_12.1.4_PUBLIC}}"
java_home="${2:-$root/.tools/jdk-21}"
if [[ ! -x "$ghidra/support/analyzeHeadless" ]] || ! grep -qx 'application.version=12.1.4' "$ghidra/Ghidra/application.properties"; then
    printf '%s\n' 'Select an extracted Ghidra 12.1.4 directory as the first argument.' >&2
    exit 1
fi
if [[ ! -x "$java_home/bin/java" || ! -x "$java_home/bin/javac" ]]; then
    if [[ -n "${2:-}" ]]; then
        printf '%s\n' 'Selected JAVA_HOME must contain java and javac.' >&2
        exit 1
    fi
    mkdir -p "$java_home"
    archive="$root/.tools/temurin-jdk-21.tar.gz"
    url='https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jdk_x64_linux_hotspot_21.0.12.1_1.tar.gz'
    checksum='ce79869e1307ed8ee1e2baa86a412b1eb5b75d10a01006d788a6f968bcfaee94'
    curl --fail --location --silent --show-error --output "$archive" "$url"
    printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check -
    tar -xzf "$archive" --strip-components=1 -C "$java_home"
fi
strace="$(command -v strace || true)"
if [[ -z "$strace" ]]; then
    strace="$root/.tools/strace/usr/bin/strace"
    if [[ ! -x "$strace" ]]; then
        mkdir -p "$root/.tools/strace"
        url="$(pacman -Sddp --print-format '%l' strace)"
        if [[ "$url" != https://* || "$url" == *$'\n'* ]]; then
            printf '%s\n' 'Expected one HTTPS strace package URL from pacman.' >&2
            exit 1
        fi
        archive="$root/.tools/strace.pkg.tar.zst"
        curl --fail --location --silent --show-error --output "$archive" "$url"
        curl --fail --location --silent --show-error --output "$archive.sig" "$url.sig"
        pacman-key --verify "$archive.sig" "$archive"
        bsdtar -xf "$archive" -C "$root/.tools/strace"
    fi
fi
node --input-type=module - "$root" "$ghidra" "$java_home" "$strace" <<'JS'
import fs from 'node:fs';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
const [root, selectedGhidra, selectedJava, selectedTrace] = process.argv.slice(2);
const ghidra = fs.realpathSync(selectedGhidra);
const java = fs.realpathSync(selectedJava);
const version = spawnSync(path.join(java, 'bin/java'), ['-version'], {encoding: 'utf8'});
const compiler = spawnSync(path.join(java, 'bin/javac'), ['-version'], {encoding: 'utf8'});
if (version.status !== 0 || !/version "21\./.test(version.stderr) || compiler.status !== 0 || !/^javac 21\./.test(compiler.stdout)) {
  throw new Error(`REA 4.1.0 requires JDK 21: ${version.stderr} ${compiler.stdout} ${compiler.stderr}`);
}
const local = path.join(root, '.local');
fs.mkdirSync(local, {recursive: true, mode: 0o700});
fs.chmodSync(local, 0o700);
fs.writeFileSync(path.join(local, 'ghidra-dir'), ghidra + '\n');
fs.writeFileSync(path.join(local, 'java-home'), java + '\n');
fs.writeFileSync(path.join(local, 'strace-path'), fs.realpathSync(selectedTrace) + '\n');
console.log(`Ghidra 12.1.4: ${ghidra}`);
console.log(`JDK: ${compiler.stdout.trim()} at ${java}`);
JS
