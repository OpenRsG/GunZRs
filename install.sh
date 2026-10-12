#!/bin/sh
# Installs GunZRs for the current user on Linux x86_64: the release in ~/.local/share/gunzrs-app,
# the `gunzrs` command in ~/.local/bin and an application menu entry. Run it from an unpacked
# release, or straight from GitHub (downloads the latest release):
#   curl -fsSL https://raw.githubusercontent.com/OpenRsG/GunZRs/main/install.sh | sh
set -eu
data="${XDG_DATA_HOME:-$HOME/.local/share}"
app="$data/gunzrs-app"
src="$(dirname "$(readlink -f "$0")")"
tmp=""
if [ ! -x "$src/gunz-play" ]; then
    url=$(curl -fsSL https://api.github.com/repos/OpenRsG/GunZRs/releases/latest |
        grep -o 'https://[^"]*linux-x86_64\.tar\.gz' | head -n 1)
    tmp=$(mktemp -d)
    curl -fsSL "$url" | tar xz -C "$tmp" --strip-components=1
    src="$tmp"
fi
rm -rf "$app"
mkdir -p "$app" "$HOME/.local/bin" "$data/applications"
cp -r "$src"/. "$app"/
ln -sf "$app/start.sh" "$HOME/.local/bin/gunzrs"
sed "s|^Exec=.*|Exec=$app/start.sh|" "$app/gunzrs.desktop" > "$data/applications/gunzrs.desktop"
if [ -n "$tmp" ]; then rm -rf "$tmp"; fi
echo "GunZRs installed: run gunzrs, or GunZRs from the application menu."
