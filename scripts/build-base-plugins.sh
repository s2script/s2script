#!/usr/bin/env bash
# Build every first-party base plugin under plugins/ into .s2sp archives.
# Demos live in examples/ and are NOT built here.
#
# Usage (from repo root):
#   scripts/build-base-plugins.sh
#   VERSION=0.1.2 scripts/build-base-plugins.sh   # stamp plugin package.json
#                                                 # versions to match a release tag
#
# Every bundled plugin tracks scripts/framework-version.sh, including dev builds.
# Explicit VERSION (or $1) overrides Git-derived release/development versions.
# The CLI stamps package versions and sibling dependency ranges before building.
# npm SDK packages and third-party plugins retain independent versions.
#
# Requires Node. Builds the local CLI first, then typechecks+bundles each plugin.
# Emits: plugins/<name>/dist/*.s2sp
set -euo pipefail
cd "$(dirname "$0")/.."

if [ ! -d plugins ]; then
    echo "ERROR: plugins/ directory missing" >&2
    exit 1
fi

echo "=== build @s2script/sdk ==="
# node_modules is load-bearing now, not just a build convenience: npm's workspace
# symlinks are how a plugin resolves a SIBLING's published contract in place
# (spec §3.1–3.2), so a missing node_modules is a broken build, not a slow one.
if [ ! -d node_modules ]; then
    npm install --no-fund --no-audit
fi
( cd packages/sdk && npm run build )

CLI="node packages/sdk/dist/cli.js"

TAG_VERSION="$(bash scripts/framework-version.sh "${1:-${VERSION:-}}")"
echo "=== stamp plugin versions → $TAG_VERSION ==="
STAMP=(--stamp-version "$TAG_VERSION")

# Workspace mode prints one artifact path per built plugin on stdout (progress,
# the stamp report and every failure go to stderr), so counting stdout lines
# reproduces the old per-plugin `built=$((built + 1))` exactly.
#
# There is deliberately no *-demo skip any more: the workspace model has no
# exclusion glob, demos belong in examples/ (this script has said so since it was
# written), and package-release.sh still refuses to COPY a */-demo/* artifact
# into the zip — the guard that actually protects a release is still in place.
BUILT_LIST=$(mktemp)
trap 'rm -f "$BUILT_LIST"' EXIT

if ! $CLI build "${STAMP[@]+"${STAMP[@]}"}" >"$BUILT_LIST"; then
    echo "FAIL: one or more base plugins failed to build" >&2
    exit 1
fi

built=$(grep -c . "$BUILT_LIST" || true)
if [ "$built" -eq 0 ]; then
    echo "FAIL: no base plugins found under plugins/" >&2
    exit 1
fi

echo "PASS: built $built base plugin(s)"
# maxdepth 4 so opt-in plugins under plugins/disabled/*/dist/ are listed too.
find plugins -maxdepth 4 -type f -name '*.s2sp' | sort
