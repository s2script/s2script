#!/usr/bin/env bash
# Build every first-party base plugin under plugins/ into .s2sp archives.
# Demos live in examples/ and are NOT built here.
#
# Usage (from repo root):
#   scripts/build-base-plugins.sh
#   VERSION=0.1.2 scripts/build-base-plugins.sh   # stamp plugin package.json
#                                                 # versions to match a release tag
#
# When VERSION (or $1) is set — typically the GitHub Release tag without a
# leading v — every plugins/*/package.json (and disabled/*/) is rewritten to
# that version BEFORE build so the .s2sp manifest matches the runtime zip.
# npm @s2script/* packages are independent (Changesets); plugins track the tag.
#
# Requires Node. Builds the local CLI first, then typechecks+bundles each plugin.
# Emits: plugins/<name>/dist/*.s2sp
set -euo pipefail
cd "$(dirname "$0")/.."

if [ ! -d plugins ]; then
    echo "ERROR: plugins/ directory missing" >&2
    exit 1
fi

# Optional: stamp plugin versions to match a release tag (plugins track the zip).
TAG_VERSION="${VERSION:-${1:-}}"
TAG_VERSION="${TAG_VERSION#v}"
if [ -n "$TAG_VERSION" ]; then
    echo "=== stamp plugin versions → $TAG_VERSION ==="
    for d in plugins/*/ disabled/*/; do
        [ -f "$d/package.json" ] || continue
        node -e '
          const fs = require("fs");
          const p = process.argv[1];
          const ver = process.argv[2];
          const j = JSON.parse(fs.readFileSync(p, "utf8"));
          if (j.version === ver) process.exit(0);
          j.version = ver;
          fs.writeFileSync(p, JSON.stringify(j, null, 2) + "\n");
          console.log("  " + j.name + " → " + ver);
        ' "$d/package.json" "$TAG_VERSION"
    done
fi

echo "=== build @s2script/cli ==="
# Workspaces hoist deps to the repo root (package.json workspaces: packages/*).
if [ ! -d node_modules ]; then
    npm install --no-fund --no-audit
fi
( cd packages/cli && npm run build )

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
