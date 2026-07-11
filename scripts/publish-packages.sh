#!/usr/bin/env bash
# DEPRECATED as the primary publish path — prefer Changesets + OIDC
# (.github/workflows/changesets.yml). This script is a local dry-run /
# emergency classic-token fallback only.
#
# Maintainer flow (normal):
#   1. On a PR that changes packages/:  npm run changeset
#   2. Merge → CI opens a Version Packages PR
#   3. Merge the version PR → CI runs `changeset publish` via OIDC
#
# Bootstrap (one-time, before OIDC works): packages must exist on npm and
# each needs a Trusted Publisher → GabeHirakawa/s2script/changesets.yml.
# See scripts/bootstrap-npm-trusted-publishing.sh and docs/INSTALL.md.
#
# Prefer: DRY_RUN=1 scripts/publish-packages.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "note: prefer Changesets + OIDC trusted publishing on main. Continuing as fallback…" >&2

if [ ! -d node_modules ]; then
  npm install --no-fund --no-audit
fi
( cd packages/cli && npm run build )

PACKAGES=(
  events entity math frame timers console interfaces config commands chat
  clients cookies admin bans server damage db http ws menu topmenu votes
  plugins trace usermessages globals cs2 zones cli
)

for name in "${PACKAGES[@]}"; do
  dir="packages/$name"
  if [[ ! -f "$dir/package.json" ]]; then
    echo "skip missing $dir"
    continue
  fi
  echo "=== publishing @s2script/$name ==="
  if [[ "${DRY_RUN:-}" == "1" ]]; then
    ( cd "$dir" && npm publish --access public --dry-run )
  else
    ( cd "$dir" && npm publish --access public )
  fi
done

echo "done."
