// Plugin-declared engine calls (plugin-gamedata slice, Task 6) — descriptor RESOLUTION + INVOKE.
// See docs/superpowers/specs/2026-07-24-plugin-gamedata-design.md.
//
// WHY A BARE VTABLE INDEX IS NEVER TRUSTED (spec §3): on the pinned libserver.so the borrowed
// ItemServices slots 24/25/26 are GiveNamedItem-overload THUNKS. A thunk is valid in-range code, so
// the .text-range guard (the IsAddressInServerText idiom used throughout s2script_mm.cpp) does NOT
// catch it — the call silently misbehaves instead of crashing. Hence a "vtable" target MUST carry a
// masked `prologue` pattern, matched at the resolved slot; .text-range validation stays as the
// necessary-but-insufficient first gate. A "signature" target must additionally match UNIQUELY (the
// Rule-2 doctrine in docs/re-strategy.md, mirroring ResolveSigValidated in s2script_mm.cpp).
//
// UNIQUENESS IS ALSO INSUFFICIENT, which is why a signature target may carry validators too: a
// borrowed signature can match exactly ONCE at the WRONG in-range function (proven twice on the
// pinned build). The whole `validate` object crosses this ABI as JSON and is evaluated by the
// engine-free call_validate.cpp — see that TU for the closed vocabulary and what each gate proves.
//
// THE INVOKE TECHNIQUE lives behind call_abi.h. SysV uses independent GP/SSE streams; Microsoft x64
// uses the AUTHOR-POSITIONAL class sequence that now crosses the engine-ops ABI. The Windows backend
// uses one MASM bridge to place raw typed slots exactly (paired GP/XMM registers, shadow space,
// stack slots, RAX/XMM0 return); it never invents a mismatched C++ function-pointer type.
// This remains valid only for NON-VARIADIC callees; variadics and overloads are out of scope.
//
// LAYERING. Nothing here names a game class/field/function: the plugin's own regenerable gamedata
// supplies every name as an opaque string that crosses the core untouched (the same discipline as
// __s2_schema_offset). No raw pointer crosses back to the core — an entity RETURN is converted to a
// packed CEntityHandle read off the entity's own identity, which the core then runs through the
// books-gated __s2_handle_adopt path (spec §4), so a raw pointer can never mint an EntityRef.
#include "engine_calls.h"
#include "call_abi.h"
#include "sigscan.h"
#include "vtable.h"   // s2vtable::GetVTableByName — RTTI vtable-by-name (CS2 exports no game vtables)
#include "platform/module.h"
// The CLOSED validator vocabulary (prologue / string-xref / vtable-member). It lives in its own
// engine-free TU so shim/tests/call_validate_test.cpp can drive the SHIPPED gates over a synthetic
// module image — this TU cannot be compiled outside the game (it includes the entity system), and a
// validator that cannot fail in a test is decoration.
#include "call_validate.h"

// Entity system: CGameEntitySystem / CConcreteEntityList::m_pIdentityChunks / CEntityIdentity /
// MAX_TOTAL_ENTITIES / EF_IS_INVALID_EHANDLE — the receiver + entity-arg resolution walk below reads
// ONLY system-owned identity-chunk memory (never the instance) to decide liveness.
#include <entity2/entitysystem.h>
#include <entity2/entityinstance.h>   // CEntityInstance::GetRefEHandle (inline; identity-backed)

#include <cstdio>     // snprintf — the degrade-reason string
#include <cstring>    // strcmp
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

// Closed-vocabulary budget shared with the core and both ABI backends. The SDK's build-time
// validator rejects a descriptor that exceeds either class bound; these are runtime backstops.
// 9 declared integer-class args, plus the optional receiver = 10 integer slots total.
//
// This budget includes stack-passed args; compiler-generated calls in each backend own the register,
// shadow-space and spill placement.
constexpr int kMaxGpArgs = 9;
constexpr int kMaxFpArgs = 8;

// A vtable has a naturally small bound; cap the index BEFORE the vt[] read so a corrupt/hostile
// index degrades instead of reading out of bounds (the Shim_EntitySubobjVcall precedent).
constexpr int kMaxVtableIndex = 512;

// The "no entity" value for `returns: "entity"` — see the S2_ENTITY_HANDLE_NONE comment in
// engine_calls.h for why it is not 0. Core skips decoding when it sees it.
constexpr uint64_t kInvalidEntityHandle = S2_ENTITY_HANDLE_NONE;

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

using ModText = s2platform::ModuleView;

ModText FindModuleText(const char* soname) {
    return s2platform::FindModule(soname);
}

