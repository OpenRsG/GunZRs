# Native investigation system

## Active stack

OMP profile `reverse-engineering` -> REA 4.1.0 -> Ghidra 12.1.4 with 64-bit JDK 21.
`tools/package-lock.json` locks REA and the MCP client. The portable Temurin
21.0.12.1+1 archive and its vendor SHA-256 are pinned in the native installer.
Existing Ghidra projects are not adopted or modified by framework sessions.

`scripts/rea.sh` selects Ghidra explicitly from ignored local installation paths.
The project MCP deadline is 600 seconds. Missing or unsupported engines fail;
there is no automatic provider substitution or analysis retry loop.

## Measured generic checks

`bash scripts/check-framework.sh` completed with exit 0. Engine evidence is in
`.local/re/engine-check/run-Hph58C/report.json`. Query-wrapper evidence is in
`.local/re/query-check/run-d6evrY/report.json`; the final query regression check
completed with exit 0, including failure propagation and cache rejection.

| Observed check | Result |
| --- | --- |
| Generated ELF and x86-64 PE DLL multiply/add decompilation | pass |
| Function assembly, marker string/xrefs and typed references | pass |
| First function analysis | 5,259 ms ELF; 5,383 ms PE DLL |
| Warm same-session pseudocode | 6 ms ELF; 4 ms PE DLL |
| Exact CLI snapshot replay, no native launch in execve trace | 993 ms ELF; 947 ms PE DLL |
| Owned temporary projects/runtimes and MCP transport released | pass |
| Calculator CLI first query / replay | 7,472 ms / 1,003 ms |
| Independent codec CLI first query / replay | 7,384 ms / 978 ms |
| Explicit fresh query | new native analysis, 7,508 ms |
| Changed calculator bytes at the same filename | new SHA namespace, new analysis and changed multiply/add result |
| Original input hashes unchanged by investigation | pass |
| Unknown procedure | nonzero exit, `ok: false`, no reusable snapshot |

These are generated-fixture measurements, not benchmarks for arbitrary programs.
First-analysis figures exclude metadata open/Java discovery; CLI figures include
startup and process tracing. The ELF fixtures execute their own numeric assertions;
the generated PE DLL is statically analyzed and never executed.

In one recorded calculator run, concise output was 3,600 bytes while its paired
raw response was 39,128 bytes. Complete raw evidence remains available; the
display projection omits other dossier facets rather than claiming they are absent.

REA's full CLI envelope can report `ok: true` while its data contains an analysis
error and the process exits 1. `query.sh` normalizes process/analysis failures to
`ok: false`, preserves error details and never promotes them to the reusable cache.

## Coverage boundary

Verified on this Linux x64 host: x86-64 ELF and x86-64 Windows PE DLL native
analysis, plus independent ELF applications through the query wrapper. There is
no target/game-specific dependency. Other native formats, architectures, packed
or heavily obfuscated inputs remain unverified here. Managed, JavaScript, APK and
browser operations advertised by upstream REA are not certified by these checks.
Successful decompilation does not guarantee original source or runtime parity.

## Work without repeated imports

1. State one selected input, unresolved question and the evidence that settles it.
   Reuse saved findings and pinned public references before importing.
2. For one function, use `scripts/query.sh BINARY PROCEDURE`. It hashes the input,
   selects the question directory and lets REA reuse an exact snapshot. Stdout is
   a concise view; use the retained raw response for omitted dossier facets.
3. For multiple related questions, open the input once in MCP and keep that live
   session. Use bounded string -> xref -> function queries. `analyze_function`
   already contains several facets; request more only for a specific gap.
4. Preserve input identity, architecture/image base, exported analysis profile,
   exact arguments and limitations. Session run IDs/PIDs are ownership provenance,
   not cross-session cache keys. Do not maintain a competing manual cache.
5. Close owned sessions after saving evidence. Never search/delete other owners'
   temporary resources or modify unrelated manual projects.

The first native query imports and autoanalyzes. Warm queries and exact persisted
replay address different costs. Changed bytes choose a new question namespace;
profile/argument validity is checked by REA. `--fresh` creates a new observation
while retaining old run evidence. Unknown revision/profile freshness stays unknown.

## Evidence and safety

Current notes use `claim`, `status`, `source`, `checked`, `next`; raw observations
remain intact. Inputs and proprietary/decompiled data stay outside Git. Corrections
update the current summary with links to superseded evidence.

Static originals are read-only. Runtime execution, dynamic capture and network
interaction require explicit authorization. Decompiler processes run with user
permissions; temporary copies/read-only projects are not an OS security boundary.

## Workflow reference

Question-local evidence, explicit handoffs and short document routing are inspired
by `AGENT.md`, `CONTEXT.md` and `docs/INDEX.md` at reference revision
`f608f85e407ff1b7689d54a9aafdd16e95711ac4`. Test deletion, history resets and clone
machinery were not adopted.

- [Reference workflow](https://github.com/chasmlol/2010-rust-rewrite-mashup/tree/f608f85e407ff1b7689d54a9aafdd16e95711ac4)
- [REA Ghidra contract](https://github.com/morluto/rea#ghidra-analysis-provider)
- [Portable JDK release](https://github.com/adoptium/temurin21-binaries/releases/tag/jdk-21.0.12.1%2B1)
