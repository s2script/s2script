#!/usr/bin/env bash
# Compile + run the engine-free hook dispatch policy test with the host compiler (no SDK, no game).
set -euo pipefail
cd "$(dirname "$0")/.."
out="$(mktemp -d)/hook_dispatch_test"
g++ -std=c++17 -O2 -Wall -Wextra -o "$out" shim/src/hook_dispatch.cpp shim/tests/hook_dispatch_test.cpp
"$out"