// The IsAddressInServerText guard, generalized to the descriptor's own module: a resolved
// address/slot must land inside that module's executable segment. A borrowed or stale index/xref
// could point anywhere; this stops the out-of-module case before the first call (it canNOT stop a
// wrong-but-in-range function — that is what the VALIDATORS are for, and the two must stay
// distinguishable in the reason string: "outside .text" means the pattern computed off the map, a
// validator failure means it computed a real in-range function and it is the WRONG one).
bool InModuleText(const ModText& mt, const void* fn) {
    return mt.ContainsExecutable(fn);
}

// Hand the (engine-free) validator TU the module's two views.
s2validate::ModuleView ViewOf(const ModText& mt) {
    s2validate::ModuleView mv;
    mv.text     = mt.text;
    mv.textSize = mt.textSize;
    mv.lo       = mt.lo;
    mv.hi       = mt.hi;
    return mv;
}

// The one engine touchpoint the vocabulary needs, injected rather than reached for: RTTI
// vtable-by-name. The validators never name a module or a class — both come from the descriptor.
s2validate::Ops ValidatorOps() {
    s2validate::Ops ops;
    ops.vtable_by_name = &s2vtable::GetVTableByName;
    return ops;
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

// Is `p` an instance the entity system's own books currently vouch for? Decided WITHOUT reading a
// single byte of `p`: we walk the system-owned identity chunks and compare instance POINTERS. That
// ordering is the whole point — `returns: "entity"` is author-declared, so a descriptor whose real
// return type is not a CBaseEntity* (a wrong declaration, or a call that misbehaved) would otherwise
// have its vtable/identity read through a wild pointer and SEGV. Membership first, deref second.
//
// Cost: at most MAX_TOTAL_ENTITIES pointer compares, and only on an entity-returning invoke. That is
// the right trade against a wild read, and it is the same books-first rule the rest of the entity
// system already follows.
bool PointerIsLiveEntity(const void* p) {
    CGameEntitySystem* es = S2_EntitySystemBridge();
    if (!es || !p) return false;
    for (int i = 0; i < MAX_TOTAL_ENTITIES; i++) {
        CEntityIdentity* chunk = es->m_EntityList.m_pIdentityChunks[i / MAX_ENTITIES_IN_LIST];
        if (!chunk) {                                  // sparse chunk — skip the whole block
            i += MAX_ENTITIES_IN_LIST - 1 - (i % MAX_ENTITIES_IN_LIST);
            continue;
        }
        CEntityIdentity* id = &chunk[i % MAX_ENTITIES_IN_LIST];
        if (id->m_flags & EF_IS_INVALID_EHANDLE) continue;   // free slot
        if (id->m_pInstance == p) return true;
    }
    return false;
}

// `returns: "entity"`: convert the returned pointer into a packed CEntityHandle WITHOUT ever letting
// the pointer itself become a ref. Order of operations matters:
//   1. PointerIsLiveEntity(p) — books-only membership check, NO deref (see above).
//   2. only then read the handle off the entity's own identity, and
//   3. round-trip it through the identity chunk: the books must hand back exactly this instance for
//      that (index, serial).
// The core still runs the surviving handle through the books-gated adopt path, so a handle the HOST's
// books do not vouch for degrades to null there (spec §4).
//
// The "no entity" sentinel is kInvalidEntityHandle (0xFFFFFFFF, the engine's own convention), NOT 0:
// zero is a perfectly legal encoding (index 0, serial 0), so returning it for "none" would make an
// absent entity indistinguishable from a real handle to entity slot 0 once core decodes it.
uint64_t EntityHandleFromPtr(void* p) {
    if (!p) return kInvalidEntityHandle;
    if (!PointerIsLiveEntity(p)) return kInvalidEntityHandle;   // MUST precede any deref of p
    CEntityHandle h = static_cast<CEntityInstance*>(p)->GetRefEHandle();
    if (ResolveEntity(h.GetEntryIndex(), h.GetSerialNumber()) != p) return kInvalidEntityHandle;
    return static_cast<uint64_t>(static_cast<uint32_t>(h.ToInt()));
}

int Fail(char* out, int cap, const char* reason) {
    if (out && cap > 0) std::snprintf(out, static_cast<size_t>(cap), "%s", reason);
    return -1;
}

}  // namespace

