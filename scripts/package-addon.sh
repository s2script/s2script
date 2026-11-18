#!/usr/bin/env bash
# Assembles dist/addons/ from build outputs for mounting into the CS2 server.
# Run after:  make core && make shim
# Then use:   docker compose -f docker/docker-compose.yml up -d
#
# S2_PLATFORM=win64 packages the Windows build instead (scripts/build-windows.sh sets it, with
# S2_SHIM_FILE / S2_CORE_FILE / S2_SHIM_BUILD_DIR pointing at the cross-built DLLs).
set -euo pipefail
cd "$(dirname "$0")/.."

# The SDK's strict function JSONC parser imports TypeScript even when the shipped game has
# no functionsFile. Fail before replacing dist so direct packaging cannot leave stale output.
node scripts/lib/check-game-package-deps.mjs

PLATFORM="${S2_PLATFORM:-linuxsteamrt64}"
case "$PLATFORM" in
  linuxsteamrt64) SHIM_NAME=s2script.so;  CORE_NAME=libs2script_core.so ;;
  win64)          SHIM_NAME=s2script.dll; CORE_NAME=s2script_core.dll ;;
  *) echo "ERROR: unknown S2_PLATFORM '$PLATFORM' (linuxsteamrt64|win64)" >&2; exit 1 ;;
esac
SHIM_BUILD_DIR="${S2_SHIM_BUILD_DIR:-build/shim}"
SHIM_FILE="${S2_SHIM_FILE:-$SHIM_BUILD_DIR/$SHIM_NAME}"

DIST=dist/addons
rm -rf "$DIST"
mkdir -p "$DIST/s2script/bin/$PLATFORM"
mkdir -p "$DIST/s2script/gamedata"
mkdir -p "$DIST/metamod"

# --- Metamod plugin shim (required) ---
if [ ! -f "$SHIM_FILE" ]; then
    echo "ERROR: $SHIM_FILE not found — run: make shim (or scripts/build-windows.sh)" >&2
    exit 1
fi
cp "$SHIM_BUILD_DIR/VERSION" "$DIST/s2script/VERSION"
cp "$SHIM_FILE" "$DIST/s2script/bin/$PLATFORM/$SHIM_NAME"

# --- V8 core cdylib (required; checks release first, falls back to debug) ---
CORE_SO="${S2_CORE_FILE:-}"
if [ "$PLATFORM" != linuxsteamrt64 ] && [ -z "$CORE_SO" ]; then
    echo "ERROR: S2_CORE_FILE must name the $PLATFORM $CORE_NAME (scripts/build-windows.sh sets it)" >&2
    exit 1
fi
[ -z "$CORE_SO" ] && [ -f target/release/libs2script_core.so ] && CORE_SO="target/release/libs2script_core.so"
[ -z "$CORE_SO" ] && [ -f target/debug/libs2script_core.so ] && CORE_SO="target/debug/libs2script_core.so"
if [ -z "$CORE_SO" ]; then
    echo "ERROR: $CORE_NAME not found — run: make core  (cargo build --release)" >&2
    exit 1
fi
cp "$CORE_SO" "$DIST/s2script/bin/$PLATFORM/$CORE_NAME"
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

# --- Runtime dirs (plugins and native-extension drop zones + writable configs/data) ---
mkdir -p "$DIST/s2script/plugins" "$DIST/s2script/extensions" "$DIST/s2script/configs" "$DIST/s2script/data"
# translations/: the operator-editable phrase files. Without this the directory never exists in an
# install, translations_read always returns null, and every phrase silently falls back to its
# in-code seed — the whole file mechanism dead while appearing to work.
mkdir -p "$DIST/s2script/translations"
if [ -d translations ]; then
    cp -r translations/. "$DIST/s2script/translations/"
fi

# --- Metamod plugin registration VDF ---
if [ "$PLATFORM" = linuxsteamrt64 ]; then
    cp docker/s2script.vdf "$DIST/metamod/s2script.vdf"

    # --- Copy VDF into docker/metamod/ so compose only needs a single dir mount ---
    mkdir -p docker/metamod
    cp docker/s2script.vdf docker/metamod/s2script.vdf
else
    # Metamod appends the platform's library suffix to "file".
    sed "s#bin/linuxsteamrt64/s2script#bin/$PLATFORM/s2script#" docker/s2script.vdf > "$DIST/metamod/s2script.vdf"
    grep -q "bin/$PLATFORM/s2script\"" "$DIST/metamod/s2script.vdf" || {
        echo "ERROR: could not derive the $PLATFORM Metamod VDF from docker/s2script.vdf" >&2; exit 1; }
fi

echo ""
echo "packaged: $DIST"
find "$DIST" -type f | sort
