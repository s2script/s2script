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
    src/gamedata.cpp
    src/sigscan.cpp
    # Slice 5D.1: a self-contained MurmurHash2LowerCase (the ONE tier1 symbol CKV3MemberName(const char*)
    # needs to key IGameEvent field reads) — avoids linking all of tier1 (CUtlString/UtlVectorMemory cascade).
    src/tier1_shims.cpp
)

target_include_directories(s2script PRIVATE
    src
    # sdk_stubs must come before ${HL2SDK}/public so that the compiler finds
    # our stub network_connection.pb.h before hitting the missing generated file.
    ${CMAKE_SOURCE_DIR}/src/sdk_stubs
    ${MMS}/core
    ${MMS}/core/sourcehook
    ${AMTL}
    ${HL2SDK}/public
    ${HL2SDK}/public/tier0
    ${HL2SDK}/public/tier1
    ${HL2SDK}/public/mathlib
    ${HL2SDK}/public/appframework
    # protobuf 3.21.8 runtime headers (Slice 6.1c) — reflection over the game's usermessage
    # protobuf objects (CUserMessageSayText2). Matches the vendored libprotobuf.a below.
    ${HL2SDK}/thirdparty/protobuf-3.21.8/src
)

# Source 2 / CS2 build defines.
# META_NO_HL2SDK: keeps ISmmPlugin.h from pulling in HL2SDK types;
#   the PLUGIN_EXPOSE macro then emits a plain extern-"C" CreateInterface symbol.
# _GLIBCXX_USE_CXX11_ABI=0: matches Valve's prebuilt ABI (needed if we ever link tier0/tier1).
target_compile_definitions(s2script PRIVATE
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

target_compile_options(s2script PRIVATE
    -m64
    -fvisibility=hidden
)

# Link the resident Rust core (built by `cargo build --release`).
# libprotobuf.a (Slice 6.1c): the vendored, self-contained protobuf 3.21.8 static lib — provides the
# reflection symbols (Descriptor::FindFieldByName / Reflection::Set*) for client_print's SayText2 send.
# Self-contained (no undefined deps), so it does NOT reintroduce the tier1 dlopen cascade (5D.1). Built
# with Valve's ABI (_GLIBCXX_USE_CXX11_ABI=0, set above), matching the game's in-process runtime.
target_link_libraries(s2script PRIVATE
    ${CMAKE_SOURCE_DIR}/../target/release/libs2script_core.so
    ${HL2SDK}/lib/linux64/release/libprotobuf.a
)
target_include_directories(s2script PRIVATE include)
# Ensure the runtime can find libs2script_core.so next to the plugin.
# Output as s2script.so (no lib prefix) to match Metamod's loader expectations.
set_target_properties(s2script PROPERTIES
    PREFIX ""
    OUTPUT_NAME "s2script"
    BUILD_RPATH "$ORIGIN"
    INSTALL_RPATH "$ORIGIN"
    BUILD_WITH_INSTALL_RPATH TRUE
)
