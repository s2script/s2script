#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
compiler="${CXX:-c++}"
flags=(-std=c++17 -O1 -g -Wall -Wextra -Werror -pthread -DS2FN_VALIDATION_ONLY -Ishim/src)
if echo 'int main(){return 0;}' | "$compiler" -x c++ -fsanitize=address,undefined -o "$tmp/sanitizer-probe" - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  echo 'Native copy gate with address/undefined sanitizers'
fi
"$compiler" "${flags[@]}" \
  shim/src/engine_function_copy.cpp shim/tests/engine_function_copy_test.cpp -o "$tmp/copy"
for scenario in capture indirect budget arena owner-bytes owner-values owner-rows values payload linux linux-denied linux-missing; do
  "$tmp/copy" "$scenario"
done
