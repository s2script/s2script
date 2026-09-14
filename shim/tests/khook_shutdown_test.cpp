// Host test for the shim unload coordinator (shim/src/khook_shutdown.h).
// Injects counted/delayed actions into the same coordinator type Unload uses.
// Does not run V8 shutdown from a KHook removal worker.
#include "khook_shutdown.h"
#include "khook_map.h"

#include <cstdint>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>

static int g_fail = 0;
#define CHECK(cond, msg)                                                        \
    do {                                                                        \
        if (!(cond)) {                                                          \
            std::cerr << "FAIL: " << (msg) << "\n";                             \
            g_fail++;                                                           \
        } else {                                                                \
            std::cout << "ok:   " << (msg) << "\n";                             \
        }                                                                       \
    } while (0)

namespace KHook {
IKHook* __exported__khook = nullptr;
}

namespace {

class FakeKHook final : public KHook::IKHook {
public:
    KHook::HookID_t next_id = 1;
    int setup_hook_calls = 0;
    struct Removal {
        KHook::HookID_t id = KHook::INVALID_HOOK;
        bool async = false;
        void (*fn)(KHook::HookID_t, void*) = nullptr;
        void* ctx = nullptr;
    };
    std::vector<Removal> removals;

    KHook::HookID_t SetupHook(void*, void*, void*, void*, void*, void*, void*, unsigned int,
                             bool = false) override {
        ++setup_hook_calls;
        return next_id++;
    }
    KHook::HookID_t SetupVirtualHook(void**, int, void*, void*, void*, void*, void*, void*,
                                    unsigned int, bool = false) override {
        return next_id++;
    }
    void RemoveHook(KHook::HookID_t id, bool async = false,
                    void (*hook_removal_fn)(KHook::HookID_t, void*) = nullptr,
                    void* context = nullptr) override {
        removals.push_back({id, async, hook_removal_fn, context});
    }
    void FireLastCompletion() {
        if (removals.empty()) {
            return;
        }
        const Removal& r = removals.back();
        if (r.fn) {
            r.fn(r.id, r.ctx);
        }
    }
    void* GetContextPtr() override { return nullptr; }
    void* GetOriginalFunction() override { return nullptr; }
    void* GetOriginalValuePtr() override { return nullptr; }
    void* GetOverrideValuePtr() override { return nullptr; }
    void* GetCurrentValuePtr(bool = false) override { return nullptr; }
    void DestroyReturnValue() override {}
    void* FindOriginal(void*) override { return nullptr; }
    void* FindOriginalVirtual(void**, int) override { return nullptr; }
    void* DoRecall(KHook::Action, void*, std::size_t, void*, void*) override { return nullptr; }
    void SaveReturnValue(KHook::Action, void*, std::size_t, void*, void*, bool) override {}
    void* LookupSignature(void*, std::size_t, const char*) override { return nullptr; }
    bool WasOriginalFunctionSkipped() override { return false; }
};

static KHook::Return<void> FnPre() { return {KHook::Action::Ignore}; }
static void FnTarget() {}

struct Counts {
    bool can = true;
    bool retirement_done = true;
    bool provider_retained = true;
    int begin_calls = 0;
    int finish_calls = 0;
    int shutdown_calls = 0;
    int can_calls = 0;
    int complete_calls = 0;
};

S2ShutdownActions MakeActions(Counts* c) {
    S2ShutdownActions a;
    a.can_shutdown = [c]() {
        c->can_calls++;
        return c->can;
    };
    a.begin_retirement = [c]() { c->begin_calls++; };
    a.retirement_complete = [c]() {
        c->complete_calls++;
        return c->retirement_done;
    };
    a.finish_cleanup = [c]() {
        c->finish_calls++;
        c->shutdown_calls++;
        c->provider_retained = false;
    };
    return a;
}

static void test_active_callback_unload_is_busy() {
    S2ShutdownCoordinator coord;
    coord.Reset();
    Counts c;
    c.can = false;
    coord.SetActions(MakeActions(&c));

    auto r = coord.Unload();
    CHECK(r == S2UnloadAttempt::Busy, "active callback -> Unload=Busy (false)");
    CHECK(c.begin_calls == 0, "active callback: zero destructive begin_retirement");
    CHECK(c.finish_calls == 0, "active callback: zero finish_cleanup");
    CHECK(c.shutdown_calls == 0, "active callback: shutdown_calls=0");
    CHECK(coord.State() == S2ShutdownState::Running, "Busy leaves state Running");
    CHECK(std::strlen(S2UnloadAttemptMessage(r)) > 0, "Busy writes a named retry error");
}

static void test_original_between_pre_post_is_pending() {
    S2ShutdownCoordinator coord;
    coord.Reset();
    Counts c;
    c.can = true;
    c.retirement_done = false;
    coord.SetActions(MakeActions(&c));

    auto r = coord.Unload();
    CHECK(r == S2UnloadAttempt::Pending, "original between PRE/POST -> retirement pending");
    CHECK(c.begin_calls == 1, "safe initial request begins retirement once");
    CHECK(c.shutdown_calls == 0, "original between PRE/POST: shutdown_calls=0");
    CHECK(c.provider_retained, "provider retained while retirement is pending");
    CHECK(coord.State() == S2ShutdownState::Retiring, "pending unload is Retiring");
}

static void test_pending_completion_rejects_registration() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    S2ShutdownCoordinator coord;
    coord.Reset();
    Counts c;
    c.can = true;
    c.retirement_done = false;
    coord.SetActions(MakeActions(&c));

