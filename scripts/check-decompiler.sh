#!/usr/bin/env bash
set -euo pipefail
umask 077
root="$(dirname "$(dirname "$(realpath "${BASH_SOURCE[0]}")")")"
probe="$(mktemp -d "${TMPDIR:-/tmp}/re-decompiler-probe.XXXXXX")"
trap 'rm -rf -- "$probe"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
cat > "$probe/probe.c" <<'C'
#ifdef _WIN32
#define PROBE_EXPORT __declspec(dllexport)
#else
#include <assert.h>
#define PROBE_EXPORT
#endif
static const char re_probe_marker[] = "re_probe_marker";
PROBE_EXPORT __attribute__((noinline)) int re_probe(int x) {
    const volatile char *marker = re_probe_marker;
    int result = x * 3 + 7;
    if (*marker != 'r') result = -1;
    return result;
}
#ifndef _WIN32
int main(void) {
    assert(re_probe(11) == 40);
    return 0;
}
#endif
C
step() {
    local label="$1" start status
    shift
    start="$(date +%s%3N)"
    if timeout --kill-after=5s 60s "$@"; then status=0; else status=$?; fi
    elapsed="$(( $(date +%s%3N) - start ))"
    if (( status == 0 )); then
        printf 'PASS %s (%s ms)\n' "$label" "$elapsed"
    else
        printf 'FAIL %s (%s ms, exit %s)\n' "$label" "$elapsed" "$status" >&2
        return "$status"
    fi
}
step 'ELF compile (-O0, known source)' "${CC:-cc}" -O0 -g -fno-inline -o "$probe/probe" "$probe/probe.c"
elf_compile_ms="$elapsed"
step 'ELF execute (assert re_probe(11) == 40)' "$probe/probe"
elf_run_ms="$elapsed"
step 'PE x86-64 compile (known source, no CRT)' "${CLANG:-clang}" --target=x86_64-pc-windows-msvc -O0 -fno-inline -fno-stack-protector -c "$probe/probe.c" -o "$probe/probe.obj"
pe_compile_ms="$elapsed"
step 'PE DLL link (/dll /noentry /nodefaultlib; not executed)' "${LLD_LINK:-lld-link}" /dll /noentry /nodefaultlib /machine:x64 "/out:$probe/probe.dll" "$probe/probe.obj"
pe_link_ms="$elapsed"
source_sha256="$(sha256sum "$probe/probe.c")"
node "$root/tools/check-decompiler.mjs" "$probe/probe" "$probe/probe.dll" "$probe/probe.c" "${source_sha256%% *}" "$elf_compile_ms" "$elf_run_ms" "$pe_compile_ms" "$pe_link_ms"
