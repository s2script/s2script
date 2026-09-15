#!/usr/bin/env bash
# Compile + run the engine-free shim unload coordinator test with the host compiler
# (no SDK, no game, no libkhook / SafetyHook).
#
# The sanitizers are not decoration: a coordinator that shuts down V8 while an
# original is between PRE/POST, or that finishes twice, is exactly the F1 shape.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)/khook_shutdown_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra -pthread)

if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  echo "   (with -fsanitize=address,undefined)"
else
  echo "   (no sanitizer runtime)"
fi

g++ "${flags[@]}" \
    -I shim/src \
    -isystem third_party/metamod-source/third_party/khook/include \
    -o "$out" shim/tests/khook_shutdown_test.cpp
"$out"
