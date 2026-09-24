#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3 scripts/gen-engine-function-abi.py --check
mkdir -p build/engine-function-abi
check_layout() {
  "$1"
  python3 - "$1" <<'PY'
import resource
import signal
import subprocess
import sys
resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
for mode, name in [("--reject-local", "deliberately-local-target"),
                   ("--reject-local-inventory", "identity_i32")]:
    result = subprocess.run([sys.argv[1], mode], capture_output=True, text=True)
    if (result.returncode != -signal.SIGABRT
            or f"fixture-boundary=rejected name={name} " not in result.stderr
            or "reached Configure" in result.stderr):
        raise SystemExit(f"FAIL pre-Configure rejection {mode}: {result.returncode}\n{result.stdout}\n{result.stderr}")
    print(f"PASS {mode}: rejected before Configure (SIGABRT, core disabled)")
PY
}
if [[ ${1:-} == --stock-provider ]]; then
  [[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'UNSUPPORTED platform: stock provider requires linux-x86_64-sysv' >&2; exit 2; }
  for source in third_party/libffi third_party/metamod-source third_party/metamod-source/third_party/khook third_party/metamod-source/third_party/khook/third_party/safetyhook; do
    printf '%s ' "$source"
    git -C "$source" rev-parse HEAD
    [[ -z $(git -C "$source" status --porcelain --untracked-files=no) ]] || { echo "FAIL modified source: $source" >&2; exit 1; }
  done
  cmake -S tools/engine-function-probe -B build/engine-function-abi -DCMAKE_BUILD_TYPE=Debug
  cmake --build build/engine-function-abi --target engine_function_abi_test engine_function_v8_bridge engine_function_fixture_layout_test -j2
  python3 scripts/check-engine-function-fixture.py build/engine-function-abi/libengine_function_member_fixture.so \
    | tee build/engine-function-abi/fixture-elf-boundary.txt
  check_layout build/engine-function-abi/engine_function_fixture_layout_test
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
  # Exercise the real native target DSO and guard on the host without a provider.
  # Keep these portable artifacts separate from the three frozen Linux binaries.
  layout="$PWD/build/engine-function-fixture-layout"
  mkdir -p "$layout"
  c++ -std=c++17 -Wall -Wextra -Werror -fPIC -fvisibility=hidden -fvisibility-inlines-hidden \
    -shared -isystem third_party/metamod-source/third_party/khook/include \
    shim/tests/engine_function_member_fixture.cpp -o "$layout/libengine_function_member_fixture.so"
  link_flags=(-std=c++17 -Wall -Wextra -Werror)
  [[ $(uname -s) != Linux ]] || link_flags+=(-ldl)
  c++ shim/tests/engine_function_fixture_layout_test.cpp \
    "$layout/libengine_function_member_fixture.so" "${link_flags[@]}" -o "$layout/layout"
  check_layout "$layout/layout"
  echo 'Portable normalization and fixture layout only; runtime CIF/provider proof requires --stock-provider.'
fi
