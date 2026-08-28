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

# --- Gamedata (owner tree: core/ + cs2/, each with its master) ---
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

# --- CS2 JS package (schema.generated.js + nav.generated.js + pawn.js — CS2 names live here, never in core) ---
mkdir -p "$DIST/s2script/js"
if [ -f games/cs2/js/pawn.js ]; then
    # schema.generated.js MUST precede nav.generated.js (sets __s2pkg_cs2_schema).
    # nav.generated.js MUST precede activity.js (which precedes pawn.js, the final IIFE).
    # activity.js sets globalThis.__s2_activity before pawn.js reads it.
    # csitem.generated.js sets globalThis.__s2pkg_cs2.CsItem; pawn.js's IIFE MERGES into
    # (not overwrites) globalThis.__s2pkg_cs2, so CsItem survives regardless of order.
    # weapon.js MUST run after schema.generated.js (needs __s2pkg_cs2_schema) and before pawn.js
    # (whose acquisition getters reference globalThis.__s2pkg_cs2.Weapon); it MERGES into
    # globalThis.__s2pkg_cs2 like csitem.generated.js, so exact position among the others doesn't matter.
    cat games/cs2/js/schema.generated.js games/cs2/js/nav.generated.js games/cs2/js/activity.js games/cs2/js/csitem.generated.js games/cs2/js/weapon.js games/cs2/js/pawn.js games/cs2/js/ui.js games/cs2/js/components.js > "$DIST/s2script/js/pawn.js"
fi

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
