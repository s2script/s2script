#!/usr/bin/env bash
# Compile and run the engine-free platform contract against real Linux backends plus synthetic PE.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)/platform_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra)
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  echo "   (with -fsanitize=address,undefined)"
else
  echo "   (no sanitizer runtime)"
fi

g++ "${flags[@]}" -I shim/src -o "$out" \
    shim/tests/platform_test.cpp \
    shim/src/platform/paths_common.cpp \
    shim/src/platform/paths_linux.cpp \
    shim/src/platform/module_linux.cpp \
    shim/src/platform/memory_linux.cpp \
    shim/src/platform/pe_image.cpp \
    -ldl
"$out"
