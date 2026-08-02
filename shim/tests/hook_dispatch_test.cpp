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
    // The check above only ever exercises id 0 — a mutation that hardcodes S2Hook_ShapeName to
    // always return kShapes[0] would pass it. Round-trip the OTHER shape too.
    CHECK(std::strcmp(S2Hook_ShapeName(S2_HOOK_SHAPE_THIS_F32_I32_I32_I32),
                       "this_f32_i32_i32_i32") == 0,
          "the second shape id round-trips back to its own name, not the first shape's");
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

static void test_bypass_latch_rejects_out_of_range_ids() {
    // The hook id crosses in from OUTSIDE this TU (a gamedata-declared index), so it is exactly the
    // input this code cannot assume is in range. S2_HOOK_MAX exists so the latch array is a FIXED
    // size; only the bounds guard in each function keeps an out-of-range id from reading or writing
    // past the end of it. Drive negative, exactly-at-the-boundary, and well-past-the-boundary ids
    // through BOTH functions so deleting either guard is a provable regression — deterministically,
    // via the sentinel bytes that flank the latch storage, NOT via a sanitizer (a plain array next
    // to another global cannot be proven this way: two separate globals have unspecified relative
    // address order, so an off-the-end access might land anywhere, including nowhere observable).

    // --- BypassArm: a removed/broken guard WRITES past the array, flipping a sentinel it must
    // never touch. Start both sentinels clear; if the guard holds, they stay clear. ---
    S2Hook_DebugSetSentinels(false, false);
    S2Hook_BypassArm(-1);
    S2Hook_BypassArm(S2_HOOK_MAX);
    S2Hook_BypassArm(S2_HOOK_MAX + 1000);
    CHECK(!S2Hook_DebugGuardLo(), "an out-of-range BypassArm(-1) left the low sentinel clear");
    CHECK(!S2Hook_DebugGuardHi(),
          "an out-of-range BypassArm(S2_HOOK_MAX) left the high sentinel clear");

    // --- BypassTake: a removed/broken guard READS (and then clears) past the array. A "sentinel
    // stays clear" check can't distinguish this from a correct guard, since neither one ever writes
    // a clear sentinel back to anything but clear — so PLANT both sentinels SET first. A correct
    // guard rejects by id alone and never looks at the sentinel; a broken one reads SET, wrongly
    // reports the hook as armed, and clears it on the way out. Both effects are observable with a
    // plain CHECK. ---
    S2Hook_DebugSetSentinels(true, true);
    CHECK(!S2Hook_BypassTake(-1),
          "BypassTake rejects a negative hook id even though the sentinel it would alias is SET");
    CHECK(!S2Hook_BypassTake(S2_HOOK_MAX),
          "BypassTake rejects hook id == S2_HOOK_MAX even though the sentinel it would alias is SET");
    CHECK(!S2Hook_BypassTake(S2_HOOK_MAX + 1000), "BypassTake rejects a hook id far past S2_HOOK_MAX");
    CHECK(S2Hook_DebugGuardLo(), "and left the low sentinel UNTOUCHED — a guarded take never reads it");
    CHECK(S2Hook_DebugGuardHi(), "and left the high sentinel UNTOUCHED — a guarded take never reads it");
    S2Hook_DebugSetSentinels(false, false);  // leave the debug state clean for anything run after
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
    test_bypass_latch_rejects_out_of_range_ids();
    test_collapse_contract();
    test_ops_are_injected_not_linked();
    if (g_fail) { std::cerr << g_fail << " check(s) FAILED\n"; return 1; }
    std::cout << "hook_dispatch_test: all checks passed\n";
    return 0;
}
