// Plugin-declared engine calls (plugin-gamedata slice, Task 6) — descriptor RESOLUTION + INVOKE.
// See docs/superpowers/specs/2026-07-24-plugin-gamedata-design.md.
//
// WHY A BARE VTABLE INDEX IS NEVER TRUSTED (spec §3): on the pinned libserver.so the borrowed
// ItemServices slots 24/25/26 are GiveNamedItem-overload THUNKS. A thunk is valid in-range code, so
// the .text-range guard (the IsAddressInServerText idiom used throughout s2script_mm.cpp) does NOT
// catch it — the call silently misbehaves instead of crashing. Hence a "vtable" target MUST carry a
// masked `prologue` pattern, matched here at the resolved slot; .text-range validation stays as the
// necessary-but-insufficient first gate. A "signature" target is self-validating (it matches this
// build or it does not) and is additionally required to match UNIQUELY (the Rule-2 doctrine in
// docs/re-strategy.md, mirroring ResolveSigValidated in s2script_mm.cpp).
//
// THE INVOKE TECHNIQUE. Under SysV x86-64, integer-class and float-class arguments are assigned to
// two INDEPENDENT register sequences (rdi/rsi/rdx/rcx/r8/r9 and xmm0..7), so positional interleaving
// between the classes does not matter to the callee. Therefore ONE fixed max-arity prototype can
// call every descriptor in the closed v1 vocabulary: `this` in rdi plus the 5 remaining GP slots and
// all 8 xmm slots are ALWAYS passed, and the callee simply reads the prefix its own prototype
// declares and ignores the rest. That buys the whole closed vocabulary with no hand-written asm and
// no combinatorial switch — only two casts, selected by RETURN class (rax vs xmm0). This is valid
// only for NON-VARIADIC callees; variadics (and overloads) are out of scope per spec §4, which is
// exactly why the vocabulary is closed.
//
// LAYERING. Nothing here names a game class/field/function: the plugin's own regenerable gamedata
// supplies every name as an opaque string that crosses the core untouched (the same discipline as
// __s2_schema_offset). No raw pointer crosses back to the core — an entity RETURN is converted to a
// packed CEntityHandle read off the entity's own identity, which the core then runs through the
// books-gated __s2_handle_adopt path (spec §4), so a raw pointer can never mint an EntityRef.
#include "engine_calls.h"
#include "sigscan.h"
#include "vtable.h"   // s2vtable::GetVTableByName — RTTI vtable-by-name (CS2 exports no game vtables)

// Entity system: CGameEntitySystem / CConcreteEntityList::m_pIdentityChunks / CEntityIdentity /
// MAX_TOTAL_ENTITIES / EF_IS_INVALID_EHANDLE — the receiver + entity-arg resolution walk below reads
// ONLY system-owned identity-chunk memory (never the instance) to decide liveness.
#include <entity2/entitysystem.h>
#include <entity2/entityinstance.h>   // CEntityInstance::GetRefEHandle (inline; identity-backed)

#include <link.h>     // dl_iterate_phdr, ElfW
#include <cstdio>     // snprintf — the degrade-reason string
#include <cstring>    // strcmp/strstr/memcpy
#include <string>
#include <vector>

// The shim's existing non-static bridge to GetEntitySystem() (defined in s2script_mm.cpp, where the
// IGameResourceService pointer + its gamedata offset live). Declared locally exactly as ekv.cpp
// does — the entity system is re-read on EVERY call, never cached, so it becomes valid once a map is
// live and can never go stale across a changelevel.
class CGameEntitySystem;
CGameEntitySystem* S2_EntitySystemBridge();

