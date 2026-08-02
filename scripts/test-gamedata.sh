#!/usr/bin/env bash
# Compile + run the owner-scoped gamedata loader unit test with the host compiler
# (no sniper container, no SDK — gamedata.cpp depends only on nlohmann/json).
set -euo pipefail
cd "$(dirname "$0")/.."
out="$(mktemp -d)/gamedata_test"
g++ -std=c++17 -O2 -Wall -Wextra -I shim/third_party \
    -o "$out" shim/src/gamedata.cpp shim/tests/gamedata_test.cpp
"$out"
