#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
compiler="${CXX:-c++}"
flags=(-std=c++17 -O1 -g -Wall -Wextra -Ishim/src -Ithird_party/hde)
hde_flags=()
case "$(uname -m)" in arm64|aarch64) hde_flags+=(-D_M_X64);; esac
"$compiler" "${flags[@]}" ${hde_flags[@]+"${hde_flags[@]}"} -x c++ -c third_party/hde/hde64.c -o "$tmp/hde64.o"
libs=()
if [[ "$(uname -s)" == Linux ]]; then libs+=(-ldl); fi
"$compiler" "${flags[@]}" -DS2FN_VALIDATION_ONLY -DS2_RESOLVER_ENGINE_FREE \
  shim/src/engine_function_bridge.cpp shim/src/engine_function_abi.cpp \
  shim/src/engine_resolver.cpp shim/src/original_module.cpp shim/src/sigscan.cpp \
  shim/src/call_validate.cpp shim/src/vtable.cpp shim/tests/engine_function_bridge_test.cpp "$tmp/hde64.o" \
  ${libs[@]+"${libs[@]}"} -o "$tmp/bridge"
"$tmp/bridge"
if [[ ${1:-} == --stock-provider ]]; then
  [[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'UNSUPPORTED platform: bridge runtime proof requires linux-x86_64-sysv' >&2; exit 2; }
  # Reuse the exact upstream provider inventory and private pinned libffi build.
  # The temporary driver adds only the bridge test target, not a provider fork.
  mkdir -p build/engine-function-bridge-driver
  cat > build/engine-function-bridge-driver/CMakeLists.txt <<CMAKE
cmake_minimum_required(VERSION 3.20)
project(engine_function_bridge_test C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
add_subdirectory("$PWD/tools/engine-function-probe" probe)
add_library(bridge_target_fixture SHARED "$PWD/shim/tests/engine_function_bridge_test.cpp")
target_compile_definitions(bridge_target_fixture PRIVATE S2BRIDGE_TARGET_FIXTURE)
target_compile_options(bridge_target_fixture PRIVATE -fvisibility=hidden -fno-gnu-unique)
add_executable(bridge_stock
  "$PWD/shim/src/engine_function_bridge.cpp"
  "$PWD/shim/src/engine_resolver.cpp" "$PWD/shim/src/original_module.cpp"
  "$PWD/shim/src/sigscan.cpp" "$PWD/shim/src/call_validate.cpp" "$PWD/shim/src/vtable.cpp"
  "$PWD/third_party/hde/hde64.c" "$PWD/shim/tests/engine_function_bridge_test.cpp")
target_compile_definitions(bridge_stock PRIVATE S2_RESOLVER_ENGINE_FREE)
target_include_directories(bridge_stock PRIVATE "$PWD/third_party/hde")
target_compile_options(bridge_stock PRIVATE -UNDEBUG -Wall -Wextra)
target_link_libraries(bridge_stock PRIVATE engine_function_adapter s2_libffi bridge_target_fixture)
target_link_options(bridge_stock PRIVATE "-Wl,--wrap=ffi_closure_alloc" "-Wl,--wrap=ffi_closure_free")
CMAKE
  cmake -S build/engine-function-bridge-driver -B build/engine-function-bridge -DCMAKE_BUILD_TYPE=Debug
  cmake --build build/engine-function-bridge --target bridge_stock -j"${S2_BUILD_JOBS:-2}"
  build/engine-function-bridge/bridge_stock
else
  echo 'Portable bridge normalization/resolver fixtures only; real CIF/stock hook tests require --stock-provider.'
fi
