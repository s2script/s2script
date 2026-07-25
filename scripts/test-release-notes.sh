#!/usr/bin/env bash
# Guards scripts/release-notes.sh — the thing that keeps changeset prose in the GitHub Release.
# Its failure mode is silent (a release still publishes, just with empty notes), so it needs a test.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
ok()   { echo "  ok   $1"; }
bad()  { echo "  FAIL $1" >&2; fail=1; }

git tag --list 'v*' | grep -q . || { echo "test-release-notes: SKIP — no v* tags in this checkout."; exit 0; }
NEWEST=$(git tag --list 'v*' --sort=-v:refname | head -1)
V="${NEWEST#v}"

OUT=$(bash scripts/release-notes.sh "$V" deadbeefsha 2>&1)

grep -q 'deadbeefsha'                        <<<"$OUT" && ok "sha256 is included"        || bad "sha256 missing"
grep -q "s2script-cs2-linux-${V}.zip"        <<<"$OUT" && ok "install line names the zip" || bad "install line wrong"
# Decide INDEPENDENTLY whether this tag actually had package-changelog churn, and hold the
# generator to that. Accepting "no changes since X" unconditionally is how a boilerplate-only
# regression slips through — it did, on the first version of this test.
PREV=$(git tag --list 'v*' --sort=v:refname \
       | awk -v cur="$NEWEST" '$0 == cur { exit } { last = $0 } END { if (last) print last }')
if [ -n "$PREV" ] && ! git diff --quiet "$PREV".."$NEWEST" -- 'packages/*/CHANGELOG.md'; then
  grep -q '## Package changelogs' <<<"$OUT" \
    && ok "changelogs DID change since $PREV and the sections are in the body" \
    || bad "changelogs changed since $PREV but the body has no Package changelogs section \
(the boilerplate-only regression)"
else
  grep -qE '## Package changelogs|runtime/plugin-only release' <<<"$OUT" \
    && ok "no changelog churn, and the body says so" \
    || bad "no changelog section AND no explanation"

fi

# The whole point: prose from packages/*/CHANGELOG.md must survive into the body.
if grep -qE '^#### [0-9]+\.[0-9]+\.[0-9]+' <<<"$OUT"; then
  ok "package changelog version sections carried through"
  grep -qE '^##### (Minor|Major|Patch) Changes' <<<"$OUT" \
    && ok "inner changeset headings demoted (no heading-level clash)" \
    || bad "inner headings not demoted"
fi

# Dependency-only churn must not be the only content.
if grep -qE '^#### ' <<<"$OUT"; then
  body_lines=$(grep -cE '^- ' <<<"$OUT" || true)
  dep_lines=$(grep -cE '^- (Updated dependencies|@s2script/)' <<<"$OUT" || true)
  [ "$body_lines" -gt "$dep_lines" ] \
    && ok "real prose outnumbers dependency churn" \
    || bad "output is only dependency churn"
fi

# A missing version argument must fail loudly, not emit half a body.
if bash scripts/release-notes.sh >/dev/null 2>&1; then bad "no-args should exit non-zero"; else ok "no-args exits non-zero"; fi

[ "$fail" -eq 0 ] || { echo "test-release-notes: FAILED" >&2; exit 1; }
echo "test-release-notes: all passed"
