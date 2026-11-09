#!/usr/bin/env bash
# Compile + run the console-capture policy unit test (Server.commandEx / sm_rcon output) with the
# host compiler. console_capture.cpp has no SDK or engine contact, so no sniper container is needed.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)/console_capture_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra -D_GLIBCXX_DEBUG)

# Probe for the sanitizer runtime rather than assuming it: a missing libasan must not fail the gate.
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  echo "   (with -fsanitize=address,undefined)"
else
  echo "   (no sanitizer runtime — _GLIBCXX_DEBUG only)"
fi

g++ "${flags[@]}" -I shim/src \
    -o "$out" shim/src/console_capture.cpp shim/tests/console_capture_test.cpp
"$out"
