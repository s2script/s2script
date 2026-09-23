#!/usr/bin/env bash
# Production callbacks/accessors with an injected provider; stock JIT proof lives in the probe.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
compiler="${CXX:-c++}"
flags=(-std=c++17 -O1 -g -Wall -Wextra -pthread -Ishim/src -Ithird_party/hde
       -isystem third_party/metamod-source/third_party/khook/include)
if printf 'int main() { return 0; }\n' | "$compiler" -x c++ - -fsanitize=address,undefined -o "$tmp/probe" >/dev/null 2>&1 && "$tmp/probe"; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)
  if [[ "$(uname -s)" == Linux ]]; then flags+=(-no-pie); fi
  echo 'engine_hook_invocation: ASan + UBSan enabled'
fi
hde_flags=()
case "$(uname -m)" in arm64|aarch64) hde_flags+=(-D_M_X64);; esac
"$compiler" "${flags[@]}" ${hde_flags[@]+"${hde_flags[@]}"} -fno-sanitize=alignment \
  -x c++ -c third_party/hde/hde64.c -o "$tmp/hde64.o"
libs=()
if [[ "$(uname -s)" == Linux ]]; then libs+=(-ldl); fi
"$compiler" "${flags[@]}" shim/src/engine_hooks.cpp shim/src/hook_dispatch.cpp \
  shim/src/call_validate.cpp shim/src/original_module.cpp shim/src/sigscan.cpp \
  shim/tests/engine_hook_invocation_test.cpp "$tmp/hde64.o" \
  ${libs[@]+"${libs[@]}"} -o "$tmp/engine_hook_invocation_test"
"$tmp/engine_hook_invocation_test"
