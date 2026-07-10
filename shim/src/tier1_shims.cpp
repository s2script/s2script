// Slice 5D.1 — a self-contained definition of the ONE tier1 symbol the event system needs.
//
// Reading an IGameEvent field by name (`ev->GetInt(CKV3MemberName(key), …)`) constructs a
// CKV3MemberName from the C-string key. That constructor calls MakeStringToken → MurmurHash2LowerCase,
// which lives in tier1 (hl2sdk/tier1/generichash.cpp). The shim otherwise links no tier1, so at
// dlopen time Metamod fails with `undefined symbol: MurmurHash2LowerCase(char const*, unsigned int)`.
//
// Compiling the whole generichash.cpp — or linking tier1.a — cascades into CUtlString / UtlVectorMemory /
// V_tier0_strlen (tier1/tier0), which are not otherwise resolvable. Since CKV3MemberName needs ONLY this
// one function, we provide it here, self-contained: the EXACT MurmurHash2 algorithm from Valve's
// generichash.cpp, with a plain ASCII lowercase in place of CUtlString::ToLowerFast (identical result:
// ToLowerFast is an ASCII-only A–Z → a–z pass). The hash MUST match Valve's byte-for-byte or event field
// lookups silently return defaults — verified by inspection against hl2sdk/tier1/generichash.cpp (the
// MurmurHash2 core is line-for-line identical). Live field-read verification is pending 5D.1b: the event
// manager isn't acquired yet (CS2 doesn't export it), so this function isn't exercised at runtime yet —
// re-check it against Valve on any hl2sdk bump.

#include <cstdint>
#include <cstring>
#include <cstdlib>

namespace {
// Exact copy of Valve's MurmurHash2 core (generichash.cpp). LittleDWord() is identity on little-endian
// x86-64 (our only target); we use memcpy for the 4-byte load to avoid an unaligned-read UB.
uint32_t s2_MurmurHash2(const void* key, int len, uint32_t seed) {
    const uint32_t m = 0x5bd1e995u;
    const int r = 24;
    uint32_t h = seed ^ (uint32_t)len;
    const unsigned char* data = (const unsigned char*)key;
    while (len >= 4) {
        uint32_t k;
        std::memcpy(&k, data, 4);
        k *= m; k ^= k >> r; k *= m;
        h *= m; h ^= k;
        data += 4; len -= 4;
    }
    switch (len) {
        case 3: h ^= (uint32_t)data[2] << 16; [[fallthrough]];
        case 2: h ^= (uint32_t)data[1] << 8;  [[fallthrough]];
        case 1: h ^= (uint32_t)data[0]; h *= m;
    }
    h ^= h >> 13; h *= m; h ^= h >> 15;
    return h;
}
} // namespace

// Global-namespace symbol matching the SDK declaration (uint32 == unsigned int on x86-64 Linux):
//   uint32 MurmurHash2LowerCase(char const *pString, uint32 nSeed)   -> _Z20MurmurHash2LowerCasePKcj
unsigned int MurmurHash2LowerCase(char const* pString, unsigned int nSeed) {
    if (!pString) return s2_MurmurHash2("", 0, nSeed);
    size_t len = std::strlen(pString);
    char stackbuf[256];
    char* buf = (len < sizeof(stackbuf)) ? stackbuf : (char*)std::malloc(len + 1);
    if (!buf) return nSeed;  // OOM — degrade (won't match, but never crash)
    for (size_t i = 0; i < len; ++i) {
        char c = pString[i];
        buf[i] = (c >= 'A' && c <= 'Z') ? (char)(c + ('a' - 'A')) : c;
    }
    unsigned int h = s2_MurmurHash2(buf, (int)len, nSeed);
    if (buf != stackbuf) std::free(buf);
    return h;
}

// Only the 2-arg overload above is what CKV3MemberName(const char*) needs (via MakeStringToken →
// MurmurHash2LowerCase(str, 0x31415926)). The length-taking overload is intentionally NOT provided:
// it's unused, and Valve's version lowercases the whole string then hashes nLength bytes (a subtle
// parity trap if reimplemented). If a caller ever needs it, add it matching generichash.cpp exactly.

// ---------------------------------------------------------------------------------------------------
// quat_identity — the same self-contained-symbol pattern. mathlib.h declares
//   extern const Quaternion quat_identity;
// and it is referenced transitively (transform.h's CTransform uses it as the default orientation, so
// any SDK header pulling in transform.h drags the reference into the shim). It is DEFINED in
// mathlib/mathlib_base.cpp as `const Quaternion quat_identity(0,0,0,1)`; the engine does not export it,
// and the shim deliberately does not link mathlib.a (it would cascade like tier1). Quaternion is a
// POD `{ float x, y, z, w; }`; identity is (0,0,0,1). We need only the 16-byte symbol, so a
// layout-compatible stand-in avoids pulling all of mathlib into this TU. Re-check on any hl2sdk bump.
struct s2_QuatLayout { float x, y, z, w; };
extern const s2_QuatLayout quat_identity;
const s2_QuatLayout quat_identity = { 0.0f, 0.0f, 0.0f, 1.0f };