namespace {

// The engine's server module. Used when a descriptor names no module (the "vtable" target kind
// carries a class, not a module). Kept HERE rather than in the core so no module/game identifier is
// compiled into core/ (the check-boundary invariant); s2_schema_offset hardcodes the same soname.
constexpr const char* kEngineModule = "libserver.so";

// Arg budget (spec §4): `this` consumes the first of SysV's six GP argument registers, so at most 5
// integer-class args, and at most 8 float args. The SDK's build-time validator rejects a descriptor
// that exceeds either bound; these are the belt-and-braces runtime guards.
constexpr int kMaxGpArgs = 5;
constexpr int kMaxFpArgs = 8;

// A vtable has a naturally small bound; cap the index BEFORE the vt[] read so a corrupt/hostile
// index degrades instead of reading out of bounds (the Shim_EntitySubobjVcall precedent).
constexpr int kMaxVtableIndex = 512;

// Descriptor-table cap. Resolution is idempotent per resolved address (see the dedupe in
// S2_EngineCallResolve), so a plugin reload loop re-uses ids rather than growing the table; the cap
// is the last line of defence against unbounded growth.
constexpr size_t kMaxCalls = 1024;

// gpKind values — must match the Rust side's classify_args (0 scalar, 1 entity, 2 string, 3 vector).
enum : unsigned char { kArgScalar = 0, kArgEntity = 1, kArgString = 2, kArgVector = 3 };
// retKind values — must match the core's return vocabulary.
enum { kRetVoid = 0, kRetBool = 1, kRetInt = 2, kRetFloat = 3, kRetEntity = 4 };

struct ResolvedCall { void* fn; };
std::vector<ResolvedCall> g_calls;   // the returned call id is the index; entries are never removed
                                     // (an id stays valid for the process; the core's registry owns
                                     // per-plugin lifetime and drops its own table on unload)

// ---------------------------------------------------------------------------
// Module text lookup. A per-TU copy of s2script_mm.cpp's FindModuleText (which is file-static
// there): pick the LARGEST PF_X segment across ALL loaded modules whose soname contains `soname`,
// because Metamod:Source inserts its own thin libserver.so proxy via the gameinfo SearchPath whose
// path ALSO contains the substring — stopping at the first match grabs the ~95 KB proxy instead of
// the real ~25 MB game module (Slice 5D.2). vtable.cpp keeps the same local copy for the same
// TU-boundary reason.
// ---------------------------------------------------------------------------
struct ModText { const uint8_t* text; size_t size; };

ModText FindModuleText(const char* soname) {
    struct Ctx { const char* name; ModText out; } ctx{ soname, { nullptr, 0 } };
    dl_iterate_phdr([](struct dl_phdr_info* info, size_t, void* data) -> int {
        auto* c = static_cast<Ctx*>(data);
        if (!info->dlpi_name || !std::strstr(info->dlpi_name, c->name)) return 0;
        for (int i = 0; i < info->dlpi_phnum; i++) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type == PT_LOAD && (ph.p_flags & PF_X) && ph.p_filesz > c->out.size) {
                c->out.text = reinterpret_cast<const uint8_t*>(info->dlpi_addr + ph.p_vaddr);
                c->out.size = ph.p_filesz;                 // largest PF_X seg across all matches
            }
        }
        return 0;   // keep scanning ALL modules — the metamod proxy must not shadow the game module
    }, &ctx);
    return ctx.out;
}

// The IsAddressInServerText guard, generalized to the descriptor's own module: a resolved
// address/slot must land inside that module's executable segment. A borrowed or stale index/xref
// could point anywhere; this stops the out-of-module case before the first call (it canNOT stop a
// wrong-but-in-range function — that is what `prologue` is for).
bool InModuleText(const ModText& mt, const void* fn) {
    if (!mt.text || !fn) return false;
    const uint8_t* p = static_cast<const uint8_t*>(fn);
    return p >= mt.text && p < mt.text + mt.size;
}

// Masked compare of `pat` at `fn`, fully bounds-checked against the module's text before any read.
bool MatchesAt(const ModText& mt, const void* fn, const std::vector<int>& pat) {
    if (!mt.text || !fn || pat.empty()) return false;
    const uint8_t* p = static_cast<const uint8_t*>(fn);
    if (p < mt.text || p + pat.size() > mt.text + mt.size) return false;
    for (size_t i = 0; i < pat.size(); i++) {
        if (pat[i] >= 0 && p[i] != static_cast<uint8_t>(pat[i])) return false;
    }
    return true;
}

