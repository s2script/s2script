#pragma once

// Staged process-terminal shutdown. Ordinary native unload is deliberately
// non-destructive: the only transition out of Running starts in the public
// ISource2ServerConfig::PreShutdown callback on the core owner thread.

#include "khook_binding.h"

#include <functional>

enum class S2TerminalPhase {
    Running,
    PreCleaned,
    PreReturned,
    ShutdownEntered,
    ShutdownReturned,
    Complete,
    Failed,
};

struct S2TerminalPreActions {
    std::function<bool()> can_begin;
    std::function<bool()> retire_and_finish;
};

class S2TerminalCoordinator {
public:
    void Reset(long owner_tid) {
        phase_ = S2TerminalPhase::Running;
        owner_tid_ = owner_tid;
        S2Hook_SetLifecycle(S2HookLifecycle::Running);
    }

    S2TerminalPhase Phase() const { return phase_; }
    long OwnerTid() const { return owner_tid_; }

    bool PreShutdownPre(long current_tid, S2TerminalPreActions actions) {
        if (phase_ != S2TerminalPhase::Running || current_tid != owner_tid_ ||
            (actions.can_begin && !actions.can_begin())) {
            phase_ = S2TerminalPhase::Failed;
            return false;
        }
        S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
        if (!actions.retire_and_finish || !actions.retire_and_finish()) {
            phase_ = S2TerminalPhase::Failed;
            return false;
        }
        phase_ = S2TerminalPhase::PreCleaned;
        return true;
    }

    bool PreShutdownPost(long current_tid, bool original_skipped) {
        if (phase_ != S2TerminalPhase::PreCleaned || current_tid != owner_tid_ || original_skipped) {
            phase_ = S2TerminalPhase::Failed;
            return false;
        }
        phase_ = S2TerminalPhase::PreReturned;
        return true;
    }

    bool ShutdownPre(long current_tid) {
        if (phase_ != S2TerminalPhase::PreReturned || current_tid != owner_tid_) {
            phase_ = S2TerminalPhase::Failed;
            return false;
        }
        phase_ = S2TerminalPhase::ShutdownEntered;
        return true;
    }

    bool ShutdownPost(long current_tid, bool original_skipped) {
        if (phase_ != S2TerminalPhase::ShutdownEntered || current_tid != owner_tid_ || original_skipped) {
            phase_ = S2TerminalPhase::Failed;
            return false;
        }
        phase_ = S2TerminalPhase::ShutdownReturned;
        return true;
    }

    bool Unload(long current_tid, const std::function<bool()>& remove_lifecycle_markers) {
        if (phase_ == S2TerminalPhase::Complete) return true;
        if (phase_ != S2TerminalPhase::ShutdownReturned) return false;
        if (current_tid != owner_tid_) {
            phase_ = S2TerminalPhase::Failed;
            return false;
        }
        if (!remove_lifecycle_markers || !remove_lifecycle_markers()) {
            phase_ = S2TerminalPhase::Failed;
            return false;
        }
        phase_ = S2TerminalPhase::Complete;
        S2Hook_SetLifecycle(S2HookLifecycle::Ready);
        return true;
    }

private:
    S2TerminalPhase phase_ = S2TerminalPhase::Running;
    long owner_tid_ = -1;
};
