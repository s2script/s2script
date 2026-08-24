cmake_minimum_required(VERSION 3.20)
project(s2script_shim C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
if(MSVC)
    # Match upstream Metamod's release ABI (/MT; /MTd for an explicit Debug configuration) and
    # apply it to the shim, vendored SDK TUs, and every native selftest.
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()

# SDK paths (relative to shim/ source dir, so go up one level to repo root)
set(HL2SDK ${CMAKE_SOURCE_DIR}/../third_party/hl2sdk)
set(MMS    ${CMAKE_SOURCE_DIR}/../third_party/metamod-source)
set(AMTL   ${MMS}/third_party/amtl)

# Cargo profile directory the resident core library was built into. `make shim` and
# scripts/build-sniper.sh always build core as --release, so this defaults to "release" and is
# unaffected. Linux CI's cheap PR build compiles core in debug and selects that profile explicitly;
# Windows CI builds and links Release artifacts to exercise the deployable MSVC configuration.
set(S2_CORE_LIB_DIR "release" CACHE STRING "cargo profile dir holding the core DLL/shared library")

set(S2_PLATFORM_SOURCES
    src/platform/paths_common.cpp
    src/platform/pe_image.cpp)
if(WIN32)
    enable_language(ASM_MASM)
    set(S2_CALL_ABI_SOURCES
        src/call_abi_win.cpp
        src/call_abi_win_msvc.asm)
    set(S2_HOOK_ABI_SOURCE src/hook_abi_win.cpp)
    set(S2_PLATFORM_MEMORY_SOURCE src/platform/memory_win.cpp)
    set(S2_PLATFORM_BACKEND_SOURCES
        src/platform/module_win.cpp
        ${S2_PLATFORM_MEMORY_SOURCE}
        src/platform/paths_win.cpp)
    set(S2_PLATFORM_IMAGE_SOURCE src/platform/image_win.cpp)
    set(S2_CORE_SYMBOLS_SOURCE src/core_symbols_win.cpp)
    set(S2_CRASH_HANDLER_SOURCE src/crash_handler_win.cpp)
    set(S2_DETOUR_TEST_ASM_SOURCE tests/detour_test_target_win_msvc.asm)
else()
    set(S2_CALL_ABI_SOURCES src/call_abi_sysv.cpp)
    set(S2_HOOK_ABI_SOURCE src/hook_abi_sysv.cpp)
    set(S2_PLATFORM_MEMORY_SOURCE src/platform/memory_linux.cpp)
    set(S2_PLATFORM_BACKEND_SOURCES
        src/platform/module_linux.cpp
        ${S2_PLATFORM_MEMORY_SOURCE}
        src/platform/paths_linux.cpp)
    set(S2_PLATFORM_IMAGE_SOURCE src/platform/image_linux.cpp)
    set(S2_CORE_SYMBOLS_SOURCE src/core_symbols_linux.cpp)
    set(S2_CRASH_HANDLER_SOURCE src/crash_handler.cpp)
endif()
list(APPEND S2_PLATFORM_SOURCES
    ${S2_PLATFORM_BACKEND_SOURCES}
    ${S2_PLATFORM_IMAGE_SOURCE}
    ${S2_CORE_SYMBOLS_SOURCE})

# Build the exact protobuf revision whose headers the shim compiles against. The historical
# HL2SDK win64/2015 archive was produced by a different MSVC toolset/CRT and is not ABI-safe to
# mix into this binary.
if(WIN32)
    set(protobuf_BUILD_TESTS OFF CACHE BOOL "" FORCE)
    set(protobuf_BUILD_CONFORMANCE OFF CACHE BOOL "" FORCE)
    set(protobuf_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
    set(protobuf_BUILD_PROTOC_BINARIES OFF CACHE BOOL "" FORCE)
    set(protobuf_BUILD_LIBPROTOC OFF CACHE BOOL "" FORCE)
    set(protobuf_BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
    set(protobuf_INSTALL OFF CACHE BOOL "" FORCE)
    set(protobuf_WITH_ZLIB OFF CACHE BOOL "" FORCE)
    set(protobuf_MSVC_STATIC_RUNTIME ON CACHE BOOL "" FORCE)
    add_subdirectory(
        ${HL2SDK}/thirdparty/protobuf-3.21.8
        ${CMAKE_BINARY_DIR}/protobuf-3.21.8
        EXCLUDE_FROM_ALL)
endif()

add_library(s2script SHARED
    src/s2script_mm.cpp
    ${S2_PLATFORM_SOURCES}
    # Entity lifecycle listeners slice: the isolated IEntityListener TU (mirrors ekv.cpp's isolation
    # discipline — the only new TU that includes entity2/entitysystem.h for the listener interface).
    src/entity_listener.cpp
    src/gamedata.cpp
    src/sigscan.cpp
    # Slice 5D.1: a self-contained MurmurHash2LowerCase (the ONE tier1 symbol CKV3MemberName(const char*)
    # needs to key IGameEvent field reads) — avoids linking all of tier1 (CUtlString/UtlVectorMemory cascade).
    src/tier1_shims.cpp
    # CCommand's ctor/Tokenize/Reset/EnsureBuffers are DECLARED in tier1/convar.h but exported by no
    # shipped CS2 binary — using them links and then fails at dlopen, taking the whole addon down.
    # Same doctrine as tier1_shims.cpp: define the few members we need rather than link all of tier1.
    src/ccommand_shim.cpp
    src/ccommand_tokenize.cpp
    # Slice 6.6: a minimal self-contained x86-64 inline detour (the damage hook) + the vendored HDE
    # length disassembler (pure, no OS deps). No third-party detour library / no dlopen cascade.
    src/detour.cpp
    # ...and its engine-free half: prologue decode, relocation, and the named refusals. Split out so
    # shim/tests/detour_reloc_test.cpp drives the SHIPPED relocator against byte buffers at pretend
    # addresses — a relocator proven only by "the server did not crash" is not proven.
    src/detour_reloc.cpp
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
    # Crash reporter: vendored in-process Breakpad on both native platforms.
    ${S2_CRASH_HANDLER_SOURCE}
    # Plugin-gamedata slice: plugin-declared engine calls — descriptor resolution (byte signature /
    # RTTI vtable slot + prologue validation + .text range) and the fixed-max-arity SysV invoke.
    src/engine_calls.cpp
    ${S2_CALL_ABI_SOURCES}
    # A5b: the CLOSED validator vocabulary (prologue / string-xref / vtable-member). Its own TU
    # because engine_calls.cpp cannot be compiled outside the game (it includes the entity system);
    # every outside contact is injected, so shim/tests/call_validate_test.cpp drives the SHIPPED
    # gates over a synthetic module image.
    src/call_validate.cpp
    # deferred-dispatch slice: the queue/drain POLICY, engine-free (every engine + core call is
    # injected through S2DeferOps), so shim/tests/defer_queue_test.cpp drives the SHIPPED drain.
    src/defer_queue.cpp
    # declarative inbound hooks: the engine-free POLICY half (shape vocabulary, bypass latch,
    # collapse contract) — also built standalone into hook_dispatch_selftest below, so the gate
    # drives exactly the code that ships...
    src/hook_dispatch.cpp
    # ...and the ENGINE half: one compile-time thunk per (shape, hook slot), lazy detour install,
    # and the block-scoped arg view. Cannot be unit-tested outside the game (it marshals a receiver
    # through the entity system), which is precisely why the policy lives in the TU above.
    src/engine_hooks.cpp
    ${S2_HOOK_ABI_SOURCE}
    ${HL2SDK}/entity2/entitykeyvalues.cpp
    ${HL2SDK}/tier1/keyvalues3.cpp
    ${HL2SDK}/public/tier0/memoverride.cpp   # CSSharp compiles this — routes operator new/delete
                                             # through the engine allocator (g_pMemAlloc) so our
                                             # compiled KV3 code + the engine share ONE heap. Do NOT
                                             # apply gc-sections to this TU (its global new/delete
                                             # operators must survive) — list it separately below.
)

set(BREAKPAD ${CMAKE_SOURCE_DIR}/../third_party/breakpad/src)
if(WIN32)
    # --- Crash reporter (Breakpad Windows client, vendored) -------------------------------
    # Exact in-process ExceptionHandler closure. DbgHelp and Rpcrt4 are intentionally loaded
    # dynamically by Breakpad itself; all remaining Win32 APIs come from the standard system
    # libraries. CMAKE_MSVC_RUNTIME_LIBRARY above gives these TUs the same /MT ABI as the shim.
    add_library(breakpad_client STATIC
        ${BREAKPAD}/client/windows/handler/exception_handler.cc
        ${BREAKPAD}/client/windows/crash_generation/crash_generation_client.cc
        ${BREAKPAD}/common/windows/guid_string.cc
    )
    target_include_directories(breakpad_client PUBLIC ${BREAKPAD})
    target_compile_definitions(breakpad_client PRIVATE UNICODE _UNICODE)
else()
    # --- Crash reporter (Breakpad Linux client, vendored) ---------------------------------
    enable_language(ASM)
    find_package(Threads REQUIRED)
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
    target_compile_options(breakpad_client PRIVATE
        -ffunction-sections -fdata-sections
        "$<$<COMPILE_LANGUAGE:CXX>:SHELL:-include cstddef>")
endif()

# Tokenizer selftest for our CCommand::Tokenize (not shipped; run by scripts/ci-native.sh).
add_executable(ccommand_selftest src/ccommand_selftest.cpp src/ccommand_tokenize.cpp)
target_include_directories(ccommand_selftest PRIVATE src)

# Owner-scoped gamedata loader unit test (not shipped; run by scripts/ci-native.sh).
add_executable(gamedata_selftest src/gamedata.cpp tests/gamedata_test.cpp)
target_include_directories(gamedata_selftest PRIVATE src third_party)

# Deferred-dispatch queue unit test (not shipped; run by scripts/ci-native.sh via
# scripts/test-defer-queue.sh, which adds _GLIBCXX_DEBUG + the sanitizers). This target exists so
# the test also builds under the ordinary cmake configuration.
add_executable(defer_queue_selftest src/defer_queue.cpp tests/defer_queue_test.cpp)
target_include_directories(defer_queue_selftest PRIVATE src include)

# Descriptor-validator unit test (not shipped; run by scripts/ci-native.sh via
# scripts/test-call-validate.sh, which adds the sanitizers). This target exists so the test also
# builds under the ordinary cmake configuration.
add_executable(call_validate_selftest
    src/call_validate.cpp src/sigscan.cpp
    ${CMAKE_SOURCE_DIR}/../third_party/hde/hde64.c
    tests/call_validate_test.cpp)
target_include_directories(call_validate_selftest
    PRIVATE src third_party ${CMAKE_SOURCE_DIR}/../third_party/hde)

# Engine-free half of declarative inbound hooks (not shipped; run by scripts/ci-native.sh).
add_executable(hook_dispatch_selftest src/hook_dispatch.cpp tests/hook_dispatch_test.cpp)
target_include_directories(hook_dispatch_selftest PRIVATE src)

# Native ABI selftest. On Windows this compiles and runs the production MASM outbound bridge and
# native Microsoft hook thunks; on Linux it covers the selected SysV backends. Keep the Windows
# C++ caller/callees optimized so the gate exercises release-like register/stack code generation.
add_executable(abi_selftest
    tests/abi_test.cpp
    ${S2_CALL_ABI_SOURCES}
    ${S2_HOOK_ABI_SOURCE}
    src/hook_dispatch.cpp)
target_include_directories(abi_selftest PRIVATE src)
if(WIN32)
    target_compile_options(abi_selftest PRIVATE $<$<COMPILE_LANGUAGE:CXX>:/O2>)
endif()
enable_testing()
add_test(NAME abi_selftest COMMAND abi_selftest)

# Detour prologue relocation (not shipped; run by scripts/ci-native.sh via
# scripts/test-detour-reloc.sh, which adds the sanitizers). This target exists so the test also
# builds under the ordinary cmake configuration.
add_executable(detour_reloc_selftest
    src/detour_reloc.cpp src/detour.cpp ${S2_PLATFORM_MEMORY_SOURCE}
    ${CMAKE_SOURCE_DIR}/../third_party/hde/hde64.c tests/detour_reloc_test.cpp
    ${S2_DETOUR_TEST_ASM_SOURCE})
target_include_directories(detour_reloc_selftest
    PRIVATE src ${CMAKE_SOURCE_DIR}/../third_party/hde)

# OS-neutral platform contract + the real Linux backends. The synthetic PE image makes PE range
# parsing and MSVC RTTI lookup testable in the Linux gate.
add_executable(platform_selftest
    tests/platform_test.cpp
    src/platform/paths_common.cpp
    src/platform/pe_image.cpp
    ${S2_PLATFORM_BACKEND_SOURCES})
target_include_directories(platform_selftest PRIVATE src)
target_link_libraries(platform_selftest PRIVATE ${CMAKE_DL_LIBS})
if(WIN32)
    target_link_libraries(platform_selftest PRIVATE Psapi)
endif()


if(WIN32)
    # Launches a child process that faults after arming the production Windows handler.
    add_executable(crash_selftest src/crash_selftest_win.cpp src/crash_handler_win.cpp)
    target_include_directories(crash_selftest PRIVATE src)
    target_link_libraries(crash_selftest PRIVATE breakpad_client)
else()
    # The standalone fork-and-crash selftest (not shipped; Linux uses fork/signals).
    add_executable(crash_selftest src/crash_selftest.cpp src/crash_handler.cpp)
    target_include_directories(crash_selftest PRIVATE src)
    target_link_libraries(crash_selftest PRIVATE breakpad_client)
    target_compile_definitions(crash_selftest PRIVATE _GLIBCXX_USE_CXX11_ABI=0)
endif()

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
    # model precache: igamesystem.h / igamesystemfactory.h live under game/shared, not public/
    ${HL2SDK}/game/shared
    # protobuf 3.21.8 runtime headers (Slice 6.1c) — reflection over the game's usermessage
    # protobuf objects (CUserMessageSayText2). Matches the vendored libprotobuf.a below.
    ${HL2SDK}/thirdparty/protobuf-3.21.8/src
    # Slice 6.6: the vendored HDE length disassembler (used by src/detour.cpp).
    ${CMAKE_SOURCE_DIR}/../third_party/hde
)

# Source 2 / CS2 build defines. Keep these aligned with the pinned Metamod
# hl2sdk-manifests/SdkHelpers.ambuild + manifests/cs2.json.
target_compile_definitions(s2script PRIVATE
    SOURCE_ENGINE=25
    GAME_DLL
    RAD_TELEMETRY_DISABLED
    PLATFORM_64BITS
    X64BITS
    META_IS_SOURCE2
    META_NO_HL2SDK
)
if(WIN32)
    target_compile_definitions(s2script PRIVATE
        WIN32
        WIN64
        _WINDOWS
        COMPILER_MSVC
        COMPILER_MSVC64
        _CRT_SECURE_NO_DEPRECATE
        _CRT_SECURE_NO_WARNINGS
        _CRT_NONSTDC_NO_DEPRECATE)
    target_compile_options(s2script PRIVATE /W3 /Zi)
    target_link_options(s2script PRIVATE /SUBSYSTEM:WINDOWS)
else()
    target_compile_definitions(s2script PRIVATE
        LINUX
        _LINUX
        POSIX
        GNUC
        COMPILER_GCC
        _FILE_OFFSET_BITS=64
        _GLIBCXX_USE_CXX11_ABI=0
        stricmp=strcasecmp
        strnicmp=strncasecmp
        _stricmp=strcasecmp
        _vsnprintf=vsnprintf)
    target_compile_options(s2script PRIVATE -m64 -fvisibility=hidden)
endif()

# Link the resident Rust core (built by `cargo build --release`).
# libprotobuf.a (Slice 6.1c): the vendored, self-contained protobuf 3.21.8 static lib — provides the
# reflection symbols (Descriptor::FindFieldByName / Reflection::Set*) for client_print's SayText2 send.
# Self-contained (no undefined deps), so it does NOT reintroduce the tier1 dlopen cascade (5D.1). Built
# with Valve's ABI (_GLIBCXX_USE_CXX11_ABI=0, set above), matching the game's in-process runtime.
if(WIN32)
    set(S2_CORE_DLL
        "${CMAKE_SOURCE_DIR}/../target/${S2_CORE_LIB_DIR}/s2script_core.dll")
    set(S2_CORE_IMPORT_LIB
        "${CMAKE_SOURCE_DIR}/../target/${S2_CORE_LIB_DIR}/s2script_core.dll.lib")
    # Cargo has historically left MSVC's side-effect import library under deps/ even while copying
    # the DLL to the profile root. Accept both layouts, but fail at configure time with a named
    # diagnostic rather than surfacing an opaque LINK1104 later.
    if(NOT EXISTS "${S2_CORE_IMPORT_LIB}")
        set(S2_CORE_IMPORT_LIB
            "${CMAKE_SOURCE_DIR}/../target/${S2_CORE_LIB_DIR}/deps/s2script_core.dll.lib")
    endif()
    if(NOT EXISTS "${S2_CORE_DLL}" OR NOT EXISTS "${S2_CORE_IMPORT_LIB}")
        message(FATAL_ERROR
            "Rust core artifacts are missing; run cargo build for profile '${S2_CORE_LIB_DIR}' first "
            "(need s2script_core.dll and s2script_core.dll.lib)")
    endif()
    target_link_libraries(s2script PRIVATE
        ${S2_CORE_IMPORT_LIB}
        protobuf::libprotobuf
        ${HL2SDK}/lib/public/win64/tier0.lib
        legacy_stdio_definitions
        kernel32
        Psapi)
    add_custom_command(TARGET s2script POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
                "${S2_CORE_DLL}" "$<TARGET_FILE_DIR:s2script>/s2script_core.dll"
        COMMENT "Staging the resident Rust core DLL beside s2script.dll")
else()
    target_link_libraries(s2script PRIVATE
        ${CMAKE_SOURCE_DIR}/../target/${S2_CORE_LIB_DIR}/libs2script_core.so
        ${HL2SDK}/lib/linux64/release/libprotobuf.a)
endif()
target_include_directories(s2script PRIVATE include)

# Crash reporter: link the platform's vendored Breakpad client (its PUBLIC include dir reaches
# the selected crash-handler TU during compilation).
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
if(NOT WIN32)
    set_source_files_properties(
        src/ekv.cpp
        ${HL2SDK}/entity2/entitykeyvalues.cpp
        ${HL2SDK}/tier1/keyvalues3.cpp
        # NOTE: memoverride.cpp is deliberately EXCLUDED here — its global operator new/delete must
        # NOT be gc-sectioned away (they're the whole point). It compiles WITHOUT these flags.
        PROPERTIES COMPILE_OPTIONS "-ffunction-sections;-fdata-sections"
    )
    target_link_options(s2script PRIVATE -Wl,--gc-sections)
endif()

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
)
if(NOT WIN32)
    set_target_properties(s2script PROPERTIES
        BUILD_RPATH "$ORIGIN"
        INSTALL_RPATH "$ORIGIN"
        BUILD_WITH_INSTALL_RPATH TRUE)
endif()

# CMake/CTest is the portable native-test entry point used by the Windows runner. The Linux gate
# still runs sanitizer-specialized shell wrappers in addition to these ordinary builds.
add_test(NAME ccommand_selftest COMMAND ccommand_selftest)
add_test(NAME gamedata_selftest COMMAND gamedata_selftest)
add_test(NAME defer_queue_selftest COMMAND defer_queue_selftest)
add_test(NAME call_validate_selftest COMMAND call_validate_selftest)
add_test(NAME hook_dispatch_selftest COMMAND hook_dispatch_selftest)
add_test(NAME detour_reloc_selftest COMMAND detour_reloc_selftest)
add_test(NAME platform_selftest COMMAND platform_selftest)
if(WIN32)
    add_test(NAME crash_selftest COMMAND crash_selftest)
endif()
