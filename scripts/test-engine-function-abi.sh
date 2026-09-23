#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3 scripts/gen-engine-function-abi.py --check
mkdir -p build/engine-function-abi
if [[ ${1:-} == --stock-provider ]]; then
  [[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'UNSUPPORTED platform: stock provider requires linux-x86_64-sysv' >&2; exit 2; }
  for source in third_party/libffi third_party/metamod-source third_party/metamod-source/third_party/khook third_party/metamod-source/third_party/khook/third_party/safetyhook; do
    printf '%s ' "$source"
    git -C "$source" rev-parse HEAD
    [[ -z $(git -C "$source" status --porcelain --untracked-files=no) ]] || { echo "FAIL modified source: $source" >&2; exit 1; }
  done
  cmake -S tools/engine-function-probe -B build/engine-function-abi -DCMAKE_BUILD_TYPE=Debug
  cmake --build build/engine-function-abi --target engine_function_abi_test engine_function_v8_bridge -j2
  if [[ ${S2FN_ABI_GDB:-0} == 1 ]]; then
    command -v gdb >/dev/null || { echo 'gdb required for S2FN_ABI_GDB=1' >&2; exit 2; }
    # Run exactly the same matrix. Parent mode preserves the intentional child
    # lifetime-refusal test; the debugger must never turn a crash into success.
    gdb --batch --return-child-result -ex 'set pagination off' \
      -ex 'set follow-fork-mode parent' -ex 'set detach-on-fork on' \
      -ex run -ex 'thread apply all bt full' -ex 'info registers' \
      -ex 'x/24i $pc-32' --args build/engine-function-abi/engine_function_abi_test
  else
    build/engine-function-abi/engine_function_abi_test
  fi
else
  c++ -std=c++17 -Wall -Wextra -Werror -DS2FN_VALIDATION_ONLY -Ishim/src \
    shim/src/engine_function_abi.cpp shim/tests/engine_function_abi_test.cpp \
    -o build/engine-function-abi/signatures
  build/engine-function-abi/signatures
  echo 'Portable normalization only; runtime CIF/provider proof requires --stock-provider.'
fi
