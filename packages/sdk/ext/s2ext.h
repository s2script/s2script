/* s2script native extension ABI, version 1.
 *
 * A native extension is a shared library in addons/s2script/extensions/ that adds natives to
 * s2script without a core change, the way SourceMod extensions do. It never sees V8 or Rust:
 * it registers plain C functions with a declared signature, and the host binds each one as a
 * JavaScript function. A plugin that declares the extension in package.json imports it by name:
 *
 *     "s2script": { "extensions": { "@community/fastmath": "^0.1.0" } }
 *     import { clamp } from "@community/fastmath";
 *
 * The rules:
 *
 * - Export two functions, s2ext_info and s2ext_init. The host calls s2ext_info first and refuses
 *   the extension (with a named reason, while the server keeps running) if its ABI version is
 *   not one the host speaks. Then it calls s2ext_init once, on the game thread, where the
 *   extension registers its natives.
 * - Every native's C function takes an opaque receiver first, then its declared arguments:
 *       int32_t clamp(void* recv, int32_t v, int32_t lo, int32_t hi);
 *   The receiver is meaningless to the extension; it is there because it is the shape V8's fast
 *   API calls, which lets the host point optimised JavaScript straight at your function.
 * - Natives run on the game thread. A native whose types are all numbers or booleans, and that
 *   does not set S2N_CAN_FAIL, is a fast native: optimised code calls it directly, so it must not
 *   block, must not call back into the host except log and schema_offset, and must not keep the
 *   receiver.
 * - Extensions load at server start and are never unloaded.
 * - Structs only grow by appending fields, and each carries its size, so an older or newer peer
 *   is detected without a version bump. A real break bumps S2EXT_ABI_VERSION.
 * - Native code runs with full access, the same trust as a Metamod plugin. Engine access goes
 *   through the host services below, never through engine tables directly.
 *
 * Supported platform for ABI 1: x86-64 Linux (the System V calling convention).
 */
#ifndef S2EXT_H
#define S2EXT_H

#include <stdint.h>
#ifndef __cplusplus
#include <stdbool.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define S2EXT_ABI_VERSION 1u

/* Argument and result types, and how each crosses.
 *
 *   type          C type            JavaScript         notes
 *   S2T_VOID      void              undefined          result only
 *   S2T_BOOL      bool              boolean            arguments convert as JS truthiness
 *   S2T_I32       int32_t           number             arguments convert as ToInt32
 *   S2T_U32       uint32_t          number             arguments convert as ToUint32
 *   S2T_F32       float             number
 *   S2T_F64       double            number
 *   S2T_STRING    const char*       string             UTF-8, NUL-terminated. An argument is
 *                                                      valid only during the call, and a string
 *                                                      with an embedded NUL is refused. A result
 *                                                      is copied at once and must stay valid until
 *                                                      your next native returns; NULL gives null.
 *   S2T_ENTITY    void*             EntityRef          argument only: the live entity's address,
 *                                                      valid only during the call. A stale ref
 *                                                      throws S2Error code "stale_entity" and
 *                                                      your function is not called.
 *
 * Limits per native: at most 5 arguments of the integer class (bool, i32, u32, string, entity)
 * and at most 8 of the float class (f32, f64), so every argument travels in a register.
 * A native with a string or entity anywhere, or with S2N_CAN_FAIL, is called through the host's
 * generic path rather than directly. That path costs roughly 100-150 ns per call, against
 * under 10 ns for a fast native.
 */
typedef enum S2Type {
  S2T_VOID = 0,
  S2T_BOOL = 1,
  S2T_I32 = 2,
  S2T_U32 = 3,
  S2T_F32 = 4,
  S2T_F64 = 5,
  S2T_STRING = 6,
  S2T_ENTITY = 7
} S2Type;

/* Native flags. */
enum {
  /* The native may call host->fail. If it does, its result is discarded and the call throws an
   * S2Error with the given code. */
  S2N_CAN_FAIL = 1u
};

/* Log levels for S2HostApi.log. */
enum { S2LOG_INFO = 0, S2LOG_WARN = 1, S2LOG_ERROR = 2 };

/* What the host hands an extension. Grows only by appending fields: check `size` before using a
 * field added after version 1. Every service takes the host pointer it was reached through. */
typedef struct S2HostApi {
  uint32_t size;
  uint32_t abi;
  /* The runtime's version, e.g. "0.4.0". */
  const char* host_version;
  /* One line in the server console, prefixed with the extension's name. */
  void (*log)(const struct S2HostApi* host, int32_t level, const char* message);
  /* A networked field's byte offset in its class (Source 2 schema), or -1 if either name is
   * unknown. Cached by the host; safe from a fast native. Game thread only. */
  int32_t (*schema_offset)(const struct S2HostApi* host, const char* class_name, const char* field);
  /* Inside a native registered with S2N_CAN_FAIL: make this call throw S2Error. `code` is a short
   * lowercase identifier ([a-z0-9_], up to 32 bytes), `message` is free text. Ignored, with a
   * warning, anywhere else. */
  void (*fail)(const struct S2HostApi* host, const char* code, const char* message);
} S2HostApi;

/* One native, as s2ext_init registers it. */
typedef struct S2NativeDesc {
  uint32_t size;               /* sizeof(S2NativeDesc) */
  const char* name;            /* a JavaScript identifier, unique in this extension */
  void* fn;                    /* ret fn(void* recv, args...) */
  S2Type ret;
  const S2Type* args;
  uint32_t nargs;
  uint32_t flags;              /* S2N_* */
  const char* doc;             /* optional: a one-paragraph description, for the generated typings */
  const char* const* arg_names;/* optional: nargs parameter names, for the generated typings */
} S2NativeDesc;

typedef struct S2Registrar S2Registrar;
typedef struct S2RegistrarApi {
  uint32_t size;
  /* Register one native. Returns 0, or nonzero if the descriptor is refused; the host has then
   * recorded why, and the extension is refused as a whole once s2ext_init returns. Valid only
   * during s2ext_init. */
  int32_t (*native)(S2Registrar* reg, const S2NativeDesc* desc);
} S2RegistrarApi;

typedef struct S2ExtInfo {
  uint32_t size;               /* sizeof(S2ExtInfo) */
  uint32_t abi;                /* S2EXT_ABI_VERSION */
  const char* name;            /* the npm-style name plugins import, e.g. "@community/fastmath" */
  const char* version;         /* semver, e.g. "0.1.0" */
} S2ExtInfo;

#if defined(_WIN32)
#define S2EXT_EXPORT __declspec(dllexport)
#else
#define S2EXT_EXPORT __attribute__((visibility("default")))
#endif

/* Exported by the extension. s2ext_info must return a pointer that stays valid for the process.
 * s2ext_init returns 0 on success; anything else refuses the extension. The host pointer stays
 * valid for the process, so keep it for later service calls. */
S2EXT_EXPORT const S2ExtInfo* s2ext_info(void);
S2EXT_EXPORT int32_t s2ext_init(const S2HostApi* host, S2Registrar* reg, const S2RegistrarApi* api);

#ifdef __cplusplus
}
#endif
#endif
