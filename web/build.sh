#!/usr/bin/env bash
# Builds the WebGPU client into web/dist and the game data packs into web/dist/data.
#   web/build.sh [GAME_DIR] [MAP..]     (no MAP: every map)
# Needs: the wasm32-unknown-unknown Rust target, wasm-bindgen-cli matching Cargo.lock's
# wasm-bindgen (installed into .local/tools if missing), and optionally wasm-opt (binaryen).
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD
dist=web/dist
tools=$root/.local/tools
export PATH=$tools/bin:$tools/node_modules/.bin:$PATH

want=$(awk '/^name = "wasm-bindgen"$/{getline; gsub(/[^0-9.]/,""); print; exit}' Cargo.lock)
if [[ "$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')" != "$want" ]]; then
  cargo install wasm-bindgen-cli --version "$want" --locked --root "$tools"
fi

# simd128: glam's vector/matrix math (transforms, skinning, collision) uses wasm SIMD; every
# browser with WebGPU supports it.
RUSTFLAGS='--cfg getrandom_backend="wasm_js" -C target-feature=+simd128' \
  cargo build --profile web --target wasm32-unknown-unknown --bin gunz-play --locked
rm -rf "$dist/pkg" && mkdir -p "$dist/pkg"
wasm-bindgen --target web --no-typescript --out-dir "$dist/pkg" \
  target/wasm32-unknown-unknown/web/gunz-play.wasm
wasm=$dist/pkg/gunz-play_bg.wasm
if command -v wasm-opt >/dev/null; then
  # -O3 for speed; size is handled by brotli below
  wasm-opt -O3 --enable-simd --enable-bulk-memory --enable-nontrapping-float-to-int \
    --enable-sign-ext --enable-mutable-globals "$wasm" -o "$wasm.opt" && mv "$wasm.opt" "$wasm"
fi
# The page asks for the engine as `?v=HASH` of its files, so browsers cache it for good
# (`serve.py`) and a new build is a new URL.
build=$(cat "$wasm" "$dist/pkg/gunz-play.js" | sha256sum | cut -c1-16)
html=$(<web/index.html)
printf '%s\n' "${html//__BUILD__/$build}" > "$dist/index.html"
cp web/serve.py "$dist/"
# Precompressed copies; serve.py (or any server with precompressed-file support) sends them.
for f in "$dist/pkg/"*.wasm "$dist/pkg/"*.js "$dist/index.html"; do
  gzip -9 -k -f "$f"
  command -v brotli >/dev/null && brotli -q 11 -k -f "$f"
done

# Game data: only if missing or asked for (it reads your install; keep it on your machine).
if [[ ! -f "$dist/data/index.json" || $# -gt 0 ]]; then
  game=()
  if [[ -d "${1:-}" ]]; then game=("$1"); shift; fi
  cargo build --release --locked --bin gunz-play --bin gunz-pack
  # files/ (each clothing file on its own) is kept: the install's files do not change
  rm -rf "$dist/data/maps" "$dist/data/core.pack.gz" "$dist/data/index.json"
  target/release/gunz-pack "${game[@]}" "$dist/data" "$@"
fi
ls -la "$dist/pkg"
echo "serve: python3 $dist/serve.py  (http://localhost:8080)"
