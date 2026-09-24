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
cp build/shim/VERSION "$DIST/s2script/VERSION"
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

# --- Shared engine and extension gamedata; CS2's relocated source is packaged below. ---
# <owner>/custom/ is EXCLUDED. It is the operator override channel: a maintainer's local hot-fix is
# untracked (see .gitignore) but would otherwise be copied here, then into the release zip by
# package-release.sh, and land on every operator's server indistinguishable from shipped data.
# .gitignore stops it being committed; this stops it being shipped.
if [ -d gamedata ]; then
    tar -C gamedata --exclude=custom -cf - . | tar -C "$DIST/s2script/gamedata" -xf -
else
    echo "ERROR: gamedata/ tree not found" >&2
    exit 1
fi

# --- Verified game packages; shipped game JS/data have no legacy deployed copies. ---
node --experimental-strip-types --no-warnings scripts/build-game-packages.mjs --out "$DIST/s2script"

# --- Runtime dirs (plugins drop zone + writable configs/data) ---
mkdir -p "$DIST/s2script/plugins" "$DIST/s2script/configs" "$DIST/s2script/data"
# translations/: the operator-editable phrase files. Without this the directory never exists in an
# install, translations_read always returns null, and every phrase silently falls back to its
# in-code seed — the whole file mechanism dead while appearing to work.
mkdir -p "$DIST/s2script/translations"
if [ -d translations ]; then
    cp -r translations/. "$DIST/s2script/translations/"
fi

# --- Metamod plugin registration VDF ---
cp docker/s2script.vdf "$DIST/metamod/s2script.vdf"

# --- Copy VDF into docker/metamod/ so compose only needs a single dir mount ---
mkdir -p docker/metamod
cp docker/s2script.vdf docker/metamod/s2script.vdf

echo ""
echo "packaged: $DIST"
find "$DIST" -type f | sort
