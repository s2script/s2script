cmake_minimum_required(VERSION 3.20)
project(s2script_shim C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# SDK paths (relative to shim/ source dir, so go up one level to repo root)
set(HL2SDK ${CMAKE_SOURCE_DIR}/../third_party/hl2sdk)
set(MMS    ${CMAKE_SOURCE_DIR}/../third_party/metamod-source)
set(AMTL   ${MMS}/third_party/amtl)

add_library(s2script SHARED
    src/s2script_mm.cpp
    # Entity lifecycle listeners slice: the isolated IEntityListener TU (mirrors ekv.cpp's isolation
    # discipline — the only new TU that includes entity2/entitysystem.h for the listener interface).
    src/entity_listener.cpp
    src/gamedata.cpp
    src/sigscan.cpp
    # Slice 5D.1: a self-contained MurmurHash2LowerCase (the ONE tier1 symbol CKV3MemberName(const char*)
    # needs to key IGameEvent field reads) — avoids linking all of tier1 (CUtlString/UtlVectorMemory cascade).
    src/tier1_shims.cpp
    # Slice 6.6: a minimal self-contained x86-64 inline detour (the damage hook) + the vendored HDE
    # length disassembler (pure, no OS deps). No third-party detour library / no dlopen cascade.
    src/detour.cpp
    ${CMAKE_SOURCE_DIR}/../third_party/hde/hde64.c
    # Ray-trace slice: RTTI vtable-by-name resolution (ported DynLibUtils algorithm) + the
    # CNavPhysicsInterface::TraceShape structs/call. Engine-generic (Source-2 physics).
    src/vtable.cpp
    src/trace.cpp
    # EKV slice: compile the SDK's own CEntityKeyValues implementation into the shim (the CSSharp
    # approach — its CMakeLists compiles exactly entity2/entitykeyvalues.cpp). keyvalues3.cpp uses
    # plain libc malloc/free (verified), NOT g_pMemAlloc. ekv.cpp is the ONLY TU that includes
    # entitykeyvalues.h; everything else sees void*.
    src/ekv.cpp
    # Crash-reporter slice: Breakpad arming + the async-signal-safe .s2meta sidecar writer.
    src/crash_handler.cpp
    ${HL2SDK}/entity2/entitykeyvalues.cpp
    ${HL2SDK}/tier1/keyvalues3.cpp
    ${HL2SDK}/public/tier0/memoverride.cpp   # CSSharp compiles this — routes operator new/delete
                                             # through the engine allocator (g_pMemAlloc) so our
                                             # compiled KV3 code + the engine share ONE heap. Do NOT
                                             # apply gc-sections to this TU (its global new/delete
                                             # operators must survive) — list it separately below.
)

# --- Crash reporter (Breakpad, vendored) ---------------------------------------------------
# The Linux client only: exception_handler + the in-process minidump writer. Compiled as a
# static lib with the SAME Valve ABI define as the shim; linked into s2script.so (gc-sections
# drops the unused microdump/core-dump paths). Verified to build under Steam Runtime 3
# (glibc 2.31) in the rust:bullseye sniper container.
enable_language(ASM)
# Breakpad's guid_creator uses pthread_once. On the Steam-Runtime-3 sniper toolchain (glibc 2.31)
# pthread is a separate library (host glibc >= 2.34 folds it into libc), so link it explicitly and
# let it propagate (PUBLIC) to both s2script.so and the crash_selftest executable.
find_package(Threads REQUIRED)
set(BREAKPAD ${CMAKE_SOURCE_DIR}/../third_party/breakpad/src)
add_library(breakpad_client STATIC
    ${BREAKPAD}/client/linux/crash_generation/crash_generation_client.cc
    ${BREAKPAD}/client/linux/dump_writer_common/thread_info.cc
    ${BREAKPAD}/client/linux/dump_writer_common/ucontext_reader.cc
    ${BREAKPAD}/client/linux/handler/exception_handler.cc
    ${BREAKPAD}/client/linux/handler/minidump_descriptor.cc
    ${BREAKPAD}/client/linux/log/log.cc
    ${BREAKPAD}/client/linux/microdump_writer/microdump_writer.cc
    ${BREAKPAD}/client/linux/minidump_writer/linux_dumper.cc
    ${BREAKPAD}/client/linux/minidump_writer/linux_ptrace_dumper.cc
    ${BREAKPAD}/client/linux/minidump_writer/minidump_writer.cc
    ${BREAKPAD}/client/linux/minidump_writer/pe_file.cc
    ${BREAKPAD}/client/minidump_file_writer.cc
    ${BREAKPAD}/common/convert_UTF.cc
    ${BREAKPAD}/common/md5.cc
    ${BREAKPAD}/common/string_conversion.cc
    ${BREAKPAD}/common/linux/breakpad_getcontext.S
    ${BREAKPAD}/common/linux/elf_core_dump.cc
    ${BREAKPAD}/common/linux/elfutils.cc
    ${BREAKPAD}/common/linux/file_id.cc
    ${BREAKPAD}/common/linux/guid_creator.cc
    ${BREAKPAD}/common/linux/linux_libc_support.cc
    ${BREAKPAD}/common/linux/memory_mapped_file.cc
    ${BREAKPAD}/common/linux/safe_readlink.cc
)
target_include_directories(breakpad_client PUBLIC ${BREAKPAD})
target_link_libraries(breakpad_client PUBLIC Threads::Threads)
set_target_properties(breakpad_client PROPERTIES POSITION_INDEPENDENT_CODE ON)
target_compile_definitions(breakpad_client PRIVATE _GLIBCXX_USE_CXX11_ABI=0)
# Breakpad's memory_allocator.h uses std::max_align_t but only includes <stddef.h> (which declares
# ::max_align_t, not the std:: one). Newer host libstdc++ pulls std::max_align_t in transitively, but
# the Steam-Runtime-3 sniper toolchain (bullseye gcc 10) does not — force-include <cstddef> so the
# vendored source compiles unpatched on both. (The submodule stays pristine.)
target_compile_options(breakpad_client PRIVATE
    -ffunction-sections -fdata-sections
    # C++ only — the .S assembly TU (breakpad_getcontext.S) must not force-include a C++ header.
    "$<$<COMPILE_LANGUAGE:CXX>:SHELL:-include cstddef>")

# The standalone fork-and-crash selftest (not shipped; run manually + in the sniper container).
add_executable(crash_selftest src/crash_selftest.cpp src/crash_handler.cpp)
target_include_directories(crash_selftest PRIVATE src)
target_link_libraries(crash_selftest PRIVATE breakpad_client)
target_compile_definitions(crash_selftest PRIVATE _GLIBCXX_USE_CXX11_ABI=0)

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
    # Slice 6.6: the vendored HDE length disassembler (used by src/detour.cpp).
    ${CMAKE_SOURCE_DIR}/../third_party/hde
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

# Crash reporter: link the vendored Breakpad client (its PUBLIC include dir reaches
# src/crash_handler.cpp during compilation).
target_link_libraries(s2script PRIVATE breakpad_client)

# S2_HL2SDK_BUILD: the pinned hl2sdk submodule SHA, stamped into every crash envelope's
# gamedata.hl2sdk. Resolved at configure time from the vendored checkout (${HL2SDK} is
# defined at the top of shim/CMakeLists.txt).
execute_process(
    COMMAND git -C ${HL2SDK} rev-parse --short HEAD
    OUTPUT_VARIABLE S2_HL2SDK_SHA OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
if(NOT S2_HL2SDK_SHA)
    set(S2_HL2SDK_SHA "unknown")
endif()
target_compile_definitions(s2script PRIVATE S2_HL2SDK_BUILD="${S2_HL2SDK_SHA}")

# EKV slice: split the SDK TUs into per-function sections and GC unreachable ones at link time,
# so never-called KV3 paths (ToString/CBufferString/CUtlBuffer serialization, V_StringTo* array
# conversion, GetEHandle->FindFirstEntityHandleByName) drop their undefined symbol references.
set_source_files_properties(
    src/ekv.cpp
    ${HL2SDK}/entity2/entitykeyvalues.cpp
    ${HL2SDK}/tier1/keyvalues3.cpp
    # NOTE: memoverride.cpp is deliberately EXCLUDED here — its global operator new/delete must
    # NOT be gc-sectioned away (they're the whole point). It compiles WITHOUT these flags.
    PROPERTIES COMPILE_OPTIONS "-ffunction-sections;-fdata-sections"
)
target_link_options(s2script PRIVATE -Wl,--gc-sections)

# EKV slice: memoverride.cpp needs MEMOVERRIDE_MODULE defined (it stringifies the value into
# g_pszModule via MKSTRING) — only relevant for the debug-allocator-attribution build (unused
# here), but the macro must still expand to SOMETHING for the TU to compile.
set_source_files_properties(${HL2SDK}/public/tier0/memoverride.cpp PROPERTIES
    COMPILE_DEFINITIONS "MEMOVERRIDE_MODULE=s2script"
)

# Ensure the runtime can find libs2script_core.so next to the plugin.
# Output as s2script.so (no lib prefix) to match Metamod's loader expectations.
set_target_properties(s2script PROPERTIES
    PREFIX ""
    OUTPUT_NAME "s2script"
    BUILD_RPATH "$ORIGIN"
    INSTALL_RPATH "$ORIGIN"
    BUILD_WITH_INSTALL_RPATH TRUE
)
