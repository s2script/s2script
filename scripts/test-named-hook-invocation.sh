#!/usr/bin/env bash
# Production named callbacks with an injected provider; stock JIT proof lives in the probe.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
compiler="${CXX:-c++}"
flags=(-std=c++17 -O1 -g -Wall -Wextra -pthread -Ishim/src
       -isystem third_party/metamod-source/third_party/khook/include)
if printf 'int main() { return 0; }\n' | "$compiler" -x c++ - -fsanitize=address,undefined -o "$tmp/probe" >/dev/null 2>&1 && "$tmp/probe"; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)
  if [[ "$(uname -s)" == Linux ]]; then flags+=(-no-pie); fi
  echo 'named_hook_invocation: ASan + UBSan enabled'
fi
"$compiler" "${flags[@]}" shim/src/named_hooks.cpp shim/tests/named_hook_invocation_test.cpp \
  -o "$tmp/named_hook_invocation_test"
"$tmp/named_hook_invocation_test"
