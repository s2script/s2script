#!/usr/bin/env bash
# Fail if the generated game-kit workshop layouts, stylesheet, manifest or runtime descriptor
# (games/cs2/js/gamekit.generated.js) is stale.
#
# Bars, meters, themes and tones are class families (s2-w*, gk-acc-*, gk-t-*) and the panels are
# fixed id pools whose counts the server runtime must agree with: a server can only address panels
# and classes the client's layout already has, and Panorama ignores unknown ones silently. One
# generator owns both sides.
set -euo pipefail
cd "$(cd "$(dirname "$0")/.." && pwd)"
node scripts/gen-gamehud-layout.mjs --check
