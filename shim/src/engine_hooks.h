#pragma once
#include <cstdint>

// Declarative inbound hooks — the ENGINE half: the compile-time thunks, detour installation, and
// the block-scoped arg view. Engine-generic: nothing here names a game class, field, or function;
// the address arrives already resolved and validated by S2_EngineCallResolve, and the shape arrives
// as an id from the closed vocabulary in hook_dispatch.h.
//
// LAZY INSTALL. Core calls S2_HookInstall on the FIRST-EVER subscribe to a hook, idempotently —
// the same discipline as the UserCmd.onRun detour (s2script_mm.cpp). No subscribers means no
// patched bytes and no treadmill surface. s2detour::RemoveAll() at Unload restores every prologue.
extern "C" {

// Install the detour. `addr` is a resolved, validated absolute address. Returns 0, or -1 with a
// NUL-terminated reason in `reasonOut` (s2detour::Install refuses an unrelocatable prologue, and a
// shape with no compiled thunk is a named failure, never a silent skip).
int  S2_HookInstall(int hookId, int shape, int64_t addr, char* reasonOut, int reasonCap);

// Arm this hook's bypass latch. Core calls it immediately before invoking the `bypassWith` call
// descriptor, so our own outbound call does not fire our own hook (SourceMod parity).
void S2_HookArmBypass(int hookId);

// Block-scoped arg view accessors. `idx` is the descriptor's positional param index; every one is
// bounds-checked against the installed shape, so a stale generated binding cannot read off the
// frame. Return 0 on success, -1 on a bad index or a null view.
int  S2_HookReadF32 (void* argView, int idx, float* out);
int  S2_HookReadI32 (void* argView, int idx, int32_t* out);
int  S2_HookWriteF32(void* argView, int idx, float value);
int  S2_HookWriteI32(void* argView, int idx, int32_t value);

// The detour's `this`, as a packed CEntityHandle, when the descriptor surfaces it. -1 when it does
// not. NO RAW POINTER LEAVES THIS TU — core turns the handle into a books-gated EntityRef.
int  S2_HookReceiverHandle(void* argView, uint32_t* outHandle);

}  // extern "C"
