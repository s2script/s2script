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
// primitive into a dead frame. The selected hook ABI backend therefore publishes its view for
// exactly the duration of dispatch, and every accessor requires the caller's pointer to BE that view.
// The shim's usual block-scoped-static idiom (s_currentDamageInfo & co, nulled after dispatch)
// solves the same problem for objects the shim owns; this is that idiom applied to a frame pointer
// the caller holds.
//
// ENGINE-GENERIC: nothing here names a game class, field, or function. The address arrives already
// resolved and validated; the shape arrives as an id.
#include "engine_hooks.h"

#include "detour.h"
#include "engine_calls.h"   // S2_EntityHandleFromPtr — the books-first receiver -> CEntityHandle pack
#include "call_validate.h"   // the arg-width check: does the SHAPE match the callee's machine code?
#include "hook_dispatch.h"
#include "hook_abi.h"

#include <cstddef>
#include <cstdio>

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
// ShapeInfo and its fit-to-view proof live with the selected ABI backend. Keeping one table is
// critical: the thunk that populates a slot and the accessor that reads it must share metadata.
constexpr int kViewI64Slots = s2hookabi::kViewI64Slots;
using ArgView = s2hookabi::ArgView;

int Fail(char* out, int cap, const char* reason) {
    if (out && cap > 0) std::snprintf(out, static_cast<size_t>(cap), "%s", reason);
    return -1;
}

// The patch window used to be duplicated here as a constant (14) and checked at both ends before
// calling s2detour. That under-covered the read: s2detour steals WHOLE INSTRUCTIONS until it has
// enough bytes, so it routinely disassembles past 14 — 18 bytes for TerminateRound — which is
// exactly the read-into-unmapped-memory the guard existed to prevent.
//
// The probe is now passed INTO s2detour, which consults it for every instruction before decoding it.
// The guard is exact instead of approximate, there is no width constant to keep in sync with
// detour.cpp, and because it is injected it is drivable from shim/tests/detour_reloc_test.cpp.

