# Resident companion: imports IKHook only through PLUGIN_SAVEVARS. Never link the
# standalone provider target into this DSO.
if(NOT S2FN_BUILD_TOKEN MATCHES "^[0-9a-f]+$" OR NOT S2FN_SOURCE_REVISION MATCHES "^[0-9a-f]+$")
    message(FATAL_ERROR "Live probe requires source-bound S2FN_BUILD_TOKEN and S2FN_SOURCE_REVISION")
endif()
set(MMS "${ROOT}/third_party/metamod-source")
set(HL2SDK "${ROOT}/third_party/hl2sdk")
add_library(s2_engine_function_probe SHARED live_plugin.cpp "${ROOT}/shim/src/engine_function_abi.cpp"
    "${ROOT}/shim/src/gamedata.cpp" "${ROOT}/shim/src/engine_resolver.cpp" "${ROOT}/shim/src/original_module.cpp"
    "${ROOT}/shim/src/call_validate.cpp" "${ROOT}/shim/src/sigscan.cpp" "${ROOT}/shim/src/vtable.cpp" "${ROOT}/third_party/hde/hde64.c")
target_include_directories(s2_engine_function_probe PRIVATE "${ROOT}/shim/src/sdk_stubs" "${ROOT}/third_party/hde" "${ROOT}/shim/src" "${MMS}/core" "${KHOOK}/include"
    "${MMS}/third_party/amtl" "${HL2SDK}/public" "${HL2SDK}/public/tier0" "${HL2SDK}/public/tier1"
    "${HL2SDK}/public/appframework" "${HL2SDK}/public/mathlib" "${HL2SDK}/public/entity2" "${HL2SDK}/public/vscript" "${HL2SDK}/game/shared")
target_compile_definitions(s2_engine_function_probe PRIVATE LINUX _LINUX POSIX COMPILER_GCC PLATFORM_64BITS META_NO_HL2SDK
    _FILE_OFFSET_BITS=64 _GLIBCXX_USE_CXX11_ABI=0 stricmp=strcasecmp strnicmp=strncasecmp _stricmp=strcasecmp _vsnprintf=vsnprintf
    S2FN_SOURCE_REVISION="${S2FN_SOURCE_REVISION}" S2FN_BUILD_TOKEN="${S2FN_BUILD_TOKEN}")
target_compile_options(s2_engine_function_probe PRIVATE -m64 -fvisibility=hidden -fno-gnu-unique)
target_link_libraries(s2_engine_function_probe PRIVATE s2_libffi ${CMAKE_DL_LIBS})
target_link_options(s2_engine_function_probe PRIVATE -Wl,--exclude-libs,ALL -Wl,-Bsymbolic)
set_target_properties(s2_engine_function_probe PROPERTIES PREFIX "" BUILD_WITH_INSTALL_RPATH TRUE INSTALL_RPATH "$ORIGIN")
