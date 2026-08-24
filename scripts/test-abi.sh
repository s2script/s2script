#!/usr/bin/env bash
# Compile the engine-free outbound-call and inbound-hook ABI probes twice on x86-64 Linux:
# once through the native SysV backend, and once through GCC's ms_abi support. The latter makes the
# compiler emit the Microsoft x64 register, shadow-space and stack convention without needing a
# Windows runner, so mixed positional GP/XMM assignment is a behavioural gate rather than a source
# inspection.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
CXX="${CXX:-g++}"
flags=(-std=c++17 -O0 -g -Wall -Wextra -I shim/src)

build_and_run() {
  local name="$1"
  local backend="$2"
  shift 2
  "$CXX" "${flags[@]}" "$@" \
    shim/tests/abi_test.cpp \
    "shim/src/call_abi_${backend}.cpp" \
    "shim/src/hook_abi_${backend}.cpp" \
    shim/src/hook_dispatch.cpp \
    -o "$tmp/$name"
  "$tmp/$name"
}

build_and_run sysv sysv
build_and_run microsoft win -DS2_ABI_PROBE_MICROSOFT=1

echo "test-abi: both SysV and Microsoft x64 probes passed"