// The books-first pointer -> handle pack, exposed for engine_hooks.cpp's receiver surfacing (a
// detour's `this` is the same question as an entity RETURN: is this pointer an entity the host's
// books currently vouch for?). Thin wrapper, so there is exactly ONE implementation of the rule.
uint32_t S2_EntityHandleFromPtr(void* p) {
    return static_cast<uint32_t>(EntityHandleFromPtr(p));   // kInvalidEntityHandle -> the none marker
}

void* S2_ResolveEntity(int index, int serial) {
    return ResolveEntity(index, serial);
}

// InModuleText's rule, re-asked WITHOUT a module name, for a caller that holds only a resolved
// address (engine_hooks.cpp, which must never patch bytes it cannot prove are code). Module-agnostic
// on purpose: a hook target may live in whichever module its descriptor named, and this TU is the
// one that already owns the platform query — a second copy in engine_hooks.cpp would also drag the
// question of "which module?" somewhere that has no answer to it.
//
// This checks every module's executable ranges, rather than FindModuleText's
// largest-executable-range-of-one-name: the question here is containment, not identification, so the
// metamod-proxy tie-break that rule exists for does not apply.
int S2_AddressIsExecutable(const void* addr) {
    return s2platform::IsExecutableAddress(addr) ? 1 : 0;
}

// The module views behind an address: the PF_X segment that CONTAINS it, plus the module's full
// LOAD extent (lo..hi). Both are needed because a validator's reads legitimately reach outside .text
// — .rodata precedes it in the mapping — while the code it decodes must not.
int S2_ModuleViewForAddress(const void* addr, const unsigned char** outText, std::size_t* outSize,
                            const unsigned char** outLo, const unsigned char** outHi) {
    if (!addr || !outText || !outSize || !outLo || !outHi) return 0;
    s2platform::ModuleView view;
    if (!s2platform::ModuleViewForAddress(addr, &view)) return 0;
    *outText = view.text;
    *outSize = view.textSize;
    *outLo   = view.lo;
    *outHi   = view.hi;
    return 1;
}

// The address behind a call id, for the ONE caller that needs bytes rather than a callable: the
// declarative-inbound-hook install (see the header for why this exists at all). An unknown id
// returns 0, which S2_HookInstall already refuses by name ("hook target address is null") — so a
// stale id degrades that hook instead of patching address 0.
int64_t S2_EngineCallAddress(int callId) {
    if (callId < 0 || static_cast<size_t>(callId) >= g_calls.size()) return 0;
    return static_cast<int64_t>(reinterpret_cast<uintptr_t>(g_calls[static_cast<size_t>(callId)].fn));
}

