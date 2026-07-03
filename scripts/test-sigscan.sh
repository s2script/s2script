#!/usr/bin/env bash
# Compile + run the pure pattern-scanner unit test with the host compiler (no sniper container,
# no SDK — sigscan.{h,cpp} are self-contained). Slice 5D.2 Thread A gate.
set -euo pipefail
cd "$(dirname "$0")/.."
out="$(mktemp -d)/sigscan_test"
g++ -std=c++17 -O2 -Wall -Wextra -o "$out" shim/src/sigscan.cpp shim/tests/sigscan_test.cpp
"$out"
