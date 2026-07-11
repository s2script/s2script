#!/usr/bin/env bash
# Stage a SourceMod-style release zip from an already-packaged dist/addons/.
# Caller must run scripts/build-sniper.sh (or make package) first.
# Base plugins: run scripts/build-base-plugins.sh first (or this script builds them).
#
# Usage:
#   scripts/package-release.sh [VERSION]
#
# VERSION defaults to: strip leading 'v' from GITHUB_REF_NAME, else
# `git describe --tags --exact-match`, else `git describe --tags --always`.
#
# Emits: dist/s2script-cs2-linux-<VERSION>.zip  (root = addons/…)
set -euo pipefail
cd "$(dirname "$0")/.."

DIST_ADDONS=dist/addons
STAGE=dist/release
OUT_DIR=dist

if [ ! -f "$DIST_ADDONS/s2script/bin/linuxsteamrt64/s2script.so" ]; then
    echo "ERROR: $DIST_ADDONS/s2script/bin/linuxsteamrt64/s2script.so missing — run scripts/build-sniper.sh first" >&2
    exit 1
fi
if [ ! -f "$DIST_ADDONS/s2script/bin/linuxsteamrt64/libs2script_core.so" ]; then
    echo "ERROR: libs2script_core.so missing — run scripts/build-sniper.sh first" >&2
    exit 1
fi
if [ ! -f "$DIST_ADDONS/metamod/s2script.vdf" ]; then
    echo "ERROR: metamod/s2script.vdf missing — run scripts/package-addon.sh first" >&2
    exit 1
fi

resolve_version() {
    if [ -n "${1:-}" ]; then
        echo "${1#v}"
        return
    fi
    if [ -n "${GITHUB_REF_NAME:-}" ]; then
        echo "${GITHUB_REF_NAME#v}"
        return
    fi
    if ver=$(git describe --tags --exact-match 2>/dev/null); then
        echo "${ver#v}"
        return
    fi
    if ver=$(git describe --tags --always 2>/dev/null); then
        echo "${ver#v}"
        return
    fi
    echo "0.0.0-dev"
}

VERSION="$(resolve_version "${1:-}")"
if [ -z "$VERSION" ]; then
    echo "ERROR: empty VERSION" >&2
    exit 1
fi

ZIP_NAME="s2script-cs2-linux-${VERSION}.zip"
ZIP_PATH="$OUT_DIR/$ZIP_NAME"

# Build base plugins if none are present yet (release CI builds them explicitly first).
shopt -s nullglob
existing_s2sp=(plugins/*/dist/*.s2sp)
shopt -u nullglob
if [ "${#existing_s2sp[@]}" -eq 0 ]; then
    echo "=== no built .s2sp found — running build-base-plugins.sh ==="
    VERSION="$VERSION" bash scripts/build-base-plugins.sh
fi

rm -rf "$STAGE"
mkdir -p "$STAGE/addons"

# Copy the packaged addon tree (binaries, gamedata, js, vdf).
cp -a "$DIST_ADDONS/metamod" "$STAGE/addons/metamod"
cp -a "$DIST_ADDONS/s2script" "$STAGE/addons/s2script"

# Ensure operator dirs exist even if package-addon was an older run.
mkdir -p \
    "$STAGE/addons/s2script/plugins" \
    "$STAGE/addons/s2script/configs" \
    "$STAGE/addons/s2script/data"

# Clear any leftover .s2sp from a local Docker deploy, then install base plugins.
find "$STAGE/addons/s2script/plugins" -maxdepth 1 -type f -name '*.s2sp' -delete

plugin_count=0
shopt -s nullglob
for s2sp in plugins/*/dist/*.s2sp; do
    # Skip any *-demo that somehow remained under plugins/
    case "$s2sp" in
        */*-demo/*) continue ;;
    esac
    cp "$s2sp" "$STAGE/addons/s2script/plugins/"
    plugin_count=$((plugin_count + 1))
done
shopt -u nullglob

if [ "$plugin_count" -eq 0 ]; then
    echo "ERROR: no base plugin .s2sp files to include — run scripts/build-base-plugins.sh" >&2
    exit 1
fi

cat > "$STAGE/addons/s2script/plugins/README.txt" <<EOF
First-party base plugins ($plugin_count) ship in this release.
Drop additional .s2sp archives here to load them.
The runtime watches this directory (top-level only) and hot-loads / reloads / unloads on change.
EOF

printf '%s\n' "$VERSION" > "$STAGE/addons/s2script/VERSION"

# Zip with addons/ at the archive root (unzip into game/csgo/).
rm -f "$ZIP_PATH"
(
    cd "$STAGE"
    zip -r -q "../$ZIP_NAME" addons
)

echo ""
echo "release: $ZIP_PATH"
echo "base plugins included: $plugin_count"
echo -n "sha256: "
sha256sum "$ZIP_PATH" | awk '{print $1}'
echo "layout:"
unzip -l "$ZIP_PATH" | sed -n '1,60p'
