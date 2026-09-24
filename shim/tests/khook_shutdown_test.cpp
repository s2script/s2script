#include "khook_shutdown.h"

#include <iostream>

static int g_fail = 0;
#define CHECK(cond, msg) do { \
    if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; ++g_fail; } \
    else { std::cout << "ok:   " << (msg) << "\n"; } \
} while (0)

namespace {

struct Calls {
    bool ready = true;
    bool finish = true;
    bool remove_markers = true;
    int readiness = 0;
    int cleanup = 0;
    int marker_remove = 0;
};

S2TerminalPreActions Pre(Calls& c) {
    return {
        [&] { ++c.readiness; return c.ready; },
        [&] { ++c.cleanup; return c.finish; },
    };
}

static void test_happy_path_is_staged_and_exactly_once() {
    S2TerminalCoordinator c;
    Calls calls;
    c.Reset(88);
    CHECK(c.PreShutdownPre(88, Pre(calls)), "owner PreShutdown PRE completes cleanup");
    CHECK(c.Phase() == S2TerminalPhase::PreCleaned, "PRE cleanup phase recorded");
    CHECK(calls.cleanup == 1, "terminal cleanup runs once");
    CHECK(c.PreShutdownPost(88, false), "PreShutdown POST records original return");
    CHECK(c.ShutdownPre(88), "Shutdown PRE records entry without removing its peer marker");
    CHECK(c.ShutdownPost(88, false), "Shutdown POST records original return");
    CHECK(c.Unload(88, [&] { ++calls.marker_remove; return calls.remove_markers; }),
          "later Unload removes both lifecycle markers off-stack");
    CHECK(c.Unload(89, [&] { ++calls.marker_remove; return false; }),
          "completed Unload is idempotent");
    CHECK(calls.cleanup == 1 && calls.marker_remove == 1,
          "no staged action repeats");
}

static void test_ordinary_unload_is_zero_mutation() {
    S2TerminalCoordinator c;
    Calls calls;
    c.Reset(88);
    CHECK(!c.Unload(89, [&] { ++calls.marker_remove; return true; }),
          "ordinary native unload is rejected");
    CHECK(calls.marker_remove == 0, "ordinary unload invokes no removal action");
    CHECK(c.Phase() == S2TerminalPhase::Running, "ordinary unload preserves Running phase");
    CHECK(S2Hook_Lifecycle() == S2HookLifecycle::Running,
          "ordinary unload keeps hook registration and dispatch live");
}

static void test_wrong_thread_and_busy_refuse_before_mutation() {
    S2TerminalCoordinator c;
    Calls calls;
    c.Reset(88);
    CHECK(!c.PreShutdownPre(89, Pre(calls)), "wrong owner thread is rejected");
    CHECK(calls.readiness == 0 && calls.cleanup == 0, "wrong thread mutates nothing");

    c.Reset(88);
    calls.ready = false;
    CHECK(!c.PreShutdownPre(88, Pre(calls)), "busy core is rejected");
    CHECK(calls.readiness == 1 && calls.cleanup == 0, "busy core runs no cleanup");
    CHECK(S2Hook_Lifecycle() == S2HookLifecycle::Running,
          "busy refusal occurs before Retiring publication");
}

static void test_failed_cleanup_never_advances() {
    S2TerminalCoordinator c;
    Calls calls;
    calls.finish = false;
    c.Reset(88);
    CHECK(!c.PreShutdownPre(88, Pre(calls)), "reported cleanup failure rejects PRE completion");
    CHECK(c.Phase() == S2TerminalPhase::Failed, "cleanup failure is terminal Failed");
    CHECK(!c.PreShutdownPre(88, Pre(calls)), "failed cleanup is not retried");
    CHECK(calls.cleanup == 1, "failed cleanup action runs exactly once");
    CHECK(!c.PreShutdownPost(88, false), "failed cleanup cannot advance at POST");
    CHECK(!c.ShutdownPre(88), "failed cleanup cannot advance lifecycle phase");
}

static void test_marker_completion_is_load_bearing() {
    S2TerminalCoordinator c;
    Calls calls;
    c.Reset(88);
    CHECK(c.PreShutdownPre(88, Pre(calls)) && c.PreShutdownPost(88, false) &&
          c.ShutdownPre(88) && c.ShutdownPost(88, false),
          "reach final marker stage");
    calls.remove_markers = false;
    CHECK(!c.Unload(88, [&] { ++calls.marker_remove; return calls.remove_markers; }),
          "withheld lifecycle marker completion rejects unload success");
    CHECK(c.Phase() == S2TerminalPhase::Failed, "final marker failure cannot report Complete");
}

static void test_original_skip_and_out_of_order_callbacks_fail_closed() {
    S2TerminalCoordinator c;
    Calls calls;
    c.Reset(88);
    CHECK(!c.PreShutdownPost(88, false), "PreShutdown POST before PRE is rejected");
    CHECK(c.Phase() == S2TerminalPhase::Failed, "out-of-order PRE callback is terminal Failed");

    c.Reset(88);
    CHECK(c.PreShutdownPre(88, Pre(calls)), "reach PreShutdown POST");
    CHECK(!c.PreShutdownPost(88, true), "skipped PreShutdown original is rejected");
    CHECK(c.Phase() == S2TerminalPhase::Failed, "skipped PreShutdown original fails closed");

    c.Reset(88);
    calls = Calls{};
    CHECK(c.PreShutdownPre(88, Pre(calls)) && c.PreShutdownPost(88, false) &&
          c.ShutdownPre(88),
          "reach Shutdown POST");
    CHECK(!c.ShutdownPost(88, true), "skipped Shutdown original is rejected");
    CHECK(c.Phase() == S2TerminalPhase::Failed, "skipped Shutdown original fails closed");
}

static void test_later_phases_require_the_core_owner_thread() {
    S2TerminalCoordinator c;
    Calls calls;
    c.Reset(88);
    CHECK(c.PreShutdownPre(88, Pre(calls)), "reach PreShutdown POST for TID check");
    CHECK(!c.PreShutdownPost(89, false), "PreShutdown POST rejects a different thread");

    c.Reset(88);
    calls = Calls{};
    CHECK(c.PreShutdownPre(88, Pre(calls)) && c.PreShutdownPost(88, false),
          "reach Shutdown PRE for TID check");
    CHECK(!c.ShutdownPre(89),
          "Shutdown PRE rejects a different thread");

    c.Reset(88);
    calls = Calls{};
    CHECK(c.PreShutdownPre(88, Pre(calls)) && c.PreShutdownPost(88, false) &&
          c.ShutdownPre(88) &&
          c.ShutdownPost(88, false), "reach final Unload for TID check");
    CHECK(!c.Unload(89, [&] { ++calls.marker_remove; return true; }),
          "final Unload rejects a different thread");
    CHECK(calls.marker_remove == 0, "wrong-thread final Unload invokes no marker action");
    CHECK(c.Phase() == S2TerminalPhase::Failed, "wrong-thread terminal phase fails closed");
}

}  // namespace

int main() {
    test_happy_path_is_staged_and_exactly_once();
    test_ordinary_unload_is_zero_mutation();
    test_wrong_thread_and_busy_refuse_before_mutation();
    test_failed_cleanup_never_advances();
    test_marker_completion_is_load_bearing();
    test_original_skip_and_out_of_order_callbacks_fail_closed();
    test_later_phases_require_the_core_owner_thread();
    if (g_fail) return 1;
    std::cout << "khook_shutdown_test: all checks passed\n";
    return 0;
}
