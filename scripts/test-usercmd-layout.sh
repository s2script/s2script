#!/usr/bin/env bash
# Compile + run the engine-free usercmd layout test (direct field reads + the trust check).
set -euo pipefail
cd "$(dirname "$0")/.."
out="$(mktemp -d)/usercmd_layout_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra)
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
fi
g++ "${flags[@]}" -I shim/src -o "$out" shim/tests/usercmd_layout_test.cpp
"$out"
