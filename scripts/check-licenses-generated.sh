#!/usr/bin/env bash
# Fail if the committed third-party notice file is out of date vs a fresh generation.
# A stale licenses.txt is a compliance bug that otherwise looks like a green build.
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"
./scripts/gen-licenses.sh >/dev/null

# `git status --porcelain`, not `git diff --exit-code`: diff ignores UNTRACKED files, so a
# never-committed licenses.txt would pass the gate vacuously. Status catches modified AND
# untracked, which is what "the committed notices match a fresh generation" actually means.
PATHS=(licenses/ packages/*/LICENSE-MIT packages/*/LICENSE-APACHE)
DIRTY="$(git status --porcelain -- "${PATHS[@]}")"
if [ -n "$DIRTY" ]; then
    echo "FAIL: license artifacts are stale or uncommitted:" >&2
    echo "$DIRTY" >&2
    git --no-pager diff --stat -- "${PATHS[@]}" >&2 || true
    echo "  run ./scripts/gen-licenses.sh and commit the result" >&2
    exit 1
fi
echo "PASS: third-party notices and per-package license texts are up to date"
