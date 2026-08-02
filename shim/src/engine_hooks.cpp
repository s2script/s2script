// Declarative inbound hooks — the ENGINE half. See engine_hooks.h for the C-ABI contract and
// hook_dispatch.h for the engine-free policy this TU drives (shape vocabulary, bypass latch,
// collapse rule).
//
// HOW A THUNK KNOWS WHICH HOOK IT IS. A detour handler receives no hook id: the engine calls it
// with the callee's arguments and nothing else. So the id is baked in at COMPILE time — each shape
// is a function TEMPLATE parameterised on the id, instantiated once per hook slot, and the table
// below holds all S2_HOOK_MAX addresses. Install hands s2detour the slot's OWN thunk, so inside the
// handler `Id` is a constant, not a lookup. Cost: S2_HOOK_MAX x shapes tiny functions (the whole
// table is a few KB of .text); benefit: no return-address arithmetic, no per-call search, and no
// ABI risk whatsoever. The alternative — one thunk per shape recovering the id from the return
// address — is fragile and platform-specific, and was rejected for exactly that reason.
//
// THE THUNK BODY ORDER IS THE CONTRACT (identical in every shape):
//   1. S2Hook_BypassTake(Id)  -> our own outbound call armed the latch: pass straight through to the
//      original WITHOUT firing the hook. This is SourceMod's g_pIgnoreTerminateDetour, and it is
//      what keeps a hook from ever firing while core holds the V8 isolate borrow.
//   2. Build the ArgView over THIS FRAME's own stack args. It lives on the thunk's frame and dies
//      with it, so nothing it points at can outlive the dispatch.
//   3. S2Hook_Dispatch(Id, &view) -> the collapsed HookResult.
//   4. S2Hook_Suppresses(r) -> Handled/Stop suppress the engine call ENTIRELY (return without
//      calling the original).
//   5. Otherwise call the original with the values read back OUT of the view, so a handler's writes
//      to `mutable` params actually reach the engine.
//
// DEGRADE, NEVER CRASH. Every S2_HookInstall failure path disables exactly ONE hook with a NAMED
// reason (out-of-range id, unknown shape, an address outside any module's executable range, an
// address another hook already owns, no compiled thunk for the shape, a prologue s2detour refuses)
// and leaves the framework running. A hook that never installs is a hook whose subscribers never
// fire — never a patched byte, never a wild jump.
//
// THE ARG VIEW IS LIVENESS-GATED, not just shape-gated. The view is a pointer to a THUNK'S STACK
// FRAME that crosses the C ABI, so "is this the currently-live view?" cannot be answered from the
// pointer's own contents — a retained pointer into a since-reused frame would read a plausible
// `shape` out of whatever now occupies those bytes and turn S2_HookWriteI32 into a 4-byte write
// primitive into a dead frame. Each thunk therefore publishes its view in g_activeView for exactly
// the duration of its dispatch, and every accessor requires the caller's pointer to BE that view.
// The shim's usual block-scoped-static idiom (s_currentDamageInfo & co, nulled after dispatch)
// solves the same problem for objects the shim owns; this is that idiom applied to a frame pointer
// the caller holds.
//
// ENGINE-GENERIC: nothing here names a game class, field, or function. The address arrives already
// resolved and validated; the shape arrives as an id.
#include "engine_hooks.h"

#include "detour.h"
#include "engine_calls.h"   // S2_EntityHandleFromPtr — the books-first receiver -> CEntityHandle pack
#include "hook_dispatch.h"

#include <array>
#include <cstddef>
#include <cstdio>
#include <utility>

