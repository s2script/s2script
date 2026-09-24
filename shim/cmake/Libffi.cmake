# A private PIC archive built only from the pinned git submodule. No host lookup.
include(ExternalProject)
get_filename_component(S2_LIBFFI_SOURCE "${CMAKE_CURRENT_LIST_DIR}/../../third_party/libffi" ABSOLUTE)
execute_process(COMMAND git -C "${S2_LIBFFI_SOURCE}" rev-parse HEAD
    OUTPUT_VARIABLE S2_LIBFFI_REV OUTPUT_STRIP_TRAILING_WHITESPACE COMMAND_ERROR_IS_FATAL ANY)
if(NOT S2_LIBFFI_REV STREQUAL "5c1c43091ed611fdea774374355eb938c73a9157")
    message(FATAL_ERROR "libffi source is not pinned v3.7.1")
endif()
find_program(S2_AUTORECONF autoreconf REQUIRED)
find_program(S2_MAKE make REQUIRED)
set(S2_LIBFFI_PREFIX "${CMAKE_CURRENT_BINARY_DIR}/libffi-private")
file(MAKE_DIRECTORY "${S2_LIBFFI_PREFIX}/include")
# Autotools writes generated files: copy the source into the build tree to leave
# the pinned submodule byte-clean and permit concurrent separate build trees.
ExternalProject_Add(s2_libffi_build
    SOURCE_DIR "${CMAKE_CURRENT_BINARY_DIR}/libffi-source"
    DOWNLOAD_COMMAND "${CMAKE_COMMAND}" -E copy_directory "${S2_LIBFFI_SOURCE}" <SOURCE_DIR>
    UPDATE_COMMAND ""
    PATCH_COMMAND "${S2_AUTORECONF}" -v -i
    CONFIGURE_COMMAND "${CMAKE_COMMAND}" -E env "CC=${CMAKE_C_COMPILER}"
        <SOURCE_DIR>/configure --prefix=${S2_LIBFFI_PREFIX}
        --disable-shared --enable-static --with-pic --disable-multi-os-directory --disable-docs
    BUILD_COMMAND "${S2_MAKE}"
    INSTALL_COMMAND "${S2_MAKE}" install
    BUILD_BYPRODUCTS "${S2_LIBFFI_PREFIX}/lib/libffi.a")
add_library(s2_libffi STATIC IMPORTED GLOBAL)
set_target_properties(s2_libffi PROPERTIES
    IMPORTED_LOCATION "${S2_LIBFFI_PREFIX}/lib/libffi.a"
    INTERFACE_INCLUDE_DIRECTORIES "${S2_LIBFFI_PREFIX}/include")
add_dependencies(s2_libffi s2_libffi_build)