// ---------------------------------------------------------------------------
// Receiver / entity-arg resolution: (index, engine serial) -> CEntityInstance*, decided ENTIRELY in
// the system-owned identity chunk (the s2_ent_resolve idiom in s2script_mm.cpp — that helper is
// file-static there, so this is the same walk, not a different rule). Instance memory is never read
// to decide liveness. Returns null on an out-of-range index, a free slot, or a reused serial, so a
// stale ref degrades instead of dangling.
// ---------------------------------------------------------------------------
CEntityInstance* ResolveEntity(int index, int serial) {
    CGameEntitySystem* es = S2_EntitySystemBridge();
    if (!es) return nullptr;                                  // no map yet / service unavailable
    if (index < 0 || serial < 0 || index >= MAX_TOTAL_ENTITIES) return nullptr;
    CEntityIdentity* chunk = es->m_EntityList.m_pIdentityChunks[index / MAX_ENTITIES_IN_LIST];
    if (!chunk) return nullptr;                               // sparse (unallocated) chunk
    CEntityIdentity* id = &chunk[index % MAX_ENTITIES_IN_LIST];
    if (id->m_flags & EF_IS_INVALID_EHANDLE) return nullptr;   // free/unallocated identity slot
    if (id->GetRefEHandle().GetSerialNumber() != serial) return nullptr;   // stale slot reuse
    return id->m_pInstance;   // may be null (removal in progress) — caller treats null as not-live
}

// `returns: "entity"`: convert the returned pointer into a packed CEntityHandle WITHOUT ever letting
// the pointer itself become a ref. The handle is read off the entity's own identity (the same
// identity read Shim_EntityCreate/__s2_handle_adopt's callers rely on) and then round-tripped
// through the identity chunk: the system's books must hand back exactly this instance for that
// (index, serial), otherwise the pointer is not a live entity of this system and we yield 0. The
// core still runs the surviving handle through the books-gated adopt path, so a handle the HOST's
// books do not vouch for degrades to null there (spec §4).
uint64_t EntityHandleFromPtr(void* p) {
    if (!p) return 0;
    CEntityHandle h = static_cast<CEntityInstance*>(p)->GetRefEHandle();
    if (ResolveEntity(h.GetEntryIndex(), h.GetSerialNumber()) != p) return 0;
    return static_cast<uint64_t>(static_cast<uint32_t>(h.ToInt()));
}

int Fail(char* out, int cap, const char* reason) {
    if (out && cap > 0) std::snprintf(out, static_cast<size_t>(cap), "%s", reason);
    return -1;
}

// The two max-arity prototypes (see THE INVOKE TECHNIQUE above). All 5 GP + 8 xmm slots are always
// passed; the callee reads only what its real prototype declares.
using FnU64 = uint64_t (*)(void*, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                           double, double, double, double, double, double, double, double);
using FnF32 = float    (*)(void*, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                           double, double, double, double, double, double, double, double);

}  // namespace

