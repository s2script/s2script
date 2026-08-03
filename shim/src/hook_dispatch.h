// THE ENGINE-FREE HALF of declarative inbound hooks: the closed shape vocabulary, the per-hook
// bypass latch, and the collapse contract.
//
// WHY ITS OWN TU. The thunks themselves cannot be compiled outside the game (they need the entity
// bridge to marshal a receiver), so any rule living beside them would be untestable — the same hole
// shim/src/defer_queue.cpp and shim/src/call_validate.cpp were extracted to close. Every outside
// contact is INJECTED via S2HookOps, so shim/tests/hook_dispatch_test.cpp drives the SHIPPED code.
//
// ENGINE-GENERIC: nothing here names a game, a class, or a function.
#ifndef S2SCRIPT_HOOK_DISPATCH_H
#define S2SCRIPT_HOOK_DISPATCH_H

// The CLOSED vocabulary of inbound thunk shapes. A shape is a concrete C++ signature compiled into
// the shim; gamedata selects one BY NAME. You cannot detour an arbitrary signature from data — a
// handler must have the callee's exact ABI — which is why SourceMod uses per-arity DETOUR_DECL_MEMBER
// macros and why this vocabulary exists. Adding a shape is a CORE CHANGE, deliberately: only adding
// a hook on an EXISTING shape is zero-core-diff.
enum S2HookShape {
    S2_HOOK_SHAPE_THIS_VOID            = 0,  // void(void* self)
    S2_HOOK_SHAPE_THIS_F32_I32_I32_I32 = 1,  // void(void* self, float, int, int, int)
    S2_HOOK_SHAPE_THIS_F32_I32_I64_I64 = 2,  // void(void* self, float, int, int64, int64)
};

// WIDTH IS NOT A DETAIL — IT IS THE WHOLE CONTRACT (learned from a live SEGV).
//
// An INBOUND hook must hand the original function back exactly what the engine passed it. A trailing
// parameter we do not understand is therefore NOT free to declare `int`: `int` does not mean "we do
// not care about this arg", it means "truncate whatever the engine put in that register to 32 bits".
//
// `CCSGameRules::TerminateRound` was declared `this_f32_i32_i32_i32`. Its third parameter arrives in
// rdx and the engine stores it with `mov %rdx,-0xe0(%rbp)` — a full 64-bit store, i.e. a POINTER. The
// thunk read `edx`, called the original with `edx` (zero-extending), the engine banked half a pointer,
// and something else dereferenced it much later: SIGSEGV at a stack address with its top 32 bits gone,
// ~374KB away from TerminateRound, which is why it looked like it came from nowhere.
//
// So: when in doubt about a parameter, relay it 64-bit and do not surface it to JS. Widening an
// opaque pass-through is always safe (SysV leaves the upper half of a 32-bit arg undefined, so
// copying the whole register back preserves whatever was actually there); narrowing never is.
//
// This is the opposite of an OUTBOUND `calls` descriptor, where WE choose the value and `0` is an
// honest null — which is exactly why the shipping `terminateRound()` call never broke.

// Shape name <-> id. An unknown name returns -1 and an out-of-range id returns nullptr: a typo must
// fail BY NAME, never default to shape 0, whose wrong ABI would corrupt the stack on first call.
int         S2Hook_ShapeFromName(const char* name);
const char* S2Hook_ShapeName(int shape);

// Everything this TU needs from outside, injected so the policy is testable.
struct S2HookOps {
    // Fan the hook out to JS and return the COLLAPSED HookResult. `argView` is opaque here; the
    // thunk that owns the stack frame gives it meaning.
    int (*dispatch)(int hookId, void* argView) = nullptr;
};
void S2Hook_SetOps(const S2HookOps& ops);

// Dispatch through the injected op. A null op returns 0 (Continue) rather than crashing: the engine
// can call through a detour installed by a load whose core is already gone.
int S2Hook_Dispatch(int hookId, void* argView);

// The bypass latch — SourceMod's g_pIgnoreTerminateDetour (extensions/cstrike/forwards.cpp:132),
// made per-hook. Our own outbound descriptor invoke ARMS it; the thunk TAKES it and passes straight
// through to the original without firing the hook.
//
// Two reasons it matters. It is SM's semantic: a plugin-issued call does not fire the hook. And it
// is what keeps a hook from ever firing while core holds the isolate borrow — which is the wall
// that makes pre-hooks undeliverable (see the gamedata-tiering spec §9.2 / D3).
void S2Hook_BypassArm(int hookId);
bool S2Hook_BypassTake(int hookId);   // true AND clears, iff it was armed

// Clear EVERY latch. Part of teardown, called by S2_HookResetAll (engine_hooks.cpp) so the two
// halves of "forget every hook" cannot drift apart.
//
// Not hypothetical: the latch is one-shot but only the THUNK clears it, and a latch that was armed
// and never taken (an outbound invoke that degraded before reaching the hooked function, on a build
// whose ops table has no disarm) outlives the hook it belonged to. Hook slot ids are reused across a
// reload — that is the whole point of the slot table surviving `drop_owner` — so a stale armed latch
// would be TAKEN by the first genuine engine-driven call to whatever hook inherits that id, and that
// dispatch would silently not fire. Clearing at teardown is what keeps a slot id from carrying state
// across the boundary that resets everything else about it.
void S2Hook_BypassResetAll();

// TEST-ONLY. BypassArm/BypassTake's bounds guard is proven in shim/tests/hook_dispatch_test.cpp
// against two sentinel bytes that flank the latch storage on purpose (see the BypassState comment
// in hook_dispatch.cpp), instead of against a sanitizer: two SEPARATE globals have unspecified
// relative address order, so an off-the-end access on a bare array sitting next to another global
// can land anywhere — including nowhere observable, which is exactly what let a removed guard slip
// through the first version of this test undetected with sanitizers off. Struct members, unlike
// separate globals, keep declaration order, so an off-by-one on either side of the array is
// GUARANTEED to land on one of these two bytes and nothing else. Not part of the production
// contract; no shipped caller uses these.
void S2Hook_DebugSetSentinels(bool lo, bool hi);
bool S2Hook_DebugGuardLo();
bool S2Hook_DebugGuardHi();

// The collapse contract: Handled(2) or Stop(3) suppresses the original call; Continue(0) and
// Changed(1) do not. An out-of-range value does NOT suppress — garbage from a handler must never
// silently cancel engine behaviour (ARCHITECTURE.md maps out-of-range to Continue).
bool S2Hook_Suppresses(int hookResult);

// The number of hooks a build may install. Bounded so the latch array is fixed-size and lock-free.
enum { S2_HOOK_MAX = 64 };

#endif