namespace {

// ---------------------------------------------------------------------------
// The installed-hook table. `orig` is s2detour's trampoline to the original function.
// ---------------------------------------------------------------------------
struct Installed {
    bool    used  = false;
    int     shape = -1;
    int64_t addr  = 0;
    void*   orig  = nullptr;
};
Installed g_hooks[S2_HOOK_MAX];

// ---------------------------------------------------------------------------
// The block-scoped arg view.
//
// A param is addressed by its POSITIONAL index in the descriptor (0 = the first declared param;
// `this` is never index 0 — the receiver has its own accessor). The shape decides both how many
// params exist and what CLASS each one is, so a stale generated binding asking for an i32 where the
// shape has an f32 fails by -1 instead of reinterpreting the bits.
// ---------------------------------------------------------------------------
enum ParamClass : unsigned char { kParamF32 = 0, kParamI32 = 1 };
struct ParamSlot { ParamClass cls; unsigned char slot; };

// Sized to the WIDEST shape in the vocabulary; each shape's table is static_asserted to fit, so
// adding a shape that needs more storage fails to COMPILE rather than writing past the view.
constexpr int kViewF32Slots = 1;
constexpr int kViewI32Slots = 3;

struct ArgView {
    int     hookId = -1;
    int     shape  = -1;
    void*   self   = nullptr;
    float   f[kViewF32Slots] = {};
    int32_t i[kViewI32Slots] = {};
};

// THE LIVENESS TOKEN. The one view a thunk is currently dispatching, or null between dispatches. An
// accessor accepts ONLY this exact pointer, so a view retained past its dispatch fails by -1 instead
// of reading (or writing) a stack frame that has since been reused. It is a save/restore, not a
// set/clear: a handler can make the engine call another hooked function, and the inner dispatch must
// hand the outer one its view back — clearing to null would silently disable the outer frame's
// accessors for the rest of its dispatch. Single-threaded (the game thread drives every detour), so
// a plain pointer is the whole mechanism.
const void* g_activeView = nullptr;

// S2_HOOK_SHAPE_THIS_VOID has no params at all; the array exists only because a zero-length array is
// not standard C++, and InfoFor() reports count 0 so no index can ever reach it.
constexpr ParamSlot kParamsThisVoid[] = { { kParamF32, 0 } };
// S2_HOOK_SHAPE_THIS_F32_I32_I32_I32 — void(void* self, float, int, int, int).
constexpr ParamSlot kParamsThisF32I32I32I32[] = {
    { kParamF32, 0 },
    { kParamI32, 0 },
    { kParamI32, 1 },
    { kParamI32, 2 },
};

struct ShapeInfo { const ParamSlot* params; int count; };

// An unknown shape yields count 0, so every accessor index fails: an out-of-vocabulary shape can
// never be read as if it were shape 0, whose wrong ABI is exactly what hook_dispatch.h warns about.
// constexpr so the fits-the-view proof below can be structural rather than hand-maintained.
constexpr ShapeInfo InfoFor(int shape) {
    switch (shape) {
        case S2_HOOK_SHAPE_THIS_VOID:            return { kParamsThisVoid, 0 };
        case S2_HOOK_SHAPE_THIS_F32_I32_I32_I32: return { kParamsThisF32I32I32I32, 4 };
        default:                                 return { nullptr, 0 };
    }
}

// EVERY shape's params fit the view — checked over the whole id space InfoFor can describe, not per
// shape by hand. A new shape is only reachable once it is added to InfoFor, and the moment it is,
// this assert covers it; there is no separate line for its author to forget. That is what lets the
// accessors index v->f[]/v->i[] with the table's slot and no runtime bound: the bound is proven
// here, for all shapes, at compile time.
constexpr bool ShapeFitsView(int shape) {
    const ShapeInfo si = InfoFor(shape);
    for (int k = 0; k < si.count; k++) {
        const int limit = (si.params[k].cls == kParamF32) ? kViewF32Slots : kViewI32Slots;
        if (static_cast<int>(si.params[k].slot) >= limit) return false;
    }
    return true;
}
constexpr bool AllShapesFitView(int upTo) {
    for (int s = 0; s <= upTo; s++)
        if (!ShapeFitsView(s)) return false;
    return true;
}
// 255 is a deliberate over-scan of the shape id space: a shape InfoFor does not know yields count 0
// and passes trivially, so the bound costs nothing and cannot be outgrown by a new enumerator.
static_assert(AllShapesFitView(255),
              "a shape's params do not fit the ArgView — widen kViewF32Slots/kViewI32Slots");

// ---------------------------------------------------------------------------
// The thunks: one instantiation per (shape, hook slot).
// ---------------------------------------------------------------------------
using ThisVoidFn           = void (*)(void*);
using ThisF32I32I32I32Fn   = void (*)(void*, float, int32_t, int32_t, int32_t);

// `orig` is null only in the window between s2detour patching the prologue and publishing the
// trampoline — unreachable on the single-threaded game thread, but guarded rather than assumed.
template <int Id>
void Thunk_ThisVoid(void* self) {
    const ThisVoidFn orig = reinterpret_cast<ThisVoidFn>(g_hooks[Id].orig);
    if (S2Hook_BypassTake(Id)) { if (orig) orig(self); return; }

    ArgView v;
    v.hookId = Id;
    v.shape  = S2_HOOK_SHAPE_THIS_VOID;
    v.self   = self;

    const void* const prevView = g_activeView;
    g_activeView = &v;
    const int r = S2Hook_Dispatch(Id, &v);
    g_activeView = prevView;   // the view dies with this frame; nothing may reach it after here

    if (S2Hook_Suppresses(r)) return;
    if (orig) orig(self);
}

template <int Id>
void Thunk_ThisF32I32I32I32(void* self, float a0, int32_t a1, int32_t a2, int32_t a3) {
    const ThisF32I32I32I32Fn orig = reinterpret_cast<ThisF32I32I32I32Fn>(g_hooks[Id].orig);
    if (S2Hook_BypassTake(Id)) { if (orig) orig(self, a0, a1, a2, a3); return; }

    ArgView v;
    v.hookId = Id;
    v.shape  = S2_HOOK_SHAPE_THIS_F32_I32_I32_I32;
    v.self   = self;
    v.f[0]   = a0;
    v.i[0]   = a1;
    v.i[1]   = a2;
    v.i[2]   = a3;

    const void* const prevView = g_activeView;
    g_activeView = &v;
    const int r = S2Hook_Dispatch(Id, &v);
    g_activeView = prevView;   // the view dies with this frame; nothing may reach it after here

    if (S2Hook_Suppresses(r)) return;
    // Read back OUT of the view — a handler's writes to `mutable` params reach the engine here.
    if (orig) orig(self, v.f[0], v.i[0], v.i[1], v.i[2]);
}

// The tables. One entry per hook slot, materialised at COMPILE time by expanding an index_sequence
// over the templates above — so every slot's thunk address is a link-time constant and the id it
// carries is an immediate, not a lookup. (A template template parameter cannot bind a FUNCTION
// template, which is why this is two small builders rather than one generic one.)
constexpr std::size_t kHookSlots = static_cast<std::size_t>(S2_HOOK_MAX);

template <std::size_t... Is>
constexpr std::array<ThisVoidFn, sizeof...(Is)> MakeThisVoidTable(std::index_sequence<Is...>) {
    return {{ &Thunk_ThisVoid<static_cast<int>(Is)>... }};
}
template <std::size_t... Is>
constexpr std::array<ThisF32I32I32I32Fn, sizeof...(Is)>
MakeThisF32I32I32I32Table(std::index_sequence<Is...>) {
    return {{ &Thunk_ThisF32I32I32I32<static_cast<int>(Is)>... }};
}

constexpr auto kThisVoidThunks = MakeThisVoidTable(std::make_index_sequence<kHookSlots>{});
constexpr auto kThisF32I32I32I32Thunks =
    MakeThisF32I32I32I32Table(std::make_index_sequence<kHookSlots>{});
static_assert(kThisVoidThunks.size() == kHookSlots, "thunk table must cover every hook slot");
static_assert(kThisF32I32I32I32Thunks.size() == kHookSlots, "thunk table must cover every hook slot");

// The slot's own thunk for `shape`, or null when the shape has no compiled thunk (a NAMED install
// failure — the closed vocabulary and the compiled set must never drift apart silently).
void* ThunkFor(int shape, int hookId) {
    if (hookId < 0 || hookId >= S2_HOOK_MAX) return nullptr;
    switch (shape) {
        case S2_HOOK_SHAPE_THIS_VOID:
            return reinterpret_cast<void*>(kThisVoidThunks[static_cast<std::size_t>(hookId)]);
        case S2_HOOK_SHAPE_THIS_F32_I32_I32_I32:
            return reinterpret_cast<void*>(kThisF32I32I32I32Thunks[static_cast<std::size_t>(hookId)]);
        default:
            return nullptr;
    }
}

int Fail(char* out, int cap, const char* reason) {
    if (out && cap > 0) std::snprintf(out, static_cast<size_t>(cap), "%s", reason);
    return -1;
}

// s2detour overwrites the prologue with a 14-byte absolute jump (FF 25 <disp32> <8-byte target>;
// shim/src/detour.cpp's kAbsJmp). Both ends of that window must be provably executable before we let
// hde64_disasm read a single byte at the target — an unmapped address SEGVs inside the disassembler,
// and a mapped-but-wrong one decodes fine and gets patched.
constexpr int kPatchWindow = 14;

ArgView* ViewOf(void* argView) { return static_cast<ArgView*>(argView); }

// The gate every accessor shares: a non-null view that IS the live one. Returns null otherwise, so
// a stale or forged pointer fails by -1 and never reaches v->shape.
ArgView* LiveViewOf(void* argView) {
    if (!argView || argView != g_activeView) return nullptr;
    return ViewOf(argView);
}

}  // namespace

