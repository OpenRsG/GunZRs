#!/bin/sh
# Builds (if needed) and starts the game on its main menu. Extra arguments go to gunz-play,
# e.g. `./start.sh Mansion --bots 3`. GUNZ_GAME=DIR overrides the install folder; without it the
# Flatpak Steam install is used if present, else gunz-play finds the Steam install itself.
set -e
cd "$(dirname "$0")"
cargo build --release --bin gunz-play
flatpak="$HOME/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common/GUNZ THE DUEL"
game="${GUNZ_GAME:-}"
[ -z "$game" ] && [ -d "$flatpak" ] && game="$flatpak"
if [ -n "$game" ]; then
    exec target/release/gunz-play "$game" "$@"
fi
exec target/release/gunz-play "$@"
