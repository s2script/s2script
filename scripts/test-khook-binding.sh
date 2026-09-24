#!/usr/bin/env bash
# Compile + run the engine-free checked KHook binding test with the host compiler
# (no SDK, no game, no libkhook / SafetyHook).
#
# The sanitizers are not decoration: retirement/completion races and this-filter
# undo on INVALID_HOOK are exactly the shape a use-after-free or missed unlock
# would hide until a live callback. ASan/UBSan turn that into a deterministic abort.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)/khook_binding_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra -pthread)

# Probe for the sanitizer runtime rather than assuming it: a missing libasan must not fail the gate.
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  if [[ "$(uname -s)" == Linux ]]; then
    # GCC 10 ASan can fail before main when PIE lands at an incompatible Linux address.
    flags+=(-no-pie)
  fi
  echo "   (with -fsanitize=address,undefined)"
else
  echo "   (no sanitizer runtime)"
fi

g++ "${flags[@]}" \
    -I shim/src \
    -isystem third_party/metamod-source/third_party/khook/include \
    -o "$out" shim/tests/khook_binding_test.cpp
"$out"
