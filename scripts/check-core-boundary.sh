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

# Name-leak + games/-embed gates. They live in their own script (pure greps, no cargo) so
# test-boundary-nameleak.sh can re-run them without repeating the dependency walk above.
bash "$(dirname "$0")/check-core-names.sh"

echo "core boundary OK: s2script-core depends on no games/* crate"
