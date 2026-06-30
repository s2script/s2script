cmake_minimum_required(VERSION 3.20)
project(s2script_shim CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# SDK paths (relative to shim/ source dir, so go up one level to repo root)
set(HL2SDK ${CMAKE_SOURCE_DIR}/../third_party/hl2sdk)
set(MMS    ${CMAKE_SOURCE_DIR}/../third_party/metamod-source)
set(AMTL   ${MMS}/third_party/amtl)

add_library(s2script SHARED
    src/s2script_mm.cpp
)

target_include_directories(s2script PRIVATE
    src
    ${MMS}/core
    ${MMS}/core/sourcehook
    ${AMTL}
    ${HL2SDK}/public
    ${HL2SDK}/public/tier0
    ${HL2SDK}/public/tier1
)

# Source 2 / CS2 build defines.
# META_NO_HL2SDK: keeps ISmmPlugin.h from pulling in HL2SDK types;
#   the PLUGIN_EXPOSE macro then emits a plain extern-"C" CreateInterface symbol.
# _GLIBCXX_USE_CXX11_ABI=0: matches Valve's prebuilt ABI (needed if we ever link tier0/tier1).
target_compile_definitions(s2script PRIVATE
    _LINUX
    POSIX
    COMPILER_GCC
    PLATFORM_64BITS
    META_NO_HL2SDK
    _GLIBCXX_USE_CXX11_ABI=0
    stricmp=strcasecmp
    strnicmp=strncasecmp
    _stricmp=strcasecmp
    _vsnprintf=vsnprintf
)

target_compile_options(s2script PRIVATE
    -m64
    -fvisibility=hidden
)

# Link the resident Rust core (built by `cargo build --release`).
target_link_libraries(s2script PRIVATE
    ${CMAKE_SOURCE_DIR}/../target/release/libs2script_core.so
)
target_include_directories(s2script PRIVATE include)
# Ensure the runtime can find libs2script_core.so next to the plugin.
# Output as s2script.so (no lib prefix) to match Metamod's loader expectations.
set_target_properties(s2script PROPERTIES
    PREFIX ""
    OUTPUT_NAME "s2script"
    BUILD_RPATH "$ORIGIN"
    INSTALL_RPATH "$ORIGIN"
)
