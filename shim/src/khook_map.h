#pragma once
#include <khook.hpp>
#include "khook_binding.h"

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
