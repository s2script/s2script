#!/usr/bin/env bash
# Run the shipped recipe evaluator over verified ELF bytes and bounded live fixtures.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
compiler="${CXX:-c++}"
flags=(-std=c++17 -O1 -g -Wall -Wextra -Ishim/src -Ithird_party/hde)
if printf 'int main() { return 0; }\n' | "$compiler" -x c++ - -fsanitize=address,undefined -o "$tmp/probe" >/dev/null 2>&1 && "$tmp/probe"; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)
  echo 'engine_resolver: ASan + UBSan enabled'
fi
# HDE is a byte decoder, also usable by little-endian arm64 fixture hosts. This
# define only enables its guarded TU; it changes no vendored source/production flags.
hde_flags=()
case "$(uname -m)" in arm64|aarch64) hde_flags+=(-D_M_X64);; esac
"$compiler" "${flags[@]}" ${hde_flags[@]+"${hde_flags[@]}"} -fno-sanitize=alignment \
  -x c++ -c third_party/hde/hde64.c -o "$tmp/hde64.o"
libs=()
if [[ "$(uname -s)" == Linux ]]; then libs+=(-ldl); fi
"$compiler" "${flags[@]}" -DS2_RESOLVER_ENGINE_FREE \
  shim/src/engine_resolver.cpp shim/src/original_module.cpp shim/src/sigscan.cpp \
  shim/src/call_validate.cpp shim/src/vtable.cpp shim/tests/engine_resolver_test.cpp "$tmp/hde64.o" \
  ${libs[@]+"${libs[@]}"} -o "$tmp/engine_resolver_test"
"$tmp/engine_resolver_test"
