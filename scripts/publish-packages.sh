#!/usr/bin/env bash
# Publish all @s2script/* packages under packages/ to npm (public).
# Requires: npm login with publish rights on the @s2script org, and a prior
# version bump. Dry-run with: DRY_RUN=1 scripts/publish-packages.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

( cd packages/cli && npm run build )

PACKAGES=(
  events entity math frame timers console interfaces config commands chat
  clients cookies admin bans server damage db http ws menu topmenu votes
  plugins trace usermessages globals cs2 cli
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
