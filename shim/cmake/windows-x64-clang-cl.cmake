# CMake toolchain: cross-build the shim for Windows x64 from Linux with clang-cl + lld-link against
# the MSVC CRT and Windows SDK splatted by `xwin` (the same tree `cargo xwin` uses for core).
#
#   cmake -S shim -B build/shim-win64 -DCMAKE_TOOLCHAIN_FILE=shim/cmake/windows-x64-clang-cl.cmake \
#         -DXWIN_DIR=$HOME/.cache/cargo-xwin/xwin
#
# scripts/build-windows.sh drives this end to end.
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR AMD64)

if(NOT XWIN_DIR)
    set(XWIN_DIR "$ENV{XWIN_DIR}")
endif()
if(NOT XWIN_DIR)
    set(XWIN_DIR "$ENV{HOME}/.cache/cargo-xwin/xwin")
endif()
if(NOT EXISTS "${XWIN_DIR}/crt/include")
    message(FATAL_ERROR "XWIN_DIR (${XWIN_DIR}) has no MSVC CRT; run `xwin splat` or `cargo xwin` first")
endif()
# Try-compile projects re-read this file; pass the location through.
set(CMAKE_TRY_COMPILE_PLATFORM_VARIABLES XWIN_DIR)

find_program(S2_CLANG_CL NAMES clang-cl-19 clang-cl-18 clang-cl REQUIRED)
find_program(S2_LLD_LINK NAMES lld-link-19 lld-link-18 lld-link REQUIRED)
find_program(S2_LLVM_LIB NAMES llvm-lib-19 llvm-lib-18 llvm-lib REQUIRED)
find_program(S2_LLVM_RC NAMES llvm-rc-19 llvm-rc-18 llvm-rc)
set(CMAKE_C_COMPILER "${S2_CLANG_CL}")
set(CMAKE_CXX_COMPILER "${S2_CLANG_CL}")
set(CMAKE_LINKER "${S2_LLD_LINK}")
set(CMAKE_AR "${S2_LLVM_LIB}")
if(S2_LLVM_RC)
    set(CMAKE_RC_COMPILER "${S2_LLVM_RC}")
endif()

set(_s2_sys_includes
    "/imsvc${XWIN_DIR}/crt/include"
    "/imsvc${XWIN_DIR}/sdk/include/ucrt"
    "/imsvc${XWIN_DIR}/sdk/include/um"
    "/imsvc${XWIN_DIR}/sdk/include/shared")
string(JOIN " " _s2_sys_includes ${_s2_sys_includes})
set(CMAKE_C_FLAGS_INIT "--target=x86_64-pc-windows-msvc ${_s2_sys_includes}")
set(CMAKE_CXX_FLAGS_INIT "--target=x86_64-pc-windows-msvc ${_s2_sys_includes}")

set(_s2_libpaths
    "/libpath:${XWIN_DIR}/crt/lib/x86_64"
    "/libpath:${XWIN_DIR}/sdk/lib/um/x86_64"
    "/libpath:${XWIN_DIR}/sdk/lib/ucrt/x86_64")
string(JOIN " " _s2_libpaths ${_s2_libpaths})
set(CMAKE_EXE_LINKER_FLAGS_INIT "/manifest:no ${_s2_libpaths}")
set(CMAKE_SHARED_LINKER_FLAGS_INIT "/manifest:no ${_s2_libpaths}")
set(CMAKE_MODULE_LINKER_FLAGS_INIT "/manifest:no ${_s2_libpaths}")

# xwin splats the release CRT only: try-compiles and every target link against it.
set(CMAKE_TRY_COMPILE_CONFIGURATION Release)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")

set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