// ---------------------------------------------------------------------------
// Install. Idempotent per hook id: core calls it on every subscribe and only the first patches.
// ---------------------------------------------------------------------------
int S2_HookInstall(int hookId, int shape, int64_t addr, char* reasonOut, int reasonCap) {
    if (reasonOut && reasonCap > 0) reasonOut[0] = '\0';

    if (hookId < 0 || hookId >= S2_HOOK_MAX)
        return Fail(reasonOut, reasonCap, "hook id out of range (this build installs at most 64 hooks)");

    const char* shapeName = S2Hook_ShapeName(shape);
    if (!shapeName) {
        char buf[96];
        std::snprintf(buf, sizeof buf, "unknown hook shape id %d", shape);
        return Fail(reasonOut, reasonCap, buf);
    }

    if (!addr) return Fail(reasonOut, reasonCap, "hook target address is null");

    // THE ONE INPUT THAT ACTUALLY GETS DISASSEMBLED AND OVERWRITTEN. Everything above validates
    // core's bookkeeping; this validates the bytes. `addr` arrives already resolved and .text-range
    // checked by S2_EngineCallResolve — but a hook is installed LATER, from a table that outlives the
    // resolve, and a stale offset after a CS2 update can resolve to a wrong-but-mapped address:
    // hde64_disasm decodes it happily, Install returns true, and 14 bytes of unrelated engine code
    // become a jump into our thunk. Engine corruption reported as success. An UNMAPPED address is
    // worse still — the SEGV happens inside hde64_disasm, before any of our own guards run. So the
    // whole patch window is proven executable HERE, at the patch site, before s2detour reads a byte.
    const uintptr_t target = static_cast<uintptr_t>(addr);
    if (!S2_AddressIsExecutable(reinterpret_cast<const void*>(target)) ||
        !S2_AddressIsExecutable(reinterpret_cast<const void*>(target + kPatchWindow - 1)))
        return Fail(reasonOut, reasonCap,
                    "hook target is outside any loaded module's executable range (stale gamedata?)");

    // Already installed for this id: the same (shape, address) is a success no-op — that IS the
    // idempotence core relies on. A DIFFERENT target on a live slot is refused, because installing
    // it would orphan the first detour's trampoline with no way to unpatch it before Unload.
    if (g_hooks[hookId].used) {
        if (g_hooks[hookId].shape == shape && g_hooks[hookId].addr == addr) return 0;
        return Fail(reasonOut, reasonCap, "hook id is already installed on a different target");
    }

    // Two hook ids on ONE address would double-patch: the second Install would steal a prologue that
    // is ALREADY our jump and relocate it into its trampoline, so the original would jump back into
    // the first thunk forever. Refuse by name instead.
    for (int i = 0; i < S2_HOOK_MAX; i++) {
        if (g_hooks[i].used && g_hooks[i].addr == addr)
            return Fail(reasonOut, reasonCap, "another hook id is already installed at this address");
    }

    void* thunk = ThunkFor(shape, hookId);
    if (!thunk) {
        char buf[128];
        std::snprintf(buf, sizeof buf, "no compiled thunk for shape '%s'", shapeName);
        return Fail(reasonOut, reasonCap, buf);
    }

    // s2detour refuses a relative/rip-relative or undecodable prologue and returns false WITHOUT
    // touching memory, so a failure here is always a clean degrade.
    void* orig = nullptr;
    if (!s2detour::Install(reinterpret_cast<void*>(static_cast<uintptr_t>(addr)), thunk, &orig))
        return Fail(reasonOut, reasonCap,
                    "detour install refused this prologue (relative/rip-relative or undecodable)");

    g_hooks[hookId].shape = shape;
    g_hooks[hookId].addr  = addr;
    g_hooks[hookId].orig  = orig;
    g_hooks[hookId].used  = true;   // published LAST: a thunk reads `orig`, so it must be set first
    return 0;
}

