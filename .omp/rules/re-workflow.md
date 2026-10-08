---
description: Target-independent, question-first native investigation using warm REA/Ghidra sessions and exact cached evidence.
---
# Native investigation workflow

## Select the question
- Select one input and one unresolved contract. State the consumer, inputs/outputs and evidence that would settle it. Stop when the question is answered at the claimed evidence level.
- Static inspection is authorized only for the selected target. Do not execute an unknown target or capture its runtime merely because decompilation is available.
- Read relevant existing findings, format specifications and public references before importing. Pin reference URL + revision + file/range; reference implementations are leads, not measured target behavior.
- Use `scripts/agent.sh` and the `reverse-engineering` profile. Use actual REA schemas and capabilities; do not invent tool names or add a second MCP.

## Choose the short path
- For one function, `scripts/query.sh BINARY PROCEDURE` stores evidence and selects an exact snapshot automatically. Use `--fresh` only for an intentional new observation.
- For several related queries, keep one MCP session open. Hash/open once, reuse returned facets, and close after the question. Repeated CLI cold starts are the slow path.
- Follow bounded string -> xref -> containing function queries. Inspect only required callers/callees/data edges; expand for a named unresolved dependency rather than dumping every function.
- `analyze_function` includes pseudocode, assembly, references and API facts when supported. Do not request those again unless a required facet is missing, truncated or changed.
- The first native query imports and autoanalyzes; metadata open, doctor and handshake do not establish engine readiness. Measure first analysis separately from warm calls and persisted cache replay.

## Preserve identities and observations
- Use `.local/re/<input-sha256>/<question>/` for target evidence. Record architecture, image base, producer versions/profile and exact arguments. Multi-format framework checks retain all source/binary identities in their run report.
- REA owns the exact snapshot key. Preserve exported profiles and evidence IDs; session run IDs/PIDs are ownership provenance, not a cross-session cache key.
- Changed bytes/profile/arguments cannot reuse stale results. Retain old run evidence when requesting fresh analysis. After annotations/settings changes, do not assert freshness without matching profile/revision evidence.
- Preserve pagination, truncation, unsupported facets and unresolved indirect calls. An empty result means unknown unless the query's coverage proves absence.
- Triangulate field layout/type, initialization and use. Confirm width, signedness, stride, ownership/lifetime and effective data overrides. Plausible decompiler types and recovered algorithms remain inferred until independently checked.
- Preserve source units, coordinates, packing and transforms before choosing any reconstruction adapters. Do not confuse renderer conventions with target facts.

## Ownership and handoff
- One owner opens, queries and closes each session. Parallel workers receive separate questions and evidence paths. Never mutate another owner's findings or unrelated/manual projects.
- Save required evidence before closing; close removes owned temporary projects. No automatic retries, debugger, patches or arbitrary evaluation by default.
- Current notes record `claim`, `status`, `source`, `checked`, `next`. Correct the current summary with links to superseded raw evidence; do not rewrite the observation history.
- Keep raw decompiler output and proprietary data private. Commit only own-words, evidence-qualified specifications or implementations with retained source licenses.
- Generated fixtures prove framework integration, not every application, architecture or behavior. Report which inputs and checks ran and which coverage remains unverified.
- Keep meaningful regression checks and remove disposable runtime scaffolding. Do not import the reference project's test deletion, history resets or clone machinery.
- Notes-only changes need no build. Session owners issue engine queries; the integration owner runs changed-path verification after edits land.
