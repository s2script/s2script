#!/usr/bin/env bash
# Fails when `MODALS` in games/cs2/js/components.js disagrees with the number of `s2_m*` panel
# trees the checked-in workshop layout actually defines.
#
# WHY THIS GATE EXISTS. A server can only address panels the CLIENT's layout already contains,
# and Panorama ignores unknown ids silently — so raising MODALS without adding the markup hands
# out pooled sheets that paint nothing, and the failure surfaces as a player staring at an empty
# screen, not as an error anywhere. The two numbers live in different files with different reasons
# to change (a JS constant vs. a workshop addon layout), which is exactly the shape that drifts.
# components.test.js pins the markup side, but a components.js edit that never re-runs that suite
# locally would ride to CI with nothing comparing the two SOURCES to each other — this script is
# that comparison, wired into ci-js.sh so local green means CI green.
set -euo pipefail
cd "$(dirname "$0")/.."

COMPONENTS=games/cs2/js/components.js
LAYOUT=examples/hud-lab/workshop/panorama/layout/custom_game/s2script_lib.xml

# The constant, exactly as declared. Anchored so a comment mentioning MODALS can't satisfy it.
declared=$(sed -n 's/^  var MODALS = \([0-9][0-9]*\);.*/\1/p' "$COMPONENTS")
if [ -z "$declared" ]; then
  echo "check-modals-markup-sync: FAIL — could not find 'var MODALS = <n>;' in $COMPONENTS" >&2
  echo "  (if the declaration moved or was renamed, update this script with it)" >&2
  exit 1
fi

# Root ids only (id="s2_m<digits>"), not the _r*/_f*/_d* children — each root is one sheet tree.
in_markup=$(grep -o 'id="s2_m[0-9][0-9]*"' "$LAYOUT" | sort -u | wc -l | tr -d ' ')

if [ "$declared" -ne "$in_markup" ]; then
  echo "check-modals-markup-sync: FAIL — components.js declares MODALS = $declared but" >&2
  echo "  $LAYOUT defines $in_markup s2_m* sheet trees." >&2
  echo "  A server can only address panels the client's layout contains, so these must move" >&2
  echo "  together: add/remove whole s2_m<n> trees in the layout (and republish the workshop" >&2
  echo "  addon) in the same change that edits MODALS." >&2
  exit 1
fi

# Claims are handed out lowest-first and the ids are minted as s2_m0..s2_m(MODALS-1), so the
# markup must cover exactly that contiguous range — six trees numbered 1..6 would still count 6.
for ((i = 0; i < declared; i++)); do
  if ! grep -q "id=\"s2_m$i\"" "$LAYOUT"; then
    echo "check-modals-markup-sync: FAIL — MODALS = $declared but the layout has no s2_m$i root" >&2
    echo "  (the pool mints ids s2_m0..s2_m$((declared - 1)); the markup's trees must be that exact range)" >&2
    exit 1
  fi
done

echo "check-modals-markup-sync: ok (MODALS = $declared, layout defines s2_m0..s2_m$((declared - 1)))"
