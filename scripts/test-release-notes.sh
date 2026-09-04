#!/usr/bin/env bash
# GitHub Release notes are GitHub's own generate_release_notes (PRs since the
# previous tag). CI only attaches the zip. A custom body is how v0.5.3–v0.5.10
# each shipped the entire package changelog history.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
ok()   { echo "  ok   $1"; }
bad()  { echo "  FAIL $1" >&2; fail=1; }

wf=.github/workflows/release.yml

grep -q "generate_release_notes: true" "$wf" \
  && ok "release.yml uses GitHub generated notes" \
  || bad "release.yml dropped generate_release_notes"

if grep -qE '^[[:space:]]+body_path:' "$wf"; then
  bad "release.yml still feeds a custom body (the kitchen-sink dump)"
else
  ok "release.yml does not set a custom release body"
fi

if grep -qE '^[[:space:]]+append_body:' "$wf"; then
  bad "release.yml still appends onto an existing body (duplicates on re-run)"
else
  ok "release.yml does not append_body"
fi

if grep -qE '^[[:space:]]+bash scripts/release-notes\.sh' "$wf"; then
  bad "release.yml still runs a custom notes script"
else
  ok "release.yml does not run a custom notes script"
fi

if [ -e scripts/release-notes.sh ]; then
  bad "scripts/release-notes.sh still exists"
else
  ok "scripts/release-notes.sh is gone"
fi

grep -q 'files: \${{ steps.package.outputs.zip }}' "$wf" \
  && ok "release.yml still uploads the zip" \
  || bad "release.yml no longer uploads the zip"

[ "$fail" -eq 0 ] || { echo "test-release-notes: FAILED" >&2; exit 1; }
echo "test-release-notes: all passed"
