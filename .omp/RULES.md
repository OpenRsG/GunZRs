# Reverse-engineering invariants

- Operate on an explicitly selected target and question. Static inspection is the default; executing an unknown application, modifying it, capturing runtime behavior or performing network actions requires separate authorization. Generated fixtures may run for framework checks.
- AISandbox is forbidden, including in subagents. Use `scripts/agent.sh` and the isolated `reverse-engineering` profile. Do not change unrelated profiles.
- REA/Ghidra is the sole active native stack. No second MCP, speculative whole-program indexing or bulk decompilation without a stated question that requires it.
- Original targets are read-only. Keep proprietary bytes, extracts, dumps, databases, credentials and machine-local paths outside Git.
- Store raw evidence under ignored `.local/re/`, keyed by input SHA-256 and question. Generated multi-format checks use `.local/re/engine-check/run-*/`. One owner per session/project and shared file; preserve unrelated projects and other owners' evidence.
- Label claims observed, inferred, approximate, unknown or refuted. Record input identity, architecture, image base and RVA. Static decompilation is inference about behavior; external code and synthetic fixtures do not prove target parity.
- Reuse live sessions and exact snapshots. REA owns snapshot validity; do not serve a result across changed bytes, profiles or query arguments. Explicit fresh analysis retains previous evidence.
- Preserve pagination, truncation and unsupported facets. An empty response proves absence only when coverage establishes it. Validate binary lengths, offsets, counts, versions and decompression limits in any decoder.
- Retain source licenses and credit. Do not copy proprietary decompiled code, retail addresses or meaningless decompiler names into a reconstructed runtime.
- Use the smallest real implementation. Keep behavioral regression checks; no speculative abstractions, fake fallbacks, automatic retry loops or compatibility shims.
- Protect the desktop. Prefer headless/static work; never steal focus or inject input into the user's desktop. The local decompiler runs with user permissions, not an OS sandbox boundary.
- Handoff includes the question, evidence path, checks performed, limits, unresolved gaps and next owner/action. Claims of success cite observed output; never certify a target or format that was not exercised.