// The mathlib Vector/QAngle constants, same story (extern const in mathlib.h, defined in
// mathlib_base.cpp, dragged in transitively, engine doesn't export them, mathlib.a would cascade
// tier0). Vector and QAngle are both POD `{ float x, y, z; }`. vec3_origin/vec3_angle = (0,0,0);
// vec3_invalid = (FLT_MAX,FLT_MAX,FLT_MAX). Only the 12-byte symbol matters, so a stand-in suffices.
struct s2_Vec3Layout { float x, y, z; };
extern const s2_Vec3Layout vec3_origin;
const s2_Vec3Layout vec3_origin = { 0.0f, 0.0f, 0.0f };
extern const s2_Vec3Layout vec3_angle;
const s2_Vec3Layout vec3_angle = { 0.0f, 0.0f, 0.0f };
extern const s2_Vec3Layout vec3_invalid;
const s2_Vec3Layout vec3_invalid = { 3.402823466e+38f, 3.402823466e+38f, 3.402823466e+38f };

// ---------------------------------------------------------------------------------------------------
// EKV slice (Task 1) — leaf self-shims for entity2/entitykeyvalues.cpp + tier1/keyvalues3.cpp (the
// SDK's own CEntityKeyValues/KeyValues3 sources, compiled into the shim per the CSSharp approach).
// Classified via the plan's Rule 1 (self-contained logging/string leaves) and confirmed via `nm -Cu`
// triage against a Task-1 sniper build (comm -13 against the pre-EKV baseline `nm -u`) — every
// symbol below is either a pure string/number-parsing leaf or a discard-the-message logging no-op,
// none holds engine state or needs a live interface. All PLATFORM_INTERFACE-declared in
// tier0/dbg.h, tier0/logging.h, or tier0/platform.h with `extern "C"` linkage (DLL_EXPORT expands to
// `extern "C" __attribute__((visibility("default")))`), hence unmangled names below; MurmurHash2 is
// the one exception (regular C++ linkage, no PLATFORM_INTERFACE) — reuses the s2_MurmurHash2 core
// above (Valve's MurmurHash2 and MurmurHash2LowerCase share the identical core loop; this is the
// non-lowercased sibling CUtlSymbolLarge_Hash's `false` branch calls).
//
// THREE further undefined symbols from the same triage — CBufferString::Purge(int),
// CUtlBuffer::AddNullTermination(), CUtlBuffer::CUtlBuffer(int,int,BufferFlags_t) — are
// DELIBERATELY left undefined here (NOT self-shimmed): they are VERIFIED EXPORTED by the container's
// libtier0.so (`nm -DC libtier0.so`), so they live-resolve at dlopen (the shim .so link has no
// --no-undefined — every engine interface call already resolves this way). Self-shimming a real
// engine string-buffer/allocator method would be WRONG (wrong growable-buffer semantics, cross-heap
// risk) — see the plan's Global Constraints "self-shim vs live-resolve doctrine". memoverride.cpp
// (compiled alongside, see CMakeLists.txt) routes this TU's operator new/delete through the engine
// allocator so the shim and the engine share ONE heap, making the live-resolved CUtlBuffer ctor/dtor
// heap-safe.
// ---------------------------------------------------------------------------------------------------
#include <cstdarg>
#include <cstdio>

