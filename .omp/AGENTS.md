# Gunz2Rust

Clean-room Rust/Bevy port of GunZ: The Duel (Steam). The game runtime lives in `src/` (`mrs` archives, `map` RS loader, Bevy viewer in `main.rs`); recovered formats are documented in `docs/formats.md`.
The bundled REA/Ghidra framework (`scripts/`, `tools/`) is for static questions about retail files. Start it through `bash scripts/agent.sh` for the isolated `reverse-engineering` profile. Sticky invariants live in `.omp/RULES.md`.
`Gunz.exe` is Themida-packed: static decompilation only reaches the unpacker stub, so prefer data-driven format recovery.

Load only the route needed for the current question:
- Native investigation: read `rule://re-workflow` (fallback `.omp/rules/re-workflow.md`) and the version-matched `tools/node_modules/rea-agents/skills/reverse-engineer-anything/SKILL.md`.
- Setup, commands and coverage: `README.md`.
- Engine pins, measured checks, session and snapshot behavior: `docs/reverse-engineering.md`.
- One native function from the terminal: `scripts/query.sh BINARY PROCEDURE`; evidence and cache are selected by input identity and question, not an application name.
- Multi-step questions: use the REA MCP connection in one live session; `analyze_function` already contains several facets, so reuse them before issuing redundant queries.
- Tool implementation: `scripts/` and `tools/`. Use code intelligence when available and inspect actual upstream schemas before changing integration contracts.
- Source reuse and redistribution: `CREDITS.md` and the source's license at its pinned revision.
- Port code: `cargo run --release -- GAME_DIR [MAP]`; `cargo run --release --bin mrs -- extract GAME_DIR .local/extract` for a CRC-checked dump. Retail bytes and extracts stay in ignored `.local/`.

Reuse relevant findings first; reopen only changed sections, stale evidence or a new dependency. Notes-only changes need no build. The integration owner runs changed-path checks after executable changes land.
