#include "hook_dispatch.h"
#include <cstring>

namespace {
struct ShapeEntry { int id; const char* name; };
const ShapeEntry kShapes[] = {
    { S2_HOOK_SHAPE_THIS_VOID,            "this_void" },
    { S2_HOOK_SHAPE_THIS_F32_I32_I32_I32, "this_f32_i32_i32_i32" },
};
S2HookOps g_ops{};
bool      g_bypass[S2_HOOK_MAX] = { false };
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

void S2Hook_BypassArm(int hookId) {
    if (hookId >= 0 && hookId < S2_HOOK_MAX) g_bypass[hookId] = true;
}

bool S2Hook_BypassTake(int hookId) {
    if (hookId < 0 || hookId >= S2_HOOK_MAX) return false;
    const bool was = g_bypass[hookId];
    g_bypass[hookId] = false;                      // one-shot: a stuck latch kills the hook silently
    return was;
}

bool S2Hook_Suppresses(int hookResult) { return hookResult == 2 || hookResult == 3; }
