#!/usr/bin/env bash
# Assembles dist/addons/ from build outputs for mounting into the CS2 server.
# Run after:  make core && make shim
# Then use:   docker compose -f docker/docker-compose.yml up -d
set -euo pipefail
cd "$(dirname "$0")/.."

DIST=dist/addons
rm -rf "$DIST"
mkdir -p "$DIST/s2script/bin/linuxsteamrt64"
mkdir -p "$DIST/s2script/gamedata"
mkdir -p "$DIST/metamod"

# --- Metamod plugin shim (required) ---
if [ ! -f build/shim/s2script.so ]; then
    echo "ERROR: build/shim/s2script.so not found — run: make shim" >&2
    exit 1
fi
cp build/shim/s2script.so "$DIST/s2script/bin/linuxsteamrt64/s2script.so"

# --- V8 core cdylib (required; checks release first, falls back to debug) ---
CORE_SO=""
[ -f target/release/libs2script_core.so ] && CORE_SO="target/release/libs2script_core.so"
[ -z "$CORE_SO" ] && [ -f target/debug/libs2script_core.so ] && CORE_SO="target/debug/libs2script_core.so"
if [ -z "$CORE_SO" ]; then
    echo "ERROR: libs2script_core.so not found — run: make core  (cargo build --release)" >&2
    exit 1
fi
cp "$CORE_SO" "$DIST/s2script/bin/linuxsteamrt64/libs2script_core.so"
echo "core: $CORE_SO"

# --- Gamedata (optional; created in a later task) ---
if [ -f gamedata/core.gamedata.jsonc ]; then
    cp gamedata/core.gamedata.jsonc "$DIST/s2script/gamedata/"
fi

# --- Metamod plugin registration VDF ---
cp docker/s2script.vdf "$DIST/metamod/s2script.vdf"

echo ""
echo "packaged: $DIST"
find "$DIST" -type f | sort
