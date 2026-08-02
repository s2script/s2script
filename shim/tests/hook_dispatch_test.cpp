// Unit test for the engine-free half of declarative inbound hooks (the shape table, the bypass
// latch, and the collapse contract). Self-contained: no engine, no gamedata, no CMake SDK paths.
#include "../src/hook_dispatch.h"
#include <cstring>
#include <iostream>

static int g_fail = 0;
#define CHECK(cond, msg)                                                   \
    do {                                                                   \
        if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; g_fail++; }  \
        else         { std::cout << "ok:   " << (msg) << "\n"; }            \
    } while (0)

static void test_shape_vocabulary_is_closed() {
    CHECK(S2Hook_ShapeFromName("this_void") == S2_HOOK_SHAPE_THIS_VOID, "this_void resolves");
    CHECK(S2Hook_ShapeFromName("this_f32_i32_i32_i32") == S2_HOOK_SHAPE_THIS_F32_I32_I32_I32,
          "this_f32_i32_i32_i32 resolves");
    // An unknown shape must be a NAMED failure, never a silent default to shape 0 — a typo would
    // otherwise install a detour with the wrong ABI, which corrupts the stack on first call.
    CHECK(S2Hook_ShapeFromName("this_i32") == -1, "an unknown shape is rejected");
    CHECK(S2Hook_ShapeFromName("") == -1, "an empty shape name is rejected");
    CHECK(std::strcmp(S2Hook_ShapeName(S2_HOOK_SHAPE_THIS_VOID), "this_void") == 0,
          "shape ids round-trip back to their names");
    CHECK(S2Hook_ShapeName(99) == nullptr, "an out-of-range shape id has no name");
}

static void test_bypass_latch_is_one_shot_and_per_hook() {
    // The latch is SourceMod's g_pIgnoreTerminateDetour: our own outbound call arms it, the thunk
    // takes it, and the hook does not fire for that one invocation.
    CHECK(!S2Hook_BypassTake(0), "a fresh latch is not set");
    S2Hook_BypassArm(0);
    CHECK(S2Hook_BypassTake(0), "an armed latch is taken");
    CHECK(!S2Hook_BypassTake(0), "and taking it CLEARS it — a stuck latch would silently kill the hook");
    S2Hook_BypassArm(0);
    CHECK(!S2Hook_BypassTake(1), "the latch is per-hook, not global");
    CHECK(S2Hook_BypassTake(0), "and hook 0's latch survived hook 1's take");
}

static void test_collapse_contract() {
    CHECK(!S2Hook_Suppresses(0), "Continue does not suppress");
    CHECK(!S2Hook_Suppresses(1), "Changed does not suppress — it writes params back and still calls");
    CHECK(S2Hook_Suppresses(2), "Handled suppresses the engine call");
    CHECK(S2Hook_Suppresses(3), "Stop suppresses the engine call");
    // An out-of-range collapse must NOT suppress: garbage from a handler must never silently
    // cancel engine behaviour (ARCHITECTURE.md — out-of-range maps to Continue).
    CHECK(!S2Hook_Suppresses(99), "an out-of-range result does not suppress");
    CHECK(!S2Hook_Suppresses(-1), "a negative result does not suppress");
}

static int g_dispatchCalls = 0;
static int g_lastHookId = -1;
static int fake_dispatch(int hookId, void*) { g_dispatchCalls++; g_lastHookId = hookId; return 0; }

static void test_ops_are_injected_not_linked() {
    S2HookOps ops{}; ops.dispatch = &fake_dispatch;
    S2Hook_SetOps(ops);
    g_dispatchCalls = 0;
    CHECK(S2Hook_Dispatch(7, nullptr) == 0, "dispatch forwards to the injected op");
    CHECK(g_dispatchCalls == 1 && g_lastHookId == 7, "and passes the hook id through");
    S2HookOps none{};
    S2Hook_SetOps(none);
    // A null op must be a no-suppress no-op, not a crash: core may not be initialised yet when the
    // engine calls through a detour installed by a previous load.
    CHECK(S2Hook_Dispatch(7, nullptr) == 0, "a null dispatch op degrades to Continue");
    // The return-value check above is degenerate: fake_dispatch ALSO returns 0, so an SetOps that
    // silently keeps the old dispatch pointer instead of clearing it would pass it too. Assert the
    // stale op was actually dropped and not re-invoked — a SetOps that only assigns when
    // ops.dispatch is non-null would leave a freed plugin's function pointer live across reload.
    CHECK(g_dispatchCalls == 1, "SetOps(none) actually clears the op — it is not re-invoked");
}

int main() {
    test_shape_vocabulary_is_closed();
    test_bypass_latch_is_one_shot_and_per_hook();
    test_collapse_contract();
    test_ops_are_injected_not_linked();
    if (g_fail) { std::cerr << g_fail << " check(s) FAILED\n"; return 1; }
    std::cout << "hook_dispatch_test: all checks passed\n";
    return 0;
}