void S2_HookArmBypass(int hookId) {
    S2Hook_BypassArm(hookId);   // bounds-checked there; an out-of-range id is a silent no-op
}

// Take-and-discard: "clear the latch whether or not the call consumed it". Deliberately expressed as
// the same one-shot take the thunk performs rather than as a second way to write the slot, so there
// is exactly ONE operation that clears a latch.
void S2_HookDisarmBypass(int hookId) {
    (void)S2Hook_BypassTake(hookId);
}

// Forget every installed hook. One operation with s2detour::RemoveAll() — see engine_hooks.h.
void S2_HookResetAll(void) {
    for (int i = 0; i < S2_HOOK_MAX; i++) g_hooks[i] = Installed{};
    g_activeView = nullptr;   // no frame survives a teardown
    // ...and neither does a LATCH. Only the thunk clears one, so an armed-but-never-taken latch (an
    // outbound invoke that degraded before reaching the hooked function) survives unload — and slot
    // ids are reused on reload, so the next load's first genuine engine-driven call to that id would
    // be silently bypassed. Called from here rather than from the Unload site so it cannot be
    // forgotten: "forget every installed hook" is one operation, not two that must be kept adjacent.
    S2Hook_BypassResetAll();
}

// ---------------------------------------------------------------------------
// The arg-view accessors. Each one is LIVENESS-gated (the pointer must be the view currently being
// dispatched), then bounds- and class-checked against that view's shape.
// ---------------------------------------------------------------------------
int S2_HookReadF32(void* argView, int idx, float* out) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !out) return -1;
    const ShapeInfo si = InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != kParamF32) return -1;
    *out = v->f[si.params[idx].slot];
    return 0;
}

