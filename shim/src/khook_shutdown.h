#pragma once

// Running/Retiring/Ready decision state for checked-hook unload. This is not
// another hook registry: S2ShutdownCoordinator consumes injected actions and
// returns Busy, Pending or Complete. Production Unload supplies real actions;
// the host test supplies counted/delayed actions to the same type.
//
// Call begin/finish at most once and never call finish after a failed
// readiness check.

#include "khook_binding.h"

#include <functional>

enum class S2ShutdownState { Running, Retiring, Ready };
enum class S2UnloadAttempt { Busy, Pending, Complete };

struct S2ShutdownActions {
    std::function<bool()> can_shutdown;
    std::function<void()> begin_retirement;
    std::function<bool()> retirement_complete;
    std::function<void()> finish_cleanup;
};

inline const char* S2UnloadAttemptMessage(S2UnloadAttempt attempt) {
    switch (attempt) {
    case S2UnloadAttempt::Busy:
        return "s2script unload rejected: dispatch is active; retry meta unload";
    case S2UnloadAttempt::Pending:
        return "s2script unload pending: hook retirement in progress; retry meta unload";
    case S2UnloadAttempt::Complete:
        return "";
    }
    return "s2script unload rejected; retry meta unload";
}

class S2ShutdownCoordinator {
public:
    void Reset() {
        state_ = S2ShutdownState::Running;
        begun_ = false;
        finished_ = false;
        S2Hook_SetLifecycle(S2HookLifecycle::Running);
    }

    void SetActions(S2ShutdownActions actions) { actions_ = std::move(actions); }

    S2ShutdownState State() const { return state_; }

    S2UnloadAttempt Unload() {
        if (finished_ || state_ == S2ShutdownState::Ready) {
            return S2UnloadAttempt::Complete;
        }
        const bool can = !actions_.can_shutdown || actions_.can_shutdown();
        if (!can) {
            return begun_ ? S2UnloadAttempt::Pending : S2UnloadAttempt::Busy;
        }
        if (!begun_) {
            begun_ = true;
            state_ = S2ShutdownState::Retiring;
            S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
            if (actions_.begin_retirement) {
                actions_.begin_retirement();
            }
        }
        const bool complete = !actions_.retirement_complete || actions_.retirement_complete();
        if (!complete) {
            return S2UnloadAttempt::Pending;
        }
        const bool still = !actions_.can_shutdown || actions_.can_shutdown();
        if (!still) {
            return S2UnloadAttempt::Pending;
        }
        if (!finished_) {
            finished_ = true;
            if (actions_.finish_cleanup) {
                actions_.finish_cleanup();
            }
            state_ = S2ShutdownState::Ready;
            S2Hook_SetLifecycle(S2HookLifecycle::Ready);
        }
        return S2UnloadAttempt::Complete;
    }

private:
    S2ShutdownState state_ = S2ShutdownState::Running;
    S2ShutdownActions actions_{};
    bool begun_ = false;
    bool finished_ = false;
};
