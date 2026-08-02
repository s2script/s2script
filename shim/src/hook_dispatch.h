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
};

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

// The collapse contract: Handled(2) or Stop(3) suppresses the original call; Continue(0) and
// Changed(1) do not. An out-of-range value does NOT suppress — garbage from a handler must never
// silently cancel engine behaviour (ARCHITECTURE.md maps out-of-range to Continue).
bool S2Hook_Suppresses(int hookResult);

// The number of hooks a build may install. Bounded so the latch array is fixed-size and lock-free.
enum { S2_HOOK_MAX = 64 };

#endif
