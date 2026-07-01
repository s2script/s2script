#!/usr/bin/env bash
# Fails (exit 1) if s2script-core depends, directly or transitively, on any games/* crate.
set -euo pipefail
cd "$(dirname "$0")/.."

# Names of all packages whose manifest lives under games/
mapfile -t GAME_PKGS < <(
  cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import sys,json; m=json.load(sys.stdin); [print(p["name"]) for p in m["packages"] if "/games/" in p["manifest_path"]]'
)

# The full normal-dependency closure of s2script-core
DEPS="$(cargo tree -p s2script-core --edges normal --prefix none | awk '{print $1}' | sort -u)"

violation=0
for g in "${GAME_PKGS[@]}"; do
  if grep -qx "$g" <<<"$DEPS"; then
    echo "BOUNDARY VIOLATION: s2script-core depends on game package '$g'" >&2
    violation=1
  fi
done

if [ "$violation" -ne 0 ]; then exit 1; fi

# --- CS2 name-leak gate: core/ must contain no CS2 identifier (engine-generic only). ---
# Patterns are CS2 schema/game identifiers that must live only in games/cs2 (JS) or gamedata.
NAME_LEAK_RE='CCSPlayer|CCSPlayerPawn|CCSPlayerController|m_iHealth|m_hPlayerPawn'
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

echo "core boundary OK: s2script-core depends on no games/* crate"
