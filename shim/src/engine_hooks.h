#pragma once
#include <cstdint>

// Declarative inbound hooks — the ENGINE half: the compile-time thunks, detour installation, and
// the block-scoped arg view. Engine-generic: nothing here names a game class, field, or function;
// the address arrives already resolved and validated by S2_EngineCallResolve, and the shape arrives
// as an id from the closed vocabulary in hook_dispatch.h.
//
// LAZY INSTALL. Configure one checked KHook Function on the first subscribe. Native bindings
// remain resident across script reload. Terminal inventory removes them before ResetAll.
extern "C" {

// Accept a validated target: 0 for accepted (possibly Pending), -1 with a bounded named reason.
// First real callback observation proves activation; accepted registration never polls for it.
int  S2_HookInstall(int hookId, int shape, int64_t addr, char* reasonOut, int reasonCap);

// Arm this hook's bypass latch. Core calls it immediately before invoking the `bypassWith` call
// descriptor, so our own outbound call does not fire our own hook (SourceMod parity).
void S2_HookArmBypass(int hookId);

// Disarm it again. The pair to S2_HookArmBypass, and NOT redundant with the thunk's take: the latch
// is only consumed if the outbound call actually reaches the hooked function. An invoke that returns
// early — a degraded descriptor, a stale receiver, an unresolved `via` — leaves it armed, and the
// NEXT genuine engine-driven invocation is then silently swallowed. That is spec §10's "clear it on
// both paths": core arms immediately before the invoke and disarms immediately after, so the armed
// window is exactly the call and nothing else. Core cannot do this alone — the latch lives here and
// only the thunk's take can clear it otherwise. Out-of-range ids are a silent no-op.
void S2_HookDisarmBypass(int hookId);

// Forget metadata only after checked terminal removal completes. Premature calls are a no-op;
// active callbacks and their typed capsules must remain alive through Recall and post.
void S2_HookResetAll(void);

// Block-scoped arg view accessors. `idx` is the descriptor's positional param index; every one is
// bounds-checked against the installed shape, so a stale generated binding cannot read off the
// frame. Return 0 on success, -1 on a bad index or a null view.
int  S2_HookReadF32 (void* argView, int idx, float* out);
int  S2_HookReadI32 (void* argView, int idx, int32_t* out);
// Text params (see the kParamStr note in engine_hooks.cpp): copies the view's NUL-terminated
// copy into `out`, bounded by `cap`. -1 on a dead view, bad index, or a non-text param.
int  S2_HookReadStr (void* argView, int idx, char* out, int cap);
int  S2_HookWriteF32(void* argView, int idx, float value);
int  S2_HookWriteI32(void* argView, int idx, int32_t value);

// The detour's `this`, as a packed CEntityHandle, when the descriptor surfaces it. -1 when it does
// not. NO RAW POINTER LEAVES THIS TU — core turns the handle into a books-gated EntityRef.
int  S2_HookReceiverHandle(void* argView, uint32_t* outHandle);

// Read a u16 at `q[qslot] + offset`. The pointer stays in the shim; core supplies a schema offset
// and never sees the address. -1 if the view is dead, the slot is empty, or the pointer is null.
int  S2_HookReadU16AtQ(void* argView, int qslot, int offset, uint16_t* out);

// Does the live entity at (index, serial) have `*(entity + offset) == this`? 1 yes / 0 no.
// Books-first resolve; the pointer compare is the pickup-gate ItemServices → pawn hop.
int  S2_HookSelfMatchesField(void* argView, int index, int serial, int offset);

}  // extern "C"

// Internal terminal inventory; these do not change the public engine-ops C ABI.
struct S2HookTerminalPermit;
bool S2EngineHooksCanUnloadSync(const S2HookTerminalPermit& permit);
bool S2EngineHooksUnloadSync(const S2HookTerminalPermit& permit);
bool S2EngineHooksRemovalComplete();
