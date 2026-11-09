#!/usr/bin/env bash
# Compile + run the cvar value-store unit test (Server.setCvar must write only the type's own bytes)
# with the host compiler. cvar_value.h has no SDK or engine contact, so no sniper container is needed.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)/cvar_value_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra -D_GLIBCXX_DEBUG)

# Probe for the sanitizer runtime rather than assuming it: a missing libasan must not fail the gate.
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  echo "   (with -fsanitize=address,undefined)"
else
  echo "   (no sanitizer runtime — _GLIBCXX_DEBUG only)"
fi

g++ "${flags[@]}" -I shim/src -o "$out" shim/tests/cvar_value_test.cpp
"$out"