// ---------------------------------------------------------------------------
// Resolve. Returns a call id >= 0, or -1 with a named reason (spec §12 "Load" row) — every failure
// here degrades exactly ONE descriptor; the core logs the reason once and Engine.call() yields null.
// ---------------------------------------------------------------------------
int S2_EngineCallResolve(const char* kind, const char* module, const char* pattern,
                         const char* resolve, const char* className, int vtableIndex,
                         const char* prologue, char* reasonOut, int reasonCap) {
    if (reasonOut && reasonCap > 0) reasonOut[0] = '\0';
    if (!kind || !kind[0]) return Fail(reasonOut, reasonCap, "descriptor has no target kind");

    const char* mod = (module && module[0]) ? module : kEngineModule;
    ModText mt = FindModuleText(mod);
    if (!mt.text) return Fail(reasonOut, reasonCap, "target module is not loaded");

    void* fn = nullptr;

    if (std::strcmp(kind, "signature") == 0) {
        if (!pattern || !pattern[0]) return Fail(reasonOut, reasonCap, "signature has no pattern");
        std::vector<int> pat = s2sig::ParsePattern(pattern);
        if (pat.empty()) return Fail(reasonOut, reasonCap, "malformed signature pattern");
        // Rule 2: uniqueness, not just presence — an ambiguous pattern is as unusable as a missing one.
        int matches = s2sig::CountPattern(mt.text, mt.size, pat, 2);
        if (matches == 0) return Fail(reasonOut, reasonCap, "signature did not match this build");
        if (matches > 1)  return Fail(reasonOut, reasonCap, "signature is ambiguous (>1 match — tighten it)");
        int64_t matchOff  = s2sig::FindPattern(mt.text, mt.size, pat);
        int64_t targetOff = matchOff;                       // "direct": the match IS the target
        const char* res = (resolve && resolve[0]) ? resolve : "direct";
        if (std::strcmp(res, "ctor-body-xref") == 0) {
            targetOff = s2sig::ResolveCtorXref(mt.text, mt.size, matchOff);
        } else if (std::strcmp(res, "lea-disp") == 0) {
            targetOff = s2sig::ResolveLeaDisp(mt.text, mt.size, matchOff, /*dispOff=*/3, /*instrLen=*/7);
        } else if (std::strcmp(res, "direct") != 0) {
            return Fail(reasonOut, reasonCap, "unknown resolve strategy");
        }
        if (targetOff == s2sig::kFail) return Fail(reasonOut, reasonCap, "resolve step failed (xref/lea)");
        // uintptr arithmetic: a lea/xref target can legitimately compute to a NEGATIVE offset
        // (.rodata precedes .text in the mapping), which InModuleText then rejects.
        fn = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mt.text) +
                                     static_cast<uintptr_t>(targetOff));
        if (!InModuleText(mt, fn)) return Fail(reasonOut, reasonCap, "resolved address outside the module's .text");
    } else if (std::strcmp(kind, "vtable") == 0) {
        if (!className || !className[0]) return Fail(reasonOut, reasonCap, "vtable target has no class");
        // The v1 validator set is exactly { prologue }, and it is MANDATORY for a vtable target: the
        // SDK fails the BUILD on a missing one, and this is the load-time backstop.
        if (!prologue || !prologue[0]) return Fail(reasonOut, reasonCap, "vtable target requires validate.prologue");
        if (vtableIndex < 0 || vtableIndex >= kMaxVtableIndex)
            return Fail(reasonOut, reasonCap, "vtable index out of range");
        void** vt = s2vtable::GetVTableByName(mod, className);
        if (!vt) return Fail(reasonOut, reasonCap, "class RTTI vtable not found on this build");
        fn = vt[vtableIndex];
        if (!InModuleText(mt, fn)) return Fail(reasonOut, reasonCap, "resolved slot outside libserver .text");
        std::vector<int> pro = s2sig::ParsePattern(prologue);
        if (pro.empty()) return Fail(reasonOut, reasonCap, "malformed validate.prologue pattern");
        if (!MatchesAt(mt, fn, pro))
            return Fail(reasonOut, reasonCap, "prologue mismatch (resolved slot is not the intended function)");
    } else {
        return Fail(reasonOut, reasonCap, "unknown target kind");
    }

    // Idempotent: the same resolved address always yields the same id, so a plugin reload (or two
    // plugins declaring the same target) re-uses the entry instead of growing the table.
    for (size_t i = 0; i < g_calls.size(); i++) {
        if (g_calls[i].fn == fn) return static_cast<int>(i);
    }
    if (g_calls.size() >= kMaxCalls) return Fail(reasonOut, reasonCap, "engine-call table is full");
    g_calls.push_back(ResolvedCall{ fn });
    return static_cast<int>(g_calls.size() - 1);
}