// ---------------------------------------------------------------------------
// Resolve. Returns a call id >= 0, or -1 with a named reason (spec §12 "Load" row) — every failure
// here degrades exactly ONE descriptor; the core logs the reason once and Engine.call() yields null.
// ---------------------------------------------------------------------------
int S2_EngineCallResolve(const char* kind, const char* module, const char* pattern,
                         const char* resolve, const char* className, int vtableIndex,
                         const char* validateJson, char* reasonOut, int reasonCap) {
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
        int matches = s2sig::CountPattern(mt.text, mt.textSize, pat, 2);
        if (matches == 0) return Fail(reasonOut, reasonCap, "signature did not match this build");
        if (matches > 1)  return Fail(reasonOut, reasonCap, "signature is ambiguous (>1 match — tighten it)");
        int64_t matchOff  = s2sig::FindPattern(mt.text, mt.textSize, pat);
        int64_t targetOff = matchOff;                       // "direct": the match IS the target
        const char* res = (resolve && resolve[0]) ? resolve : "direct";
        if (std::strcmp(res, "ctor-body-xref") == 0) {
            targetOff = s2sig::ResolveCtorXref(mt.text, mt.textSize, matchOff);
        } else if (std::strcmp(res, "lea-disp") == 0) {
            targetOff = s2sig::ResolveLeaDisp(mt.text, mt.textSize, matchOff,
                                              /*dispOff=*/3, /*instrLen=*/7);
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
        // A `prologue` is MANDATORY for a vtable target — a rule about which validator must be
        // PRESENT, separate from how validators are evaluated below. The SDK fails the BUILD on a
        // missing one; this is the load-time backstop.
        if (!s2validate::DeclaresPrologue(validateJson))
            return Fail(reasonOut, reasonCap, "vtable target requires validate.prologue");
        if (vtableIndex < 0 || vtableIndex >= kMaxVtableIndex)
            return Fail(reasonOut, reasonCap, "vtable index out of range");
        void** vt = s2vtable::GetVTableByName(mod, className);
        if (!vt) return Fail(reasonOut, reasonCap, "class RTTI vtable not found on this build");
        fn = vt[vtableIndex];
        if (!InModuleText(mt, fn)) return Fail(reasonOut, reasonCap, "resolved slot outside libserver .text");
    } else {
        return Fail(reasonOut, reasonCap, "unknown target kind");
    }

    // THE SEMANTIC GATE, after the .text-range check and never merged into it (see InModuleText):
    // every validator computes or dereferences FROM `fn`, so it must first be known to be inside the
    // module. One shared, kind-agnostic pass over the descriptor's whole `validate` object — the
    // vocabulary is CLOSED, so an unknown key fails HERE by name rather than being silently ignored,
    // which is the one way a mistyped gate could vanish without a trace.
    if (!s2validate::Run(validateJson, ViewOf(mt), mod, fn, ValidatorOps(), reasonOut, reasonCap))
        return -1;

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
                        const unsigned char* argClass, int argCount,
                        const uint64_t* gp, const unsigned char* gpKind, int gpCount,
                        const double* fp, int fpCount,
                        const char* const* strs, const float* vecs,
                        int retKind, uint64_t* retOut) {
    if (retOut) *retOut = 0;
    if (callId < 0 || static_cast<size_t>(callId) >= g_calls.size()) return 0;
    if (retKind < kRetVoid || retKind > kRetEntity) return 0;     // validated BEFORE any call
    if (argCount < 0 || argCount > kMaxGpArgs + kMaxFpArgs) return 0;
    if (argCount > 0 && !argClass) return 0;
    if (gpCount < 0 || gpCount > kMaxGpArgs) return 0;
    if (fpCount < 0 || fpCount > kMaxFpArgs) return 0;
    if (gpCount > 0 && (!gp || !gpKind)) return 0;
    if (fpCount > 0 && !fp) return 0;
    void* fn = g_calls[static_cast<size_t>(callId)].fn;
    if (!fn) return 0;

    // Receiver: a books-gated (index, serial) pair, optionally hopping through ONE schema-named
    // sub-object pointer (`receiver.via`, whose offset the core live-resolves via the cached
    // __s2_schema_offset and passes here; < 0 means "no hop").
    //
    // entIndex < 0 is the RECEIVERLESS marker (`receiver.kind: "none"` — a static/free engine
    // function, which has no `this` at all). Nothing is resolved and no slot is consumed: the first
    // declared arg takes position 0. Kept as a sentinel on the existing receiver parameters.
    const bool hasReceiver = entIndex >= 0;
    void* thisPtr = nullptr;
    if (hasReceiver) {
        CEntityInstance* self = ResolveEntity(entIndex, entSerial);
        if (!self) return 0;
        thisPtr = self;
        if (subObjOff >= 0) {
            thisPtr = *reinterpret_cast<void**>(reinterpret_cast<uint8_t*>(self) + subObjOff);
            if (!thisPtr) return 0;  // sub-object absent on this entity — degrade this invocation
        }
    }

    uint64_t g[kMaxGpArgs] = { 0, 0, 0, 0, 0, 0, 0, 0, 0 };
    // The core hands us f64 across the ABI (JS numbers are doubles); narrowing to the engine's
    // 32-bit `float` happens HERE once, before the selected backend assigns registers/stack slots.
    float    f[kMaxFpArgs] = { 0, 0, 0, 0, 0, 0, 0, 0 };

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
    for (int i = 0; i < fpCount; i++) f[i] = static_cast<float>(fp[i]);   // explicit f64 -> f32 narrow

    s2callabi::InvokeRequest request{};
    request.fn = fn;
    request.hasReceiver = hasReceiver;
    request.receiver = reinterpret_cast<uint64_t>(thisPtr);
    request.argClasses = argClass;
    request.argCount = argCount;
    request.gp = g;
    request.gpCount = gpCount;
    request.fp = f;
    request.fpCount = fpCount;
    request.retKind = retKind == kRetFloat ? s2callabi::kRetF32 : s2callabi::kRetU64;
    uint64_t r = 0;
    if (!s2callabi::Invoke(request, &r)) return 0;

    if (retOut) {
        switch (retKind) {
            case kRetVoid:   break;                                        // *retOut stays 0
            case kRetBool:   *retOut = (r & 0xff) ? 1 : 0; break;           // al
            case kRetInt:    *retOut = static_cast<uint32_t>(r); break;      // eax; core reads as i32
            case kRetFloat:   *retOut = r; break;                            // low 32 bits from xmm0
            case kRetEntity: *retOut = EntityHandleFromPtr(reinterpret_cast<void*>(r)); break;
            default:         break;
        }
    }
    return 1;
}
