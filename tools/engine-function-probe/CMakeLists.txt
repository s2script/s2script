cmake_minimum_required(VERSION 3.20)
project(engine_function_probe C CXX)
if(NOT CMAKE_SYSTEM_NAME STREQUAL "Linux" OR NOT CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64|AMD64)$")
    message(FATAL_ERROR "Stock provider proof requires linux-x86_64-sysv")
endif()
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
get_filename_component(ROOT "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
set(KHOOK "${ROOT}/third_party/metamod-source/third_party/khook")
set(SAFETYHOOK "${KHOOK}/third_party/safetyhook")
include("${ROOT}/shim/cmake/Libffi.cmake")
option(S2FN_LIVE_PLUGIN "Build the resident Metamod consumer instead of the standalone test provider" OFF)
if(S2FN_LIVE_PLUGIN)
    include(live.cmake)
    return()
endif()
# Exact upstream AMBuilder source inventory, including its checked-in Zydis.
# EXCLUDE_FROM_ALL and KHOOK_STANDALONE apply only inside this test project.
add_library(engine_function_stock_provider STATIC EXCLUDE_FROM_ALL
    "${KHOOK}/src/detour.cpp" "${KHOOK}/src/ranges.cpp"
    "${SAFETYHOOK}/src/allocator.cpp" "${SAFETYHOOK}/src/easy.cpp"
    "${SAFETYHOOK}/src/inline_hook.cpp" "${SAFETYHOOK}/src/mid_hook.cpp"
    "${SAFETYHOOK}/src/utility.cpp" "${SAFETYHOOK}/src/vmt_hook.cpp"
    "${SAFETYHOOK}/src/os.linux.cpp" "${SAFETYHOOK}/zydis/Zydis.c")
target_compile_definitions(engine_function_stock_provider PUBLIC KHOOK_STANDALONE PRIVATE KHOOK_EXPORT)
target_include_directories(engine_function_stock_provider PUBLIC "${KHOOK}/include"
    PRIVATE "${SAFETYHOOK}/include" "${SAFETYHOOK}/zydis")
target_link_libraries(engine_function_stock_provider PUBLIC pthread dl)
add_library(engine_function_adapter STATIC EXCLUDE_FROM_ALL "${ROOT}/shim/src/engine_function_abi.cpp")
target_include_directories(engine_function_adapter PUBLIC "${ROOT}/shim/src")
target_link_libraries(engine_function_adapter PUBLIC engine_function_stock_provider PRIVATE s2_libffi)
# A real member target must not share executable pages with provider/std::function
# helpers: stock SafetyHook removes execute permission while patching its page.
# Hide every symbol except the address accessor; never link a provider here.
add_library(engine_function_member_fixture SHARED EXCLUDE_FROM_ALL
    "${ROOT}/shim/tests/engine_function_member_fixture.cpp")
target_include_directories(engine_function_member_fixture PRIVATE "${KHOOK}/include")
target_compile_options(engine_function_member_fixture PRIVATE -fvisibility=hidden -fno-gnu-unique)
# Static adapter headers contain ffi_cif, so each direct consumer also needs the
# private dependency's headers; it is never a runtime DT_NEEDED dependency.
foreach(target engine_function_abi_test engine_function_v8_bridge)
    if(target STREQUAL "engine_function_abi_test")
        add_executable(${target} EXCLUDE_FROM_ALL "${ROOT}/shim/tests/engine_function_abi_test.cpp" plugin.cpp)
    else()
        add_library(${target} SHARED EXCLUDE_FROM_ALL "${ROOT}/shim/tests/engine_function_abi_test.cpp")
        target_compile_definitions(${target} PRIVATE S2FN_NO_MAIN)
    endif()
    target_link_libraries(${target} PRIVATE engine_function_adapter s2_libffi engine_function_member_fixture ${CMAKE_DL_LIBS})
    target_link_options(${target} PRIVATE "-Wl,--exclude-libs,ALL" "-Wl,--wrap=ffi_closure_alloc" "-Wl,--wrap=ffi_closure_free")
    target_compile_options(${target} PRIVATE -UNDEBUG -Wall -Wextra)
endforeach()
