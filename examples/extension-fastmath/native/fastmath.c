/* @community/fastmath: a worked native extension.
 *
 * Build:  cc -shared -fPIC -O2 -I../../../packages/sdk/ext -o fastmath.so fastmath.c
 * Install: copy fastmath.so into addons/s2script/extensions/ and restart the server.
 *
 * It shows each kind of native the ABI supports: fast natives over numbers and booleans, a
 * string in and out, a native that can fail, a host service, and an entity argument. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "s2ext.h"

static const S2HostApi* host;

/* ---- fast natives: numbers and booleans only, called directly from optimised code ---- */

static int32_t clamp(void* recv, int32_t v, int32_t lo, int32_t hi) {
  (void)recv;
  return v < lo ? lo : v > hi ? hi : v;
}

static double lerp(void* recv, double a, double b, double t) {
  (void)recv;
  return a + (b - a) * t;
}

static float lerpf(void* recv, float a, float b, float t) {
  (void)recv;
  return a + (b - a) * t;
}

static bool is_even(void* recv, uint32_t v) {
  (void)recv;
  return (v & 1u) == 0;
}

/* ---- generic-path natives ---- */

/* Strings: the result must stay valid until the next native returns, so a static buffer works. */
static char label_buf[256];
static const char* label(void* recv, int32_t n, double x, const char* unit) {
  (void)recv;
  snprintf(label_buf, sizeof label_buf, "%d x %.2f %s", n, x, unit);
  return label_buf;
}

/* Can fail: S2N_CAN_FAIL lets the native call host->fail, which makes the JS call throw. */
static double parse_number(void* recv, const char* text) {
  (void)recv;
  char* end = NULL;
  double v = strtod(text, &end);
  if (end == text || *end != '\0') {
    host->fail(host, "bad_number", "not a number");
    return 0;
  }
  return v;
}

/* A host service: schema offsets come from the host, never from the extension's own tables. */
static int32_t field_offset(void* recv, const char* class_name, const char* field) {
  (void)recv;
  return host->schema_offset(host, class_name, field);
}

/* An entity argument arrives as the live entity's address, valid only during this call. */
static int32_t read_entity_i32(void* recv, void* entity, int32_t offset) {
  (void)recv;
  int32_t v;
  memcpy(&v, (const char*)entity + offset, sizeof v);
  return v;
}

static const S2ExtInfo INFO = {sizeof(S2ExtInfo), S2EXT_ABI_VERSION, "@community/fastmath", "0.1.0"};

S2EXT_EXPORT const S2ExtInfo* s2ext_info(void) { return &INFO; }

#define REG(nm, f, r, a, fl, d, an)                                                     \
  do {                                                                                  \
    S2NativeDesc desc = {sizeof(S2NativeDesc), nm, (void*)(f), r, a,                    \
                         (uint32_t)(sizeof(a) / sizeof((a)[0])), fl, d, an};            \
    if (api->native(reg, &desc)) return 1;                                              \
  } while (0)

S2EXT_EXPORT int32_t s2ext_init(const S2HostApi* h, S2Registrar* reg, const S2RegistrarApi* api) {
  host = h;
  static const S2Type i32x3[] = {S2T_I32, S2T_I32, S2T_I32};
  static const S2Type f64x3[] = {S2T_F64, S2T_F64, S2T_F64};
  static const S2Type f32x3[] = {S2T_F32, S2T_F32, S2T_F32};
  static const S2Type u32x1[] = {S2T_U32};
  static const S2Type label_args[] = {S2T_I32, S2T_F64, S2T_STRING};
  static const S2Type str1[] = {S2T_STRING};
  static const S2Type str2[] = {S2T_STRING, S2T_STRING};
  static const S2Type ent_i32[] = {S2T_ENTITY, S2T_I32};
  static const char* const clamp_names[] = {"value", "min", "max"};
  static const char* const lerp_names[] = {"a", "b", "t"};
  static const char* const even_names[] = {"value"};
  static const char* const label_names[] = {"count", "size", "unit"};
  static const char* const parse_names[] = {"text"};
  static const char* const offset_names[] = {"className", "field"};
  static const char* const read_names[] = {"entity", "offset"};
  REG("clamp", clamp, S2T_I32, i32x3, 0, "Clamp an integer into [min, max].", clamp_names);
  REG("lerp", lerp, S2T_F64, f64x3, 0, "Linear interpolation between a and b.", lerp_names);
  REG("lerpf", lerpf, S2T_F32, f32x3, 0, "lerp in single precision.", lerp_names);
  REG("isEven", is_even, S2T_BOOL, u32x1, 0, NULL, even_names);
  REG("label", label, S2T_STRING, label_args, 0, "Format a count, a size and a unit.", label_names);
  REG("parseNumber", parse_number, S2T_F64, str1, S2N_CAN_FAIL,
      "Parse a whole string as a number. Throws S2Error code \"bad_number\" otherwise.", parse_names);
  REG("fieldOffset", field_offset, S2T_I32, str2, 0,
      "A schema field's byte offset in its class, or -1.", offset_names);
  REG("readEntityInt32", read_entity_i32, S2T_I32, ent_i32, 0,
      "Read an int32 at a byte offset in a live entity.", read_names);
  host->log(host, S2LOG_INFO, "registered 8 natives");
  return 0;
}