// The gate every accessor shares: a non-null view that IS the live one. Returns null otherwise, so
// a stale or forged pointer fails by -1 and never reaches v->shape.
ArgView* LiveViewOf(void* argView) {
    return s2hookabi::LiveViewOf(argView);
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
    // hde64_disasm decodes it happily, the install succeeds, and a stretch of unrelated engine code
    // becomes a jump into our thunk. Engine corruption reported as success.
    //
    // The cheap entry-point check stays here so a plainly bogus address is refused with THIS
    // function's vocabulary; the per-instruction coverage of everything s2detour goes on to read is
    // the probe handed to Install below.
    const uintptr_t target = static_cast<uintptr_t>(addr);
    if (!S2_AddressIsExecutable(reinterpret_cast<const void*>(target)))
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

    // ARG-WIDTH: does the SHAPE agree with the machine code we are about to detour? A shape that
    // declares a parameter narrower than the callee uses truncates it — on TerminateRound that was a
    // pointer, and the resulting SIGSEGV landed ~374KB away in an unrelated function. Checked HERE,
    // at the moment we commit to patching, and refused by name like every other install failure.
    //
    // The selected backend supplies both flattened integer widths and its physical register map.
    // SysV uses independent class streams; Microsoft x64's map depends on author position.
    char argWidthNote[160] = "arg-width: not run";
    {
        const s2hookabi::ValidatorSpec abi = s2hookabi::ValidatorFor(shape);
        const unsigned char *text = nullptr, *lo = nullptr, *hi = nullptr;
        std::size_t textSize = 0;
        if (abi.wide &&
            S2_ModuleViewForAddress(reinterpret_cast<const void*>(target), &text, &textSize, &lo, &hi)) {
            s2validate::ModuleView mv;
            mv.text = text; mv.textSize = textSize; mv.lo = lo; mv.hi = hi;
            if (s2validate::ArgWidths(abi.wide, abi.slots, abi.registerMap, mv,
                                      reinterpret_cast<const void*>(target),
                                      argWidthNote, static_cast<int>(sizeof argWidthNote)) != 0)
                return Fail(reasonOut, reasonCap, argWidthNote);
        }
        // No ABI row or no module view: degrade to UNCHECKED rather than refuse. The caller's own
        // range check already passed, so this is a defensive miss, not evidence of a bad target.
    }

    void* thunk = s2hookabi::ThunkFor(shape, hookId);
    if (!thunk) {
        char buf[128];
        std::snprintf(buf, sizeof buf, "no compiled thunk for shape '%s'", shapeName);
        return Fail(reasonOut, reasonCap, buf);
    }

    // s2detour touches NO memory on failure, so a refusal here is always a clean degrade. Its reason
    // is surfaced verbatim rather than flattened into one message: "short branch in stolen prologue"
    // and "prologue leaves the target's executable range (stale gamedata?)" call for completely
    // different responses from whoever reads the log, and the old single string said neither.
    void* orig = nullptr;
    const s2detour::InstallResult inst =
        s2detour::Install(reinterpret_cast<void*>(static_cast<uintptr_t>(addr)), thunk, &orig,
                          &S2_AddressIsExecutable);
    if (!inst.ok) return Fail(reasonOut, reasonCap, inst.reason ? inst.reason : "detour install refused");
    s2hookabi::SetOriginal(hookId, orig);

    // On SUCCESS the reason buffer carries an informational note instead of staying empty: which
    // tier ran is the difference between a 5-byte and a 14-byte patch, and nobody should have to
    // guess which one a live server took when reading a crash dump. Core logs it. This TU has no
    // logger of its own on purpose — it returns strings and lets core decide what to do with them.
    if (reasonOut && reasonCap > 0)
        std::snprintf(reasonOut, static_cast<size_t>(reasonCap), "%s jump, stole %d byte(s); %s",
                      inst.usedNearJump ? "near E9" : "far FF25", inst.stolen, argWidthNote);

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
    s2hookabi::Reset();       // no original trampoline or stack view survives teardown
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
    const s2hookabi::ShapeInfo si = s2hookabi::InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != s2hookabi::kParamF32) return -1;
    *out = v->f[si.params[idx].slot];
    return 0;
}

int S2_HookReadI32(void* argView, int idx, int32_t* out) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !out) return -1;
    const s2hookabi::ShapeInfo si = s2hookabi::InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != s2hookabi::kParamI32) return -1;
    *out = v->i[si.params[idx].slot];
    return 0;
}

int S2_HookWriteF32(void* argView, int idx, float value) {
    ArgView* v = LiveViewOf(argView);
    if (!v) return -1;
    const s2hookabi::ShapeInfo si = s2hookabi::InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != s2hookabi::kParamF32) return -1;
    v->f[si.params[idx].slot] = value;
    return 0;
}

int S2_HookWriteI32(void* argView, int idx, int32_t value) {
    ArgView* v = LiveViewOf(argView);
    if (!v) return -1;
    const s2hookabi::ShapeInfo si = s2hookabi::InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != s2hookabi::kParamI32) return -1;
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

int S2_HookReadU16AtQ(void* argView, int qslot, int offset, uint16_t* out) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !out) return -1;
    if (qslot < 0 || qslot >= kViewI64Slots || offset < 0) return -1;
    const int64_t p = v->q[qslot];
    if (p == 0) return -1;
    *out = *reinterpret_cast<const uint16_t*>(static_cast<uintptr_t>(p) + static_cast<uintptr_t>(offset));
    return 0;
}

int S2_HookSelfMatchesField(void* argView, int index, int serial, int offset) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !v->self || offset < 0) return 0;
    void* ent = S2_ResolveEntity(index, serial);
    if (!ent) return 0;
    void* field = *reinterpret_cast<void**>(static_cast<char*>(ent) + offset);
    return field == v->self ? 1 : 0;
}
