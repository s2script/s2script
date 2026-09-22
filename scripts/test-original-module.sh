#!/usr/bin/env bash
# Engine-free ELF fixtures everywhere; real loaded-file identity checks on Linux.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
compiler="${CXX:-c++}"
flags=(-std=c++17 -O1 -g -Wall -Wextra -Werror -Ishim/src)
# Probe the sanitizer runtime too: an accepted compiler flag alone is insufficient.
if printf 'int main() { return 0; }\n' | "$compiler" -x c++ - -fsanitize=address,undefined -o "$tmp/probe" >/dev/null 2>&1 && "$tmp/probe"; then
  flags+=(-fsanitize=address,undefined -fno-omit-frame-pointer)
  echo 'original_module: ASan + UBSan enabled'
else
  echo 'original_module: SKIP sanitizers (compiler/runtime unavailable)'
fi
libs=()
args=()
if [[ "$(uname -s)" == Linux ]]; then
  printf 'extern "C" int original_module_fixture() { return 42; }\n' > "$tmp/fixture.cpp"
  "$compiler" -shared -fPIC -Wl,--build-id -o "$tmp/liboriginal_fixture_proxy.so" "$tmp/fixture.cpp"
  # Extra executable bytes make the real-image candidate unambiguously larger.
  printf 'asm(".text\\n.space 8192, 0x90\\n");\n' >> "$tmp/fixture.cpp"
  "$compiler" -shared -fPIC -Wl,--build-id -o "$tmp/liboriginal_fixture.so" "$tmp/fixture.cpp"
  cp "$tmp/liboriginal_fixture.so" "$tmp/replacement.so"
  libs+=(-ldl)
  args+=("$tmp/liboriginal_fixture.so" "$tmp/replacement.so" "$tmp/liboriginal_fixture_proxy.so")
fi
"$compiler" "${flags[@]}" shim/src/original_module.cpp shim/tests/original_module_test.cpp \
  ${libs[@]+"${libs[@]}"} -o "$tmp/original_module_test"
"$tmp/original_module_test" ${args[@]+"${args[@]}"}
