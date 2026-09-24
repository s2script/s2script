#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
build_dir="$(mktemp -d)"
trap 'rm -rf "$build_dir"' EXIT
flags=(-std=c++17 -O1 -g -Wall -Wextra -Werror)
if printf 'int main(){return 0;}' | ${CXX:-c++} -x c++ -fsanitize=address,undefined -o "$build_dir/probe" - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  echo "   (with -fsanitize=address,undefined)"
fi
libs=()
if [[ "$(uname -s)" == Linux ]]; then libs+=(-ldl); fi
${CXX:-c++} "${flags[@]}" -I shim/src -isystem shim/third_party shim/src/plugin_function_overrides.cpp shim/tests/plugin_function_overrides_test.cpp ${libs[@]+"${libs[@]}"} -o "$build_dir/test"
"$build_dir/test"
