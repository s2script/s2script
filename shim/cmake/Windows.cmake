# Windows x64 build of the shim (s2script.dll). Included from CMakeLists.txt when WIN32; expects the
# clang-cl toolchain in cmake/windows-x64-clang-cl.cmake (see scripts/build-windows.sh).
# Design and the two compatibility adjustments: docs/superpowers/specs/2026-11-18-windows-port-design.md.

if(NOT CMAKE_CXX_COMPILER_ID STREQUAL "Clang")
    message(FATAL_ERROR "The Windows shim is built with clang-cl (got ${CMAKE_CXX_COMPILER_ID}); "
                        "use scripts/build-windows.sh")
endif()

# Valve's prebuilt libprotobuf.lib is /MT (static CRT, FAILIFMISMATCH RuntimeLibrary=MT_StaticRelease),
# so everything linked into s2script.dll is too. Core is a separate DLL with its own CRT.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")

# Where cargo put core's import library (s2script_core.dll.lib).
set(S2_CORE_WIN_DIR "${CMAKE_SOURCE_DIR}/../target/x86_64-pc-windows-msvc/${S2_CORE_LIB_DIR}"
    CACHE PATH "directory holding s2script_core.dll.lib")

include(cmake/LibffiWin.cmake)

# public/tier0/memoverride.cpp is Linux-only here; memoverride_win.cpp carries its new/delete half.
set(S2_SHIM_SOURCES_WIN ${S2_SHIM_SOURCES})
list(REMOVE_ITEM S2_SHIM_SOURCES_WIN ${HL2SDK}/public/tier0/memoverride.cpp)
add_library(s2script SHARED ${S2_SHIM_SOURCES_WIN} src/platform_win.cpp src/memoverride_win.cpp
    src/win_link_stubs.cpp)

target_include_directories(s2script PRIVATE
    src
    ${CMAKE_SOURCE_DIR}/src/sdk_stubs
    include
    ${MMS}/core
    ${MMS}/third_party/khook/include
    ${AMTL}
    ${HL2SDK}/public
    ${HL2SDK}/public/engine2
    ${HL2SDK}/public/mathlib
    ${HL2SDK}/public/tier0
    ${HL2SDK}/public/entity2
    ${HL2SDK}/game/shared
    ${HL2SDK}/common
    ${HL2SDK}/thirdparty/protobuf-3.21.8/src
    ${CMAKE_SOURCE_DIR}/../third_party/hde
)
target_include_directories(s2script SYSTEM PRIVATE ${CMAKE_SOURCE_DIR}/third_party)

# The Source 2 defines Metamod's hl2sdk-manifests give the cs2 Windows build, plus:
#   strcasecmp/strncasecmp: the POSIX spellings the shim uses (the Linux build maps the other way).
#   __restrict: empty. Clang rejects calling a non-restrict member through a restrict `this`, which
#     Valve's headers do (Vector::IsZeroFast, bf_read). MSVC accepts it. Overriding RESTRICT itself is
#     not possible: the SDK defines it unconditionally, and clang-cl's MSVC include-stack lookup finds
#     the SDK's own copy ahead of any overlay. The qualifier only changes the mangled name of an
#     out-of-line restrict member, which would fail at link, never silently.
#   _ALLOW_COMPILER_AND_STL_VERSION_MISMATCH: the MSVC STL asks for Clang 19; CI runners ship 18.
target_compile_definitions(s2script PRIVATE
    WIN32
    _WIN32
    _WINDOWS
    COMPILER_MSVC
    COMPILER_MSVC64
    PLATFORM_64BITS
    X64BITS
    META_NO_HL2SDK
    NOMINMAX
    _CRT_SECURE_NO_WARNINGS
    _ITERATOR_DEBUG_LEVEL=0
    _ALLOW_COMPILER_AND_STL_VERSION_MISMATCH
    __restrict=
    strcasecmp=_stricmp
    strncasecmp=_strnicmp
)
target_compile_options(s2script PRIVATE
    /EHsc
    -Wno-keyword-macro            # the __restrict= define above, on purpose
    -Wno-microsoft-include
    -Wno-unused-command-line-argument
)

execute_process(
    COMMAND git -C ${HL2SDK} rev-parse --short HEAD
    OUTPUT_VARIABLE S2_HL2SDK_SHA OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
if(NOT S2_HL2SDK_SHA)
    set(S2_HL2SDK_SHA "unknown")
endif()
target_compile_definitions(s2script PRIVATE S2_HL2SDK_BUILD="${S2_HL2SDK_SHA}")

# Core is delay-loaded: Windows does not look in a plugin's own directory for its dependencies, so
# S2ScriptPlugin::Load preloads <bin>/s2script_core.dll by full path before the first core call, and
# the delay-load helper then binds to that already-loaded module by name.
target_link_libraries(s2script PRIVATE
    ${S2_CORE_WIN_DIR}/s2script_core.dll.lib
    ${HL2SDK}/lib/public/win64/tier0.lib
    ${HL2SDK}/lib/public/win64/2015/libprotobuf.lib
    s2_libffi
    delayimp
    psapi
)
target_link_options(s2script PRIVATE /DELAYLOAD:s2script_core.dll /OPT:REF /OPT:ICF)

set_target_properties(s2script PROPERTIES
    PREFIX ""
    OUTPUT_NAME "s2script"
)
