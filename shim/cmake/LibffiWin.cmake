# The pinned libffi (third_party/libffi, v3.7.1) built for Windows x64 without autotools: the C
# sources compile with clang-cl, win64.S (GNU syntax) assembles with the clang driver, and the two
# configure outputs are replaced by libffi-win64/fficonfig.h (hand-written) and an ffi.h generated
# here from include/ffi.h.in with the X86_WIN64 values configure would substitute.
get_filename_component(S2_LIBFFI_SOURCE "${CMAKE_CURRENT_LIST_DIR}/../../third_party/libffi" ABSOLUTE)
execute_process(COMMAND git -C "${S2_LIBFFI_SOURCE}" rev-parse HEAD
    OUTPUT_VARIABLE S2_LIBFFI_REV OUTPUT_STRIP_TRAILING_WHITESPACE COMMAND_ERROR_IS_FATAL ANY)
if(NOT S2_LIBFFI_REV STREQUAL "5c1c43091ed611fdea774374355eb938c73a9157")
    message(FATAL_ERROR "libffi source is not pinned v3.7.1")
endif()

set(S2_LIBFFI_GEN "${CMAKE_CURRENT_BINARY_DIR}/libffi-win64/include")
file(MAKE_DIRECTORY "${S2_LIBFFI_GEN}")
set(VERSION "3.7.1")
set(TARGET "X86_WIN64")
set(HAVE_LONG_DOUBLE "0")              # MSVC long double is double
set(HAVE_LONG_DOUBLE_VARIANT "0")
set(FFI_EXEC_TRAMPOLINE_TABLE "0")
set(FFI_VERSION_STRING "3.7.1")
set(FFI_VERSION_NUMBER "30701")
configure_file("${S2_LIBFFI_SOURCE}/include/ffi.h.in" "${S2_LIBFFI_GEN}/ffi.h" @ONLY)
file(READ "${S2_LIBFFI_GEN}/ffi.h" S2_FFI_H)
if(S2_FFI_H MATCHES "@[A-Z_]+@")
    message(FATAL_ERROR "ffi.h.in gained a substitution LibffiWin.cmake does not set: ${CMAKE_MATCH_0}")
endif()
configure_file("${S2_LIBFFI_SOURCE}/src/x86/ffitarget.h" "${S2_LIBFFI_GEN}/ffitarget.h" COPYONLY)

set(S2_LIBFFI_INCLUDES
    "${CMAKE_CURRENT_LIST_DIR}/libffi-win64"
    "${S2_LIBFFI_GEN}"
    "${S2_LIBFFI_SOURCE}/include"
    "${S2_LIBFFI_SOURCE}/src/x86")

# win64.S through the clang driver (clang-cl does not take GNU assembly).
find_program(S2_CLANG NAMES clang-19 clang-18 clang REQUIRED)
set(S2_LIBFFI_ASM_OBJ "${CMAKE_CURRENT_BINARY_DIR}/libffi-win64/win64.obj")
set(S2_LIBFFI_ASM_INCLUDES)
foreach(dir IN LISTS S2_LIBFFI_INCLUDES)
    list(APPEND S2_LIBFFI_ASM_INCLUDES "-I${dir}")
endforeach()
add_custom_command(
    OUTPUT "${S2_LIBFFI_ASM_OBJ}"
    COMMAND "${S2_CLANG}" --target=x86_64-pc-windows-msvc -c -DFFI_STATIC_BUILD
            ${S2_LIBFFI_ASM_INCLUDES} "${S2_LIBFFI_SOURCE}/src/x86/win64.S" -o "${S2_LIBFFI_ASM_OBJ}"
    DEPENDS "${S2_LIBFFI_SOURCE}/src/x86/win64.S" "${S2_LIBFFI_GEN}/ffi.h"
            "${CMAKE_CURRENT_LIST_DIR}/libffi-win64/fficonfig.h"
    VERBATIM)

add_library(s2_libffi STATIC
    "${S2_LIBFFI_SOURCE}/src/prep_cif.c"
    "${S2_LIBFFI_SOURCE}/src/types.c"
    "${S2_LIBFFI_SOURCE}/src/raw_api.c"
    "${S2_LIBFFI_SOURCE}/src/java_raw_api.c"
    "${S2_LIBFFI_SOURCE}/src/closures.c"
    "${S2_LIBFFI_SOURCE}/src/tramp.c"
    "${S2_LIBFFI_SOURCE}/src/x86/ffiw64.c"
    "${S2_LIBFFI_ASM_OBJ}")
target_include_directories(s2_libffi PUBLIC ${S2_LIBFFI_INCLUDES})
target_compile_definitions(s2_libffi PUBLIC FFI_STATIC_BUILD)
target_compile_options(s2_libffi PRIVATE -Wno-everything)
