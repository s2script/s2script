#!/usr/bin/env bash
# Compile + run the deferred-dispatch queue unit test with the host compiler (no sniper container,
# no SDK — defer_queue.cpp's only outside contact is the injected S2DeferOps struct).
#
# _GLIBCXX_DEBUG is not decoration: the defect this test pins is a buffer cleared out from under the
# drain's own walk, and libstdc++'s debug iterators turn that into a deterministic abort instead of
# whatever the heap happens to contain. ASan/UBSan are added when the toolchain has them (they catch
# the same class through the freed std::string buffers), but the gate does not depend on them being
# installed.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)/defer_queue_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra -D_GLIBCXX_DEBUG)

# Probe for the sanitizer runtime rather than assuming it: a missing libasan must not fail the gate.
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  echo "   (with -fsanitize=address,undefined)"
else
  echo "   (no sanitizer runtime — _GLIBCXX_DEBUG only)"
fi

g++ "${flags[@]}" -I shim/include -I shim/src \
    -o "$out" shim/src/defer_queue.cpp shim/tests/defer_queue_test.cpp
"$out"
