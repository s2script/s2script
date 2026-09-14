#pragma once
#include <khook.hpp>
#include "khook_binding.h"
// Action helpers for KHook handlers. Observe/BeginRemove/Drain live on the
// checked bindings: keep `auto obs = binding.Observe(...)` alive for the
// callback and honor `S2Hook_EnterDispatch(obs)` before JS (constructing
// Observe is not enough). DrainRetirement() true means not on a callback
// stack (Unload also requires RetirementPending()==0). Direct core-dispatch
// entries use S2HookDispatchGuard. Retirement retains S2HookBindingState,
// not the typed S2Checked* object — keep that object until Removed.

inline KHook::Return<void> S2_Ignore() { return { KHook::Action::Ignore }; }
inline KHook::Return<void> S2_Supersede() { return { KHook::Action::Supersede }; }
template <typename T>
inline KHook::Return<T> S2_Ignore(T v) { return { KHook::Action::Ignore, v }; }
template <typename T>
inline KHook::Return<T> S2_Supersede(T v) { return { KHook::Action::Supersede, v }; }
// Local skip decision only. By-value edits also need Recall;
// CanAcquire uses its dedicated vote/engine return policy in T9.
// collapsed JS HookResult: 0 Continue, 1 Changed, 2 Handled, 3 Stop
inline KHook::Return<void> S2_FromHookResult(int r) {
    return r >= 2 ? S2_Supersede() : S2_Ignore();
}
template <typename T>
inline KHook::Return<T> S2_FromHookResult(int r, T v) {
    return r >= 2 ? S2_Supersede(v) : S2_Ignore(v);
}
