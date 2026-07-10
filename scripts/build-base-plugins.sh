#!/usr/bin/env bash
# Build every first-party base plugin under plugins/ into .s2sp archives.
# Demos live in examples/ and are NOT built here.
#
# Usage (from repo root):
#   scripts/build-base-plugins.sh
#
# Requires Node. Builds the local CLI first, then typechecks+bundles each plugin.
# Emits: plugins/<name>/dist/*.s2sp
set -euo pipefail
cd "$(dirname "$0")/.."

if [ ! -d plugins ]; then
    echo "ERROR: plugins/ directory missing" >&2
    exit 1
fi

echo "=== build @s2script/cli ==="
(
    cd packages/cli
    if [ ! -d node_modules ]; then
        npm install --no-fund --no-audit
    fi
    npm run build
)

CLI="node packages/cli/dist/cli.js"
fail=0
built=0

for d in plugins/*/; do
    [ -f "$d/package.json" ] || continue
    name=$(basename "$d")
    # Safety: never ship *-demo from plugins/ (demos belong in examples/)
    case "$name" in
        *-demo)
            echo "SKIP demo left in plugins/: $name (move to examples/)" >&2
            continue
            ;;
    esac
    echo "=== build $d ==="
    if $CLI build "$d"; then
        built=$((built + 1))
    else
        fail=1
    fi
done

if [ "$fail" != 0 ]; then
    echo "FAIL: one or more base plugins failed to build" >&2
    exit 1
fi
if [ "$built" -eq 0 ]; then
    echo "FAIL: no base plugins found under plugins/" >&2
    exit 1
fi

echo "PASS: built $built base plugin(s)"
find plugins -maxdepth 3 -type f -name '*.s2sp' | sort