// ---------------------------------------------------------------------------
// Invoke. Degrade-never-crash: every failure path returns 0 with *retOut zeroed (spec §12 "Call"
// row) — a stale receiver, an unresolved `via` sub-object, or an out-of-budget arg list is a no-op,
// never a wild call.
// ---------------------------------------------------------------------------
int S2_EngineCallInvoke(int callId, int entIndex, int entSerial, int subObjOff,
                        const uint64_t* gp, const unsigned char* gpKind, int gpCount,
                        const double* fp, int fpCount,
                        const char* const* strs, const float* vecs,
                        int retKind, uint64_t* retOut) {
    if (retOut) *retOut = 0;
    if (callId < 0 || static_cast<size_t>(callId) >= g_calls.size()) return 0;
    if (retKind < kRetVoid || retKind > kRetEntity) return 0;     // validated BEFORE any call
    if (gpCount < 0 || gpCount > kMaxGpArgs) return 0;
    if (fpCount < 0 || fpCount > kMaxFpArgs) return 0;
    if (gpCount > 0 && (!gp || !gpKind)) return 0;
    if (fpCount > 0 && !fp) return 0;
    void* fn = g_calls[static_cast<size_t>(callId)].fn;
    if (!fn) return 0;

    // Receiver: a books-gated (index, serial) pair, optionally hopping through ONE schema-named
    // sub-object pointer (`receiver.via`, whose offset the core live-resolves via the cached
    // __s2_schema_offset and passes here; < 0 means "no hop").
    CEntityInstance* self = ResolveEntity(entIndex, entSerial);
    if (!self) return 0;
    void* thisPtr = self;
    if (subObjOff >= 0) {
        thisPtr = *reinterpret_cast<void**>(reinterpret_cast<uint8_t*>(self) + subObjOff);
        if (!thisPtr) return 0;      // sub-object absent on this entity — degrade this invocation
    }

    uint64_t g[kMaxGpArgs] = { 0, 0, 0, 0, 0 };
    double   f[kMaxFpArgs] = { 0, 0, 0, 0, 0, 0, 0, 0 };

    for (int i = 0; i < gpCount; i++) {
        switch (gpKind[i]) {
            case kArgScalar:
                g[i] = gp[i];                        // bool / int, already widened by the core
                break;
            case kArgEntity: {
                // (uint64)index<<32|serial -> a serial-gated pointer. A stale/absent ref becomes
                // nullptr, which is a legitimate "no entity" argument (the JS side passes null the
                // same way), so this does NOT fail the call.
                int idx = static_cast<int>(static_cast<int32_t>(static_cast<uint32_t>(gp[i] >> 32)));
                int ser = static_cast<int>(static_cast<int32_t>(static_cast<uint32_t>(gp[i] & 0xffffffffu)));
                g[i] = reinterpret_cast<uint64_t>(ResolveEntity(idx, ser));
                break;
            }
            case kArgString: {
                // gp[i] indexes `strs`. A descriptor has at most one string per GP slot, so gpCount
                // bounds the index — cheap self-contained validation of a core-supplied array whose
                // length does not cross the ABI. The buffer is valid only for THIS call (spec §4).
                if (!strs || gp[i] >= static_cast<uint64_t>(gpCount)) return 0;
                g[i] = reinterpret_cast<uint64_t>(strs[gp[i]]);
                break;
            }
            case kArgVector: {
                // gp[i] indexes `vecs` in 3-float strides; a vector is passed BY ADDRESS (integer
                // class). Same gpCount bound as strings.
                if (!vecs || gp[i] >= static_cast<uint64_t>(gpCount)) return 0;
                g[i] = reinterpret_cast<uint64_t>(&vecs[gp[i] * 3]);
                break;
            }
            default:
                return 0;                            // unknown arg kind — degrade, never guess
        }
    }
    for (int i = 0; i < fpCount; i++) f[i] = fp[i];

    if (retKind == kRetFloat) {
        float r = reinterpret_cast<FnF32>(fn)(thisPtr, g[0], g[1], g[2], g[3], g[4],
                                              f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]);
        if (retOut) {
            uint32_t bits = 0;
            std::memcpy(&bits, &r, sizeof(bits));     // the core reads the low 32 bits as f32
            *retOut = bits;
        }
        return 1;
    }

    uint64_t r = reinterpret_cast<FnU64>(fn)(thisPtr, g[0], g[1], g[2], g[3], g[4],
                                             f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]);
    if (retOut) {
        switch (retKind) {
            case kRetVoid:   break;                                        // *retOut stays 0
            case kRetBool:   *retOut = (r & 0xff) ? 1 : 0; break;           // al
            case kRetInt:    *retOut = static_cast<uint32_t>(r); break;      // eax; core reads as i32
            case kRetEntity: *retOut = EntityHandleFromPtr(reinterpret_cast<void*>(r)); break;
            default:         break;
        }
    }
    return 1;
}
