#!/usr/bin/env bash
# CS2 name-leak + games/-embed gates over core/. Pure greps, no cargo — split out of
# check-core-boundary.sh so test-boundary-nameleak.sh can re-run them five times
# without repeating the cargo dependency walk. check-core-boundary.sh calls this.
set -euo pipefail
cd "$(dirname "$0")/.."

# --- CS2 name-leak gate: core/ must contain no CS2 identifier (engine-generic only). ---
# Patterns are CS2 schema/game identifiers that must live only in games/cs2 (JS) or gamedata.
NAME_LEAK_RE='CCSPlayer|CCSPlayerPawn|CCSPlayerController|m_iHealth|m_hPlayerPawn|ProcessUsercmds|CSGOUserCmdPB|CBaseUserCmdPB|subtick_moves|buttons_pb'
if grep -rInE "$NAME_LEAK_RE" core/src 2>/dev/null; then
  echo "BOUNDARY VIOLATION: CS2 identifier found in core/ (must live in games/cs2 or gamedata)" >&2
  exit 1
fi

# --- include_str!/include_bytes! gate: core/ must never embed a file from games/ at compile time. ---
# This closes the gap the Slice-4 regression exploited (core include_str!-ing games/cs2/js/pawn.js).
if grep -rInE 'include_(str|bytes)!\s*\(\s*"[^"]*games/' core/src 2>/dev/null; then
  echo "BOUNDARY VIOLATION: core/ embeds a games/ file via include_str!/include_bytes! (core must stay engine-generic)" >&2
  exit 1
fi

echo "core name gates OK: no CS2 identifier and no games/ embed in core/"
