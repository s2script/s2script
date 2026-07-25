#!/usr/bin/env bash
# Fails if a branch changes a PUBLISHED package but adds no changeset.
#
# WHY THIS GATE EXISTS. `packages/*` is published to npm by changesets on merge to main. A PR that
# changes a package without a changeset still merges and still ships — it just does so with no
# version bump and no changelog entry, so the release history silently omits it.
#
# That is not hypothetical: five consecutive merged PRs (#17, #18, #19, #20, #21) each changed
# packages/ and none carried a changeset, leaving four capabilities and two new subpaths absent from
# the release notes entirely. Nothing failed — which is precisely the problem.
#
# Only PUBLISHED packages count: a private package is not released, so it needs no changeset.
# Skips cleanly when there is no upstream to diff against (a detached CI checkout of main itself).
set -euo pipefail
cd "$(dirname "$0")/.."

BASE="${CHANGESET_BASE:-origin/main}"
if ! git rev-parse --verify --quiet "$BASE" >/dev/null; then
  echo "check-changeset: SKIP — no $BASE to diff against."
  exit 0
fi

MERGE_BASE=$(git merge-base HEAD "$BASE" 2>/dev/null || true)
if [ -z "$MERGE_BASE" ] || [ "$(git rev-parse HEAD)" = "$(git rev-parse "$BASE")" ]; then
  echo "check-changeset: SKIP — HEAD is $BASE (nothing to compare)."
  exit 0
fi

CHANGED=$(git diff --name-only "$MERGE_BASE"..HEAD -- 'packages/*' || true)
[ -z "$CHANGED" ] && { echo "check-changeset: ok (no packages/ changes)"; exit 0; }

# Which of those packages are actually published?
PUBLISHED=""
while IFS= read -r f; do
  pkgdir=$(echo "$f" | cut -d/ -f1-2)
  [ -f "$pkgdir/package.json" ] || continue
  if ! python3 -c "
import json,sys
d=json.load(open('$pkgdir/package.json'))
sys.exit(0 if not d.get('private') else 1)" 2>/dev/null; then continue; fi
  PUBLISHED="$PUBLISHED$pkgdir\n"
done <<< "$CHANGED"
PUBLISHED=$(printf "%b" "$PUBLISHED" | sort -u | sed '/^$/d')
[ -z "$PUBLISHED" ] && { echo "check-changeset: ok (only private packages changed)"; exit 0; }

ADDED=$(git diff --name-only --diff-filter=A "$MERGE_BASE"..HEAD -- '.changeset/*.md' \
        | grep -v 'README' || true)
if [ -z "$ADDED" ]; then
  echo "check-changeset: FAIL — published package(s) changed with no changeset." >&2
  echo "$PUBLISHED" | sed 's/^/    /' >&2
  echo >&2
  echo "  These publish to npm on merge. Without a changeset they ship with no version bump and" >&2
  echo "  no changelog entry, so the release history silently omits the change." >&2
  echo "  Run:  npm run changeset" >&2
  exit 1
fi

echo "check-changeset: ok ($(echo "$ADDED" | wc -l | tr -d ' ') changeset(s) for $(echo "$PUBLISHED" | wc -l | tr -d ' ') published package(s))"
