/* fficonfig.h for the s2script Windows x64 build of the pinned libffi (shim/cmake/LibffiWin.cmake).
   Hand-written in place of configure's output, which needs a POSIX shell and the MSVC compiler.
   Only what libffi's x86-64 Windows sources (prep_cif, types, raw_api, java_raw_api, closures,
   tramp, x86/ffiw64.c, x86/win64.S) consult is defined. */
#define HAVE_ALLOCA 1
#define HAVE_INTTYPES_H 1
#define HAVE_MEMCPY 1
#define HAVE_STDINT_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_SYS_STAT_H 1
#define HAVE_SYS_TYPES_H 1
#define STDC_HEADERS 1
#define SIZEOF_DOUBLE 8
#define SIZEOF_LONG_DOUBLE 8
#define SIZEOF_SIZE_T 8
#define PACKAGE "libffi"
#define PACKAGE_NAME "libffi"
#define PACKAGE_VERSION "3.7.1"
#define VERSION "3.7.1"
#define EH_FRAME_FLAGS "a"

#ifdef LIBFFI_ASM
#define FFI_HIDDEN(name)
#else
#define FFI_HIDDEN
#endif