    CHECK(coord.Unload() == S2UnloadAttempt::Pending, "first unload is pending");
    CHECK(coord.State() == S2ShutdownState::Retiring, "pending completion -> state Retiring");

    auto retry = coord.Unload();
    CHECK(retry == S2UnloadAttempt::Pending, "pending completion -> Unload=false");
    CHECK(c.begin_calls == 1, "pending retry does not begin retirement again");
    CHECK(c.shutdown_calls == 0, "pending completion: shutdown_calls=0");

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto rec = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(!rec.Accepted(), "registration rejected while Retiring");
    CHECK(rec.state == S2HookState::Failed, "rejected registration is Failed");
    CHECK(!rec.reason.empty(), "rejected registration has a named reason");
}

static void test_late_completion_then_external_retry() {
    S2ShutdownCoordinator coord;
    coord.Reset();
    Counts c;
    c.can = true;
    c.retirement_done = false;
    coord.SetActions(MakeActions(&c));

    CHECK(coord.Unload() == S2UnloadAttempt::Pending, "first unload waits on completion");
    CHECK(c.shutdown_calls == 0, "late completion has not shut down yet");
    c.retirement_done = true;
    auto r = coord.Unload();
    CHECK(r == S2UnloadAttempt::Complete, "late completion then external retry -> Unload=true");
    CHECK(c.shutdown_calls == 1, "late completion then external retry -> shutdown_calls=1");
    CHECK(c.finish_calls == 1, "finish_cleanup runs once on the successful retry");
    CHECK(coord.State() == S2ShutdownState::Ready, "successful retry reaches Ready");
}

static void test_second_teardown_is_idempotent() {
    S2ShutdownCoordinator coord;
    coord.Reset();
    Counts c;
    coord.SetActions(MakeActions(&c));

    CHECK(coord.Unload() == S2UnloadAttempt::Complete, "first teardown completes");
    CHECK(c.shutdown_calls == 1, "first teardown shuts down once");
    CHECK(coord.Unload() == S2UnloadAttempt::Complete, "second teardown reports Complete");
    CHECK(c.begin_calls == 1, "second teardown does not begin retirement again");
    CHECK(c.finish_calls == 1, "second teardown -> no duplicate resource cleanup");
    CHECK(c.shutdown_calls == 1, "second teardown does not call shutdown again");
}

static void test_never_finish_after_failed_readiness() {
    S2ShutdownCoordinator coord;
    coord.Reset();
    Counts c;
    c.can = true;
    c.retirement_done = false;
    coord.SetActions(MakeActions(&c));

    CHECK(coord.Unload() == S2UnloadAttempt::Pending, "begin retirement on a safe first request");
    CHECK(c.shutdown_calls == 0, "pending begin has not finished");
    c.retirement_done = true;
    c.can = false;
    auto blocked = coord.Unload();
    CHECK(blocked == S2UnloadAttempt::Pending || blocked == S2UnloadAttempt::Busy,
          "failed readiness after begin does not finish");
    CHECK(c.shutdown_calls == 0, "never call finish after a failed readiness check");
    c.can = true;
    CHECK(coord.Unload() == S2UnloadAttempt::Complete, "later ready retry finishes once");
    CHECK(c.shutdown_calls == 1, "finish runs once after readiness recovers");
}

}  // namespace

int main() {
    test_active_callback_unload_is_busy();
    test_original_between_pre_post_is_pending();
    test_pending_completion_rejects_registration();
    test_late_completion_then_external_retry();
    test_second_teardown_is_idempotent();
    test_never_finish_after_failed_readiness();

    if (g_fail) {
        std::cerr << g_fail << " check(s) failed\n";
        return 1;
    }
    std::cout << "khook_shutdown_test: all checks passed\n";
    return 0;
}
