#include "hook_dispatch.h"
#include <cstddef>
#include <cstring>

namespace {
struct ShapeEntry { int id; const char* name; };
const ShapeEntry kShapes[] = {
    { S2_HOOK_SHAPE_THIS_VOID,            "this_void" },
    { S2_HOOK_SHAPE_THIS_F32_I32_I32_I32, "this_f32_i32_i32_i32" },
    { S2_HOOK_SHAPE_THIS_F32_I32_I64_I64, "this_f32_i32_i64_i64" },
    { S2_HOOK_SHAPE_THIS_I64_I32_I64,     "this_i64_i32_i64" },
    { S2_HOOK_SHAPE_THIS_I64_I64_I64,     "this_i64_i64_i64" },
};
S2HookOps g_ops{};

// The latch storage, sentinel-guarded on both sides. A removed/broken bounds check used to be
// invisible without a sanitizer: g_bypass was a bare array sitting next to the SEPARATE g_ops
// global, and two independent globals have unspecified relative address order, so an off-the-end
// access could land anywhere in .bss (or nowhere observable at all) depending on how the linker
// happened to place them. C++ guarantees struct MEMBERS keep declaration order (same access
// specifier ⇒ increasing addresses), and on any ABI where `alignof(bool) == 1` — x86-64 SysV, which
// is what we ship — no padding is introduced between them, so slots[-1] and slots[S2_HOOK_MAX] are
// GUARANTEED to alias guardLo/guardHi exactly and nothing else. A plain CHECK on those two bytes
// then proves the guard, no sanitizer required.
//
// The alignment half is an ABI fact, not an ISO C++ mandate, so it is ASSERTED rather than assumed:
// the static_asserts below re-prove the layout for THIS build. Without them a later member
// insertion (or a port to an ABI that pads bools) would silently move the sentinels off the array's
// edges and the whole test would keep passing while proving nothing — the self-degrading guard that
// this sentinel scheme was introduced to eliminate.
struct BypassState {
    bool guardLo = false;
    bool slots[S2_HOOK_MAX] = { false };
    bool guardHi = false;
};
static_assert(sizeof(BypassState) == S2_HOOK_MAX + 2,
              "BypassState must be exactly guardLo + slots + guardHi with no padding — otherwise "
              "an off-the-end slots[] access does not land on a sentinel and hook_dispatch_test's "
              "bounds-guard proof silently proves nothing");
static_assert(offsetof(BypassState, guardLo) == 0,
              "guardLo must sit immediately BELOW slots[0], so slots[-1] aliases it exactly");
static_assert(offsetof(BypassState, slots) == 1,
              "slots must start one byte above guardLo — a gap here is an unwatched byte");
static_assert(offsetof(BypassState, guardHi) == 1 + S2_HOOK_MAX,
              "guardHi must sit immediately ABOVE slots[S2_HOOK_MAX-1], so slots[S2_HOOK_MAX] "
              "aliases it exactly");
BypassState g_bypass;
}  // namespace

int S2Hook_ShapeFromName(const char* name) {
    if (!name || !name[0]) return -1;
    for (const ShapeEntry& e : kShapes)
        if (std::strcmp(e.name, name) == 0) return e.id;
    return -1;
}

const char* S2Hook_ShapeName(int shape) {
    for (const ShapeEntry& e : kShapes)
        if (e.id == shape) return e.name;
    return nullptr;
}

void S2Hook_SetOps(const S2HookOps& ops) { g_ops = ops; }

int S2Hook_Dispatch(int hookId, void* argView) {
    if (!g_ops.dispatch) return 0;                 // Continue — never crash on a stale detour
    return g_ops.dispatch(hookId, argView);
}

int S2Hook_DispatchPost(int hookId, void* argView, int skipped) {
    if (!g_ops.dispatch_post) return 0;
    return g_ops.dispatch_post(hookId, argView, skipped);
}

void S2Hook_BypassArm(int hookId) {
    if (hookId >= 0 && hookId < S2_HOOK_MAX) g_bypass.slots[hookId] = true;
}

bool S2Hook_BypassTake(int hookId) {
    if (hookId < 0 || hookId >= S2_HOOK_MAX) return false;
    const bool was = g_bypass.slots[hookId];
    g_bypass.slots[hookId] = false;                // one-shot: a stuck latch kills the hook silently
    return was;
}

// Only `slots` — deliberately NOT the sentinels. They are not latch state; a reset that scrubbed
// them would also scrub the evidence of an out-of-bounds write that happened before it.
void S2Hook_BypassResetAll() {
    for (bool& s : g_bypass.slots) s = false;
}

void S2Hook_DebugSetSentinels(bool lo, bool hi) { g_bypass.guardLo = lo; g_bypass.guardHi = hi; }
bool S2Hook_DebugGuardLo() { return g_bypass.guardLo; }
bool S2Hook_DebugGuardHi() { return g_bypass.guardHi; }

bool S2Hook_Suppresses(int hookResult) { return hookResult == 2 || hookResult == 3; }

int32_t S2Hook_MostRestrictiveAcquire(int32_t first, int32_t second) {
    return first != 0 ? first : second;
}