extern "C" {

// LoggingChannelID_t LOG_GENERAL — a plain global (not mangled either way in C++); referenced by
// Log_Msg(LOG_GENERAL, ...) call sites in CEntityKeyValues::AddRef/Release. Its value is inert since
// LoggingSystem_IsChannelEnabled below always returns false (Log_Msg's InternalMsg macro checks it
// first and skips the Log call entirely).
int LOG_GENERAL = 0;

// LoggingSystem_IsChannelEnabled(channelID, severity) — always-disabled, so every InternalMsg-style
// call site (Log_Msg in the AddRef/Release refcount-logging calls) short-circuits before ever
// building/forwarding a message. Matches the FIRST (PLATFORM_INTERFACE, unmangled) overload only —
// the LoggingVerbosity_t overload is PLATFORM_OVERLOAD (mangled), unused here.
bool LoggingSystem_IsChannelEnabled(int channelID, int severity) {
    (void)channelID; (void)severity;
    return false;
}

// LoggingSystem_Log(channelID, severity, fmt, ...) — a no-op returning LR_CONTINUE (0). Reachable
// only if a caller ever manages to get past the (always-false) IsChannelEnabled check above, or from
// a code path not gated by it; either way, discarding the message is safe (this is best-effort
// diagnostic logging, never behavior-affecting).
int LoggingSystem_Log(int channelID, int severity, const char* pMessageFormat, ...) {
    (void)channelID; (void)severity; (void)pMessageFormat;
    return 0;  // LR_CONTINUE
}

// Warning(fmt, ...) — CEntityKeyValues::SetKeyValue's misuse-guard paths (setting an attribute as a
// non-attribute key or vice versa). Forwarded to stderr so a real misuse is still visible in the
// container log, never silently dropped, but never crashes/blocks.
void Warning(const char* pMsg, ...) {
    va_list args;
    va_start(args, pMsg);
    std::vfprintf(stderr, pMsg ? pMsg : "", args);
    va_end(args);
}

// Plat_ExitProcess(nCode) — the real implementation behind the (macro-neutered-to-empty)
// Plat_FatalErrorFunc / the Plat_FatalError macro's terminate step; KV3's genuinely-fatal invariant
// violations (e.g. CKV3Arena::Root() called on a rootless pool arena — never hit by this slice's
// bNoRoot=true/scalar-only usage, but mirrors Valve's own intent: log + terminate, not a soft return).
void Plat_ExitProcess(int nCode) {
    std::fprintf(stderr, "[s2script] FATAL (tier0 shim Plat_ExitProcess, code=%d)\n", nCode);
    std::abort();
}

// Plat_NonFatalErrorFunc(fmt, ...) — a logged-but-recoverable warning path (e.g. alignment checks);
// log and return, never abort.
void Plat_NonFatalErrorFunc(const char* pMsg, ...) {
    va_list args;
    va_start(args, pMsg);
    std::vfprintf(stderr, pMsg ? pMsg : "", args);
    va_end(args);
}

// V_StringToBool/V_StringToInt32/V_StringToFloat32 — KeyValues3::FromString<T> template
// specializations (inline in keyvalues3.h) call these when a value is read back as a DIFFERENT type
// than it was stored as (e.g. GetInt() on a string-typed value). Never exercised by this slice (every
// EKV read-back in the demo/self-test Gets the SAME type it Set), so exact numeric-parsing fidelity
// doesn't matter for correctness here — only that linkage resolves and a call never crashes. The
// trailing (successful/remainder/flags/err_listener) parameters are accepted with primitive stand-ins
// (pointer-sized, matching the SysV ABI slot regardless of pointee type — the same technique the
// quat_identity/vec3_* PODs above use for layout-only compatibility) since extern "C" linkage means
// only the symbol NAME is linker-checked, not the parameter list.
bool V_StringToBool(const char* buf, bool default_value, bool* successful, char** remainder,
                     unsigned int flags, void* err_listener) {
    (void)remainder; (void)flags; (void)err_listener;
    if (successful) *successful = (buf != nullptr);
    if (!buf) return default_value;
    if (buf[0] == '1' || buf[0] == 't' || buf[0] == 'T' || buf[0] == 'y' || buf[0] == 'Y') return true;
    if (buf[0] == '0' || buf[0] == 'f' || buf[0] == 'F' || buf[0] == 'n' || buf[0] == 'N') return false;
    return default_value;
}

int32_t V_StringToInt32(const char* buf, int32_t default_value, bool* successful, char** remainder,
                         unsigned int flags, void* err_listener) {
    (void)flags; (void)err_listener;
    if (!buf) { if (successful) *successful = false; return default_value; }
    char* end = nullptr;
    long v = std::strtol(buf, &end, 10);
    if (remainder) *remainder = end;
    bool ok = (end != buf);
    if (successful) *successful = ok;
    return ok ? (int32_t)v : default_value;
}

float V_StringToFloat32(const char* buf, float default_value, bool* successful, char** remainder,
                         unsigned int flags, void* err_listener) {
    (void)flags; (void)err_listener;
    if (!buf) { if (successful) *successful = false; return default_value; }
    char* end = nullptr;
    float v = std::strtof(buf, &end);
    if (remainder) *remainder = end;
    bool ok = (end != buf);
    if (successful) *successful = ok;
    return ok ? v : default_value;
}

// _V_strncpy(dest, src, maxLen) — the function behind the V_strncpy(dest,src,count) macro
// (tier1/strtools.h); bounded copy with a GUARANTEED NUL terminator (unlike raw strncpy, which does
// not NUL-terminate if src is >= maxLen).
void _V_strncpy(char* dest, const char* src, int maxLen) {
    if (!dest || maxLen <= 0) return;
    if (!src) { dest[0] = '\0'; return; }
    std::strncpy(dest, src, (size_t)maxLen);
    dest[maxLen - 1] = '\0';
}

// V_tier0_memmove(dest, src, count) — the function behind the V_memmove(...) macro; a plain memmove.
void V_tier0_memmove(void* dest, const void* src, size_t count) {
    std::memmove(dest, src, count);
}

}  // extern "C"

// MurmurHash2(key, len, seed) — regular C++ linkage (no PLATFORM_INTERFACE on this declaration in
// generichash.h), reached via CUtlSymbolLarge_Hash's non-case-insensitive branch (a runtime bool
// parameter, so both branches' call targets are always emitted regardless of which is taken).
// Valve's MurmurHash2 and MurmurHash2LowerCase share the identical core loop (generichash.cpp) minus
// the lowercase pass — reuse the anonymous-namespace core above rather than duplicating it.
uint32_t MurmurHash2(const void* key, int len, uint32_t seed) {
    return s2_MurmurHash2(key, len, seed);
}
