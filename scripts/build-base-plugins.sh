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
# leading v — every plugins/*/package.json (and plugins/disabled/*/) is rewritten
# to that version BEFORE build so the .s2sp manifest matches the runtime zip.
# npm @s2script/* packages are independent (Changesets); plugins track the tag.
#
# THIS IS NOW A THIN SHIM over `s2s build --stamp-version` (design spec
# 2026-07-27 §9.3). The repo root carries `s2script.workspace`, so the CLI owns
# discovery, dependency ordering, the preflight gates, version stamping and the
# collect-all summary; the loop that used to live here was the workaround that
# justified building the capability into the CLI in the first place.
#
# The file is KEPT rather than deleted deliberately: .github/workflows/release.yml
# and scripts/package-release.sh both shell to it, and the auto-publishing release
# path is the part least worth churning. Its contract is unchanged — same env var,
# same exit codes, same "PASS: built N base plugin(s)" line, same .s2sp listing.
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

# Optional: stamp plugin versions to match a release tag (plugins track the zip).
# `--stamp-version` rewrites sibling ranges alongside the versions, which the old
# in-script `node -e` stamp never did — stamping every plugin to one version would
# otherwise trip the §5.2 range gate on any consumer declaring an older producer.
TAG_VERSION="${VERSION:-${1:-}}"
TAG_VERSION="${TAG_VERSION#v}"
STAMP=()
if [ -n "$TAG_VERSION" ]; then
    echo "=== stamp plugin versions → $TAG_VERSION ==="
    STAMP=(--stamp-version "$TAG_VERSION")
fi

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
