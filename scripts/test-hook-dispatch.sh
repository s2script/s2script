#!/usr/bin/env bash
# Compile + run the engine-free hook dispatch policy test with the host compiler (no SDK, no game).
#
# The sanitizers are not decoration: S2Hook_BypassArm/BypassTake index a fixed-size array with a
# hook id that arrives from OUTSIDE this TU, which is exactly the shape a sloppy bounds check reads
# or writes past the end of. ASan/UBSan turn that into a deterministic abort instead of silent
# corruption of whatever static data happens to sit next to the array.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)/hook_dispatch_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra)

# Probe for the sanitizer runtime rather than assuming it: a missing libasan must not fail the gate.
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  echo "   (with -fsanitize=address,undefined)"
else
  echo "   (no sanitizer runtime)"
fi

g++ "${flags[@]}" -o "$out" shim/src/hook_dispatch.cpp shim/tests/hook_dispatch_test.cpp
"$out"
