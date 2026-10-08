# Gunz2Rust

Rust/Bevy port of GunZ: The Duel (Steam), built on the GamePort2Rust
reverse-engineering framework. Requires a local retail install; no game data is
shipped or committed.

## Port

Install the game from Steam first: [GunZ: The Duel](https://store.steampowered.com/app/3139440/GunZ_The_Duel/)
(`steam://install/3139440` opens the Steam client directly). The port reads the installed
`.mrs` archives and never runs `Gunz.exe`. Set `GAME` to your install folder; the default
Steam library is `~/.steam/steam/steamapps/common/GUNZ THE DUEL` (native Steam),
the path below (Flatpak Steam), or `C:\Program Files (x86)\Steam\steamapps\common\GUNZ THE DUEL`
on Windows (Steam: right-click the game, Manage, Browse local files).

```sh
GAME="$HOME/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common/GUNZ THE DUEL"
cargo run --release --bin gunz-play -- "$GAME"                    # main menu (map, mode, bots, character, loadout)
cargo run --release --bin gunz-play -- "$GAME" Mansion --bots 3   # straight into a match
cargo run --release -- "$GAME" Mansion     # any folder in Maps.mrs, e.g. Castle, "Battle Arena"
cargo run --release --bin gunz-char -- "$GAME" man          # assembled character, bind pose
cargo run --release --bin gunz-anim -- "$GAME" man run      # character-XML animation name
cargo run --release --bin gunz-anim -- "$GAME" man run --type 2 --time 0.2 --upper attackS --pitch 20   # layered upper-body clip + aim pitch
cargo run --release --bin gunz-weapon -- "$GAME" "Raptor 50 RP" --on man --idle
cargo run --release --bin gunz-fx -- "$GAME" flame_rifle    # sfx/effect_list.xml name; --list
cargo run --release --bin mrs -- list "$GAME/system.mrs"
cargo run --release --bin mrs -- extract "$GAME" .local/extract   # CRC-checked dump
```

Main menu (no MAP given): pick a map, a mode (deathmatch, team DM, gladiator and
team gladiator = melee only, elimination = team rounds, assassinate = rounds with
a VIP per team, duel = one-on-one with a queue, training = dummy targets) with the
limits retail offers for it, bot count and skill on MATCH; character, outfit (170 man / 259
woman sets), loadout and mouse sensitivity with a 3D preview on PLAYER; START
launches the match. Game controls: mouse aims (cursor grabbed), WASD run, Space
jump (near a wall in the air: wall kick; jumping along a wall with W held: wall run), double-tap a direction to tumble, left
mouse attack, right mouse guard (melee), R reload, 1-5 / wheel switch weapon, Tab scoreboard, Esc pause
menu (resume, mouse sensitivity, return to menu, quit); dead in a round mode, Space or
click cycles the spectated player. A match ends at the time
or kill/round limit (default per mode, e.g. 10 min / 30 kills) with VICTORY / DEFEAT / DRAW, the
scoreboard, and play again / main menu / quit. Options: `--char man|woman`,
`--outfit N`, `--loadout ID,..`, `--bots N`, `--skill 0..1`, `--sens X`,
`--mode dm|tdm|gladiator|team-gladiator|elimination|assassinate|duel|training`,
`--time-limit S`, `--kill-limit N` (0 = none), `--respawn S`, `--protect S`,
`--round-time S`, `--ready S`; headless menu
shots: `--shot OUT.png --menu-page match|player`.
Bots route over a floor graph built from the map collision (stairs, jumps, drops, wall
climbs), switch weapon by range, guard and dash-slash with blades, fetch health/armour/ammo
pickups, retreat when hurt and respect `--skill`; team mode makes
them fight the other team (`RUST_LOG=gunz::bot=debug` logs their state).

Viewer controls: WASD, Space/C up/down, Shift fast, hold right mouse to look,
Esc quits. Every viewer accepts `--shot OUT.png` to render one 1280x720 frame
headlessly (no window); `gunz-play` also takes `--script` and `--time` for
reproducible runs (syntax in `src/bin/gunz-play.rs`). Working today: MRS
archives, all 30 RS v7 maps (plus the quest and challenge-quest maps, `gunz MAP`
takes any directory name) with lightmaps, skies and every `OBJECTLIST` prop
(fires, light shafts, water, fans, waving flags and curtains), map
collision (`.RS.col`), every retail `.elu`/`.elu.ani`, skinned characters,
weapons, sfx effects, a third-person deathmatch against bots with HUD and
sound. Movement constants and the cloth motion are inferred (not in the data).
Not yet: rockets/grenades/medikits, wall running, network.
Format notes: `docs/formats.md`.

# Reverse-engineering framework

Local, target-independent investigations with OMP, REA and Ghidra.

## Start

```sh
bash scripts/setup-tools.sh
bash scripts/agent.sh
```

The isolated OMP profile is `reverse-engineering`. Its existing profile data was
preserved during migration. Only REA is registered; AISandbox is denied.
Other profiles and target installations are unchanged.

## One function

```sh
bash scripts/query.sh /absolute/path/to/program function_name
bash scripts/query.sh /absolute/path/to/library.dll 0x180001000
```

The procedure is a symbol or address in the selected binary. These are usage
examples, not claims that those files or addresses exist.

The command hashes the actual input, records the question and selects a snapshot
under `.local/re/<sha256>/function-<question-hash>/`. Identical input/question can
replay saved evidence; changed bytes select a different namespace. REA validates
the actual provider/profile/query cache key. No target name or installation path
is built into the investigation.

Output is concise JSON: input identity, provider, procedure, pseudocode,
parameter availability, limitations and evidence paths. The complete raw response,
logs, request and snapshot remain in a private run directory. Results exceeding
the 16 MiB display limit fail visibly; the raw file is retained, not truncated.
Analysis failures return `ok: false` and a nonzero exit even when the upstream
CLI envelope says `ok: true`. Failed runs keep their evidence and are not cached.

Force a new observation when needed:

```sh
bash scripts/query.sh /absolute/path/to/program function_name --fresh
```

Fresh runs retain previous evidence. Invalid snapshots/profile changes are not
silently retried; an explicit fresh run can replace the active cache reference.
Original targets remain read-only and are never executed by this command.

## Several related questions

Use the OMP agent and keep one REA MCP session open: open one selected input,
follow bounded strings/xrefs/functions, reuse returned dossier facets, save a
snapshot, then close. Warm same-session queries avoid repeated imports. The CLI
helper is for one-off questions, not a second analysis engine.

## Check the framework

```sh
bash scripts/check-framework.sh
```

Checks include generated ELF and x86-64 PE DLL decompilation, strings/references,
assembly, ownership cleanup, cached replay without Ghidra launch, two independent
ELF programs, explicit fresh analysis, changed bytes at the same filename,
unchanged original inputs and unknown-procedure failures without cache promotion.
Temporary fixtures are removed; evidence remains under `.local/re/engine-check/`
and `.local/re/query-check/`.

Measured runs and coverage limits: `docs/reverse-engineering.md`.

## Prerequisites

- Node 22.19+, supported Node 24, or Node 26+.
- Extracted Ghidra 12.1.4, defaulting to `~/tools/ghidra_12.1.4_PUBLIC`.
- 64-bit JDK 21. Setup can install checksum-pinned Temurin 21.0.12.1+1 locally.
- `cc`, `clang`, `lld-link`, `timeout` for generated checks; `strace` for cache traces.

Select existing installations with:

```sh
bash scripts/setup-tools.sh /absolute/ghidra_12.1.4_PUBLIC /absolute/jdk-21
```

Native paths are stored in ignored `.local/` files. The current runner is
Linux-oriented; rootless `strace` bootstrap uses signed Arch packages. No Windows
or macOS host verification is claimed. No system Java setting is changed.

## OMP instructions

`.omp/AGENTS.md` routes tasks, `.omp/RULES.md` supplies short sticky invariants,
and `.omp/rules/re-workflow.md` describes question-first native investigations.
Only load instructions relevant to the question. Raw evidence, proprietary input,
databases, local paths and credentials stay outside Git. See `CREDITS.md` for
source provenance and licenses.

Static inspection is the default. Running an unknown target or capturing its
runtime requires separate authorization. The local decompiler runs with your
user permissions; it is not an OS security sandbox.
