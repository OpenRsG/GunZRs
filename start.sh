#!/bin/sh
# Starts GunZRs on its main menu; extra arguments go to gunz-play (`./start.sh Mansion --bots 3`).
# Works in a source checkout (builds first) and in an installed or unpacked release (runs the
# binaries beside it). The game is found through Steam; GUNZ_GAME=DIR overrides that.
set -e
dir="$(dirname "$(readlink -f "$0")")"
if [ -f "$dir/Cargo.toml" ]; then
    cargo build --release --manifest-path "$dir/Cargo.toml" --bin gunz-play --bin gunz
    dir="$dir/target/release"
fi
# renders the maps' pictures for the menu and the loading screen; does nothing once they exist
"$dir/gunz" --previews || true
exec "$dir/gunz-play" "$@"