int S2_HookReadI32(void* argView, int idx, int32_t* out) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !out) return -1;
    const ShapeInfo si = InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != kParamI32) return -1;
    *out = v->i[si.params[idx].slot];
    return 0;
}

int S2_HookWriteF32(void* argView, int idx, float value) {
    ArgView* v = LiveViewOf(argView);
    if (!v) return -1;
    const ShapeInfo si = InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != kParamF32) return -1;
    v->f[si.params[idx].slot] = value;
    return 0;
}

int S2_HookWriteI32(void* argView, int idx, int32_t value) {
    ArgView* v = LiveViewOf(argView);
    if (!v) return -1;
    const ShapeInfo si = InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != kParamI32) return -1;
    v->i[si.params[idx].slot] = value;
    return 0;
}

// The receiver, as a packed CEntityHandle. -1 when the shape has no receiver, when it is null, or
// when the entity system's own books do not vouch for it — a detour `this` is frequently NOT an
// entity at all (a rules/services singleton), and that is a normal "no receiver to surface", not an
// error. The packing is engine_calls.cpp's books-FIRST walk (membership decided without reading a
// single byte of the pointer), so a non-entity `this` can never be dereferenced here.
int S2_HookReceiverHandle(void* argView, uint32_t* outHandle) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !outHandle) return -1;
    if (!v->self) return -1;
    const uint32_t h = S2_EntityHandleFromPtr(v->self);
    if (h == S2_ENTITY_HANDLE_NONE) return -1;
    *outHandle = h;
    return 0;
}
