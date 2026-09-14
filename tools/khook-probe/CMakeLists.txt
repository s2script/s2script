cmake_minimum_required(VERSION 3.20)
project(s2_khook_probe C CXX)

# Separate Metamod test plugin. NEVER part of the production s2script release.
# Configure from the repo root:
#   cmake -S tools/khook-probe -B build/khook-probe -DS2_SOURCE_DIR="$PWD"

set(S2_SOURCE_DIR "" CACHE PATH "s2script repository root")
if(NOT S2_SOURCE_DIR)
    get_filename_component(S2_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
endif()
if(NOT EXISTS "${S2_SOURCE_DIR}/shim/CMakeLists.txt")
    message(FATAL_ERROR "S2_SOURCE_DIR=${S2_SOURCE_DIR} does not look like the s2script repo")
endif()

set(HL2SDK "${S2_SOURCE_DIR}/third_party/hl2sdk")
set(MMS    "${S2_SOURCE_DIR}/third_party/metamod-source")
set(AMTL   "${MMS}/third_party/amtl")
set(KHOOK  "${MMS}/third_party/khook")

foreach(req IN ITEMS
        "${MMS}/core/ISmmPlugin.h"
        "${KHOOK}/include/khook.hpp"
        "${HL2SDK}/public/eiface.h"
        "${S2_SOURCE_DIR}/shim/src/khook_binding.h")
    if(NOT EXISTS "${req}")
        message(FATAL_ERROR "missing ${req} — init metamod-source (with khook) and hl2sdk, or pass -DS2_SOURCE_DIR")
    endif()
endforeach()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

add_library(s2_khook_probe SHARED
    plugin.cpp
    "${S2_SOURCE_DIR}/shim/src/sigscan.cpp"
)

target_include_directories(s2_khook_probe PRIVATE
    "${S2_SOURCE_DIR}/shim/src"
    "${S2_SOURCE_DIR}/shim/src/sdk_stubs"
    "${MMS}/core"
    "${KHOOK}/include"
    "${AMTL}"
    "${HL2SDK}/public"
    "${HL2SDK}/public/tier0"
    "${HL2SDK}/public/tier1"
    "${HL2SDK}/public/mathlib"
    "${HL2SDK}/public/appframework"
    "${HL2SDK}/game/shared"
    "${HL2SDK}/thirdparty/protobuf-3.21.8/src"
)

# Same SteamRT / Valve ABI conventions as shim/CMakeLists.txt.
target_compile_definitions(s2_khook_probe PRIVATE
    LINUX
    _LINUX
    POSIX
    COMPILER_GCC
    PLATFORM_64BITS
    META_NO_HL2SDK
    _FILE_OFFSET_BITS=64
    _GLIBCXX_USE_CXX11_ABI=0
    stricmp=strcasecmp
    strnicmp=strncasecmp
    _stricmp=strcasecmp
    _vsnprintf=vsnprintf
)

target_compile_options(s2_khook_probe PRIVATE
    -m64
    -fvisibility=hidden
)

set_target_properties(s2_khook_probe PROPERTIES
    PREFIX ""
    OUTPUT_NAME "s2_khook_probe"
    BUILD_RPATH "$ORIGIN"
    INSTALL_RPATH "$ORIGIN"
    BUILD_WITH_INSTALL_RPATH TRUE
)

set(PROBE_VDF "${CMAKE_CURRENT_BINARY_DIR}/s2_khook_probe.vdf")
file(WRITE "${PROBE_VDF}"
"\"Metamod Plugin\"
{
\t\"alias\"\t\"khook_probe\"
\t\"file\"\t\"addons/s2script/bin/linuxsteamrt64/s2_khook_probe\"
}
")

# Staging layout used by scripts/test-khook-live.sh (not package-addon / package-release).
set(PROBE_STAGE "${S2_SOURCE_DIR}/build/khook-probe/stage")
add_custom_command(TARGET s2_khook_probe POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E make_directory
        "${PROBE_STAGE}/addons/s2script/bin/linuxsteamrt64"
        "${PROBE_STAGE}/addons/metamod"
    COMMAND ${CMAKE_COMMAND} -E copy
        "$<TARGET_FILE:s2_khook_probe>"
        "${PROBE_STAGE}/addons/s2script/bin/linuxsteamrt64/s2_khook_probe.so"
    COMMAND ${CMAKE_COMMAND} -E copy
        "${PROBE_VDF}"
        "${PROBE_STAGE}/addons/metamod/s2_khook_probe.vdf"
    COMMENT "stage khook-probe .so + test VDF (not a production package)"
)
