// Host test for the checked KHook binding (shim/src/khook_binding.h).
//
// Injects KHook::IKHook via KHook::__exported__khook. This proves registration
// receipts, Observe/BeginRemove bookkeeping, this-filter removal, and
// completion-aware retirement. It does not prove native trampolines.
//
// Public FFI conventions (unchanged; not exercised here): SDKHooks VP add uses
// nonzero success; declarative S2_HookInstall uses 0 success / -1 failure.
// T5 phase/filter: Add only the first {entity,kind} row; Remove only when the last
// phase is gone. These fixtures prove the binding contract (shared id, this-filter,
// both unsubscribe orders, in-callback Remove). T8 owns real-KHook delivery.
#include "khook_map.h"

#include <atomic>
#include <cstdint>
#include <iostream>
#include <string>
#include <thread>
#include <utility>
#include <vector>

static int g_fail = 0;
#define CHECK(cond, msg)                                                        \
    do {                                                                         \
        if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; g_fail++; }      \
        else         { std::cout << "ok:   " << (msg) << "\n"; }                \
    } while (0)

namespace KHook {
IKHook* __exported__khook = nullptr;
}

namespace {

class FakeKHook final : public KHook::IKHook {
public:
    KHook::HookID_t next_id = 1;
    bool fail_setup = false;
    int setup_hook_calls = 0;
    int setup_virtual_calls = 0;
    int find_original_calls = 0;
    int find_original_virtual_calls = 0;
    bool invoke_typed_removal = false;
    bool complete_sync = true;
    void* current_context = nullptr;
    struct TypedRemoval { void* context; void* helper; };
    std::unordered_map<KHook::HookID_t, TypedRemoval> typed_removals;

    struct Removal {
        KHook::HookID_t id = KHook::INVALID_HOOK;
        bool async = false;
        void (*fn)(KHook::HookID_t, void*) = nullptr;
        void* ctx = nullptr;
    };
    std::vector<Removal> removals;

    KHook::HookID_t SetupHook(void*, void* context, void* helper, void*, void*, void*, void*, unsigned int,
                             bool = false) override {
        ++setup_hook_calls;
        if (fail_setup) {
            return KHook::INVALID_HOOK;
        }
        const auto id = next_id++;
        if (invoke_typed_removal) typed_removals.emplace(id, TypedRemoval{context, helper});
        return id;
    }

    KHook::HookID_t SetupVirtualHook(void**, int, void* context, void* helper, void*, void*, void*, void*,
                                    unsigned int, bool = false) override {
        ++setup_virtual_calls;
        if (fail_setup) {
            return KHook::INVALID_HOOK;
        }
        const auto id = next_id++;
        if (invoke_typed_removal) typed_removals.emplace(id, TypedRemoval{context, helper});
        return id;
    }

    void RemoveHook(KHook::HookID_t id, bool async = false,
                    void (*hook_removal_fn)(KHook::HookID_t, void*) = nullptr,
                    void* context = nullptr) override {
        removals.push_back({id, async, hook_removal_fn, context});
        if (!async && complete_sync) {
            Complete(removals.back());
        }
    }

    void FireLastCompletion() {
        if (removals.empty()) {
            return;
        }
        Complete(removals.back());
    }
    void FireCompletion(std::size_t index) { Complete(removals.at(index)); }

private:
    void Complete(const Removal& r) {
        const auto helper = typed_removals.find(r.id);
        if (helper != typed_removals.end()) {
            const auto details = helper->second;
            typed_removals.erase(helper);
            current_context = details.context;
            reinterpret_cast<void (*)(KHook::HookID_t)>(details.helper)(r.id);
            current_context = nullptr;
        }
        if (r.fn) {
            r.fn(r.id, r.ctx);
        }
    }

public:

    void* GetContextPtr() override { return current_context; }
    void* GetOriginalFunction() override { return nullptr; }
    void* GetOriginalValuePtr() override { return nullptr; }
    void* GetOverrideValuePtr() override { return nullptr; }
    void* GetCurrentValuePtr(bool = false) override { return nullptr; }
    void DestroyReturnValue() override {}
    void* FindOriginal(void*) override {
        ++find_original_calls;
        return nullptr;
    }
    void* FindOriginalVirtual(void**, int) override {
        ++find_original_virtual_calls;
        return nullptr;
    }
    void* DoRecall(KHook::Action, void*, std::size_t, void*, void*) override { return nullptr; }
    void SaveReturnValue(KHook::Action, void*, std::size_t, void*, void*, bool) override {}
    void* LookupSignature(void*, std::size_t, const char*) override { return nullptr; }
    bool WasOriginalFunctionSkipped() override { return false; }
};

struct Dummy {
    virtual void Go() {}
    virtual ~Dummy() = default;
};

static KHook::Return<void> FnPre() { return {KHook::Action::Ignore}; }
static void FnTarget() {}
static void FnTargetB() {}
static void FnTargetC() {}
template <int N> static void InventoryTarget() { (void)N; }

static KHook::Return<void> DummyPre(Dummy* self) {
    (void)self;
    return {KHook::Action::Ignore};
}
static KHook::Return<void> DummyPost(Dummy* self) {
    (void)self;
    return {KHook::Action::Ignore};
}

struct WatchFn : S2CheckedFunction<void> {
    using S2CheckedFunction<void>::S2CheckedFunction;
    bool* died = nullptr;
    ~WatchFn() {
        if (died) {
            *died = true;
        }
    }
};

static void test_action_helpers() {
    CHECK(S2_Ignore().action == KHook::Action::Ignore, "S2_Ignore void is Ignore");
    CHECK(S2_Supersede().action == KHook::Action::Supersede, "S2_Supersede void is Supersede");
    CHECK(S2_Ignore(true).action == KHook::Action::Ignore, "S2_Ignore(T) is Ignore");
    CHECK(S2_Ignore(true).ret == true, "S2_Ignore(T) keeps the value");
    CHECK(S2_Supersede(false).action == KHook::Action::Supersede, "S2_Supersede(T) is Supersede");
    CHECK(S2_Supersede(false).ret == false, "S2_Supersede(T) keeps the value");
    CHECK(S2_FromHookResult(0).action == KHook::Action::Ignore, "HookResult Continue -> Ignore");
    CHECK(S2_FromHookResult(1).action == KHook::Action::Ignore, "HookResult Changed -> Ignore");
    CHECK(S2_FromHookResult(2).action == KHook::Action::Supersede, "HookResult Handled -> Supersede");
    CHECK(S2_FromHookResult(3).action == KHook::Action::Supersede, "HookResult Stop -> Supersede");
    CHECK(S2_FromHookResult(1, true).action == KHook::Action::Ignore, "Changed+value -> Ignore");
    CHECK(S2_FromHookResult(2, false).action == KHook::Action::Supersede, "Handled+value -> Supersede");
}

static void test_setup_invalid_is_failed_named_no_filter() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    fake.fail_setup = true;
    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto rec = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(rec.id == KHook::INVALID_HOOK, "INVALID_HOOK from SetupHook");
    CHECK(rec.state == S2HookState::Failed, "INVALID_HOOK is Failed");
    CHECK(!rec.Accepted(), "Failed receipt is not Accepted");
    CHECK(!rec.reason.empty(), "Failed Function Configure has a named reason");
    CHECK(fn.Snapshot().state == S2HookState::Failed, "Function snapshot stays Failed");
    CHECK(fake.find_original_calls == 0, "Failed Function Configure does not probe FindOriginal");

    Dummy obj;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, &DummyPost);
    auto vrec = virt.Add(&obj);
    CHECK(vrec.id == KHook::INVALID_HOOK, "INVALID_HOOK from SetupVirtualHook");
    CHECK(vrec.state == S2HookState::Failed, "failed Virtual Add is Failed");
    CHECK(!vrec.Accepted(), "failed Virtual Add is not Accepted");
    CHECK(!vrec.reason.empty(), "failed Virtual Add has a named reason");
    CHECK(!virt.HasThisFilter(&obj), "failed Virtual Add does not retain a this-filter");
    CHECK(fake.find_original_virtual_calls == 0,
          "failed Virtual Add does not probe FindOriginalVirtual");

    auto null_fn = fn.Configure(static_cast<const void*>(nullptr));
    CHECK(null_fn.state == S2HookState::Failed, "null Function address is Failed");
    CHECK(!null_fn.reason.empty(), "null Function address has a named reason");

    auto null_add = virt.Add(nullptr);
    CHECK(null_add.state == S2HookState::Failed, "null Virtual Add is Failed");
    CHECK(!null_add.reason.empty(), "null Virtual Add has a named reason");
    CHECK(fake.setup_virtual_calls == 1, "null object is rejected before SetupVirtualHook");

    S2CheckedVirtual<Dummy, void> unslotted(&DummyPre, nullptr);
    Dummy obj2;
    auto bad_slot = unslotted.Add(&obj2);
    CHECK(bad_slot.state == S2HookState::Failed, "Add before Configure(slot) is Failed");
    CHECK(!unslotted.HasThisFilter(&obj2), "invalid slot Add does not retain a this-filter");

    auto glob = virt.AddGlobal(nullptr);
    CHECK(glob.state == S2HookState::Failed, "null AddGlobal is Failed");
    CHECK(!glob.reason.empty(), "null AddGlobal has a named reason");

}

static void test_valid_id_is_pending_until_observe() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto rec = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(rec.id != KHook::INVALID_HOOK, "successful SetupHook yields a KHook id");
    CHECK(rec.id == 1u, "receipt id is the KHook id, not a declarative s2script id");
    CHECK(rec.state == S2HookState::Pending, "valid id is Pending before callback");
    CHECK(rec.Accepted(), "Pending receipt is Accepted");
    CHECK(fn.Snapshot().state != S2HookState::Active, "no Active claim before Observe");
    CHECK(fake.find_original_calls == 0, "Pending Function does not probe the engine");
    CHECK(fake.setup_hook_calls == 1, "one SetupHook for a new Function capsule");

    Dummy obj;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
    auto vrec = virt.Add(&obj);
    CHECK(vrec.state == S2HookState::Pending, "valid Virtual Add is Pending");
    CHECK(vrec.Accepted(), "Pending Virtual is Accepted");
    CHECK(virt.HasThisFilter(&obj), "accepted Virtual Add retains the this-filter");
    CHECK(virt.Snapshot().state != S2HookState::Active, "no Active claim before Observe");
    CHECK(fake.find_original_virtual_calls == 0, "Pending Virtual does not probe the engine");

}

static void test_first_matching_callback_becomes_active_once() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto rec = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    {
        auto obs = fn.Observe();
        CHECK(static_cast<bool>(obs), "Function Observe arms a live guard");
        CHECK(fn.Snapshot().id == rec.id, "Observe keeps the same KHook id");
        CHECK(fn.Snapshot().state == S2HookState::Active, "first matching Function callback is Active");
        auto obs2 = fn.Observe();
        CHECK(fn.Snapshot().state == S2HookState::Active, "Observe is Active once, not a new id");
        CHECK(!S2Hook_DrainRetirement(), "nested live guards are an active callback stack");
    }

    Dummy obj;
    Dummy other;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
    auto vrec = virt.Add(&obj);
    {
        auto miss = virt.Observe(&other);
        CHECK(!miss, "unmatched this pointer returns an empty guard");
        CHECK(virt.Snapshot().state == S2HookState::Pending, "unmatched this pointer does not activate");
        CHECK(S2Hook_DrainRetirement(), "empty unmatched guard does not hold the callback stack");
        auto obs = virt.Observe(&obj);
        CHECK(virt.Snapshot().id == vrec.id, "matching Observe keeps the same id");
        CHECK(virt.Snapshot().state == S2HookState::Active, "first matching Virtual callback is Active");
        auto obs2 = virt.Observe(&obj);
        CHECK(virt.Snapshot().state == S2HookState::Active, "Virtual Observe is Active once");
        CHECK(!S2Hook_DrainRetirement(), "live Virtual guards are an active callback stack");
    }

}

static void test_pre_post_share_one_virtual_id() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    Dummy obj;
    Dummy other;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, &DummyPost);
    auto pre = virt.Add(&obj);
    CHECK(fake.setup_virtual_calls == 1, "PRE+POST share one SetupVirtualHook");
    auto post = virt.Add(&obj);
    CHECK(post.id == pre.id, "PRE/POST re-add reuses the same KHook id");
    CHECK(fake.setup_virtual_calls == 1, "re-add does not install a second physical hook");
    CHECK(virt.HasThisFilter(&obj), "second phase reuses the accepted this-filter");
    CHECK(!virt.HasThisFilter(&other), "one-entity Add does not install a this-filter for a peer");
    {
        auto miss = virt.Observe(&other);
        CHECK(!miss, "one-entity-only: unmatched this returns an empty Observe guard");
        auto hit = virt.Observe(&obj);
        CHECK(static_cast<bool>(hit), "matched entity Observe arms a live guard");
    }

    // T5 FFI: Remove only when the last {entity,kind} phase is gone. Binding Remove always
    // drops the this-filter, so PRE-unsubscribe while POST is live must NOT call it.
    CHECK(virt.HasThisFilter(&obj), "PRE unsubscribe while POST live must not call Remove");
    virt.Remove(&obj);
    CHECK(!virt.HasThisFilter(&obj), "last phase drops the this-filter immediately");
    CHECK(virt.Snapshot().id == pre.id, "physical id is retained after last-phase filter drop");
    CHECK(virt.Snapshot().state != S2HookState::Removed,
          "filter drop is not physical removal");
    CHECK(fake.removals.empty(), "phase/filter drop does not call RemoveHook");

}

static void test_both_unsubscribe_orders_keep_filter_until_last_remove() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    Dummy pre_first;
    S2CheckedVirtual<Dummy, void> a(0u, &DummyPre, &DummyPost);
    CHECK(a.Add(&pre_first).Accepted(), "first entity+kind Add is accepted");
    CHECK(a.Add(&pre_first).Accepted(), "second phase reuses the accepted registration");
    CHECK(a.HasThisFilter(&pre_first), "PRE-then-POST keeps the this-filter until last Remove");
    a.Remove(&pre_first);
    CHECK(!a.HasThisFilter(&pre_first), "PRE-then-POST last Remove drops the this-filter");

    Dummy post_first;
    S2CheckedVirtual<Dummy, void> both(0u, &DummyPre, &DummyPost);
    CHECK(both.Add(&post_first).Accepted(), "POST-then-PRE first Add is accepted");
    CHECK(both.Add(&post_first).id == both.Snapshot().id, "POST-then-PRE reuses the same id");
    CHECK(both.HasThisFilter(&post_first), "POST unsubscribe while PRE live must not call Remove");
    both.Remove(&post_first);
    CHECK(!both.HasThisFilter(&post_first), "POST-then-PRE last Remove drops the this-filter");
    CHECK(fake.removals.empty(), "neither unsubscribe order physically RemoveHooks");
}

static void test_unsubscribe_in_callback_drops_filter_without_destroying() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    Dummy obj;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
    auto rec = virt.Add(&obj);
    {
        auto obs = virt.Observe(&obj);
        virt.Remove(&obj);
        CHECK(!virt.HasThisFilter(&obj), "unsubscribe in callback drops the this-filter immediately");
        CHECK(virt.Snapshot().state != S2HookState::Removed, "unsubscribe is not synchronous destruction");
        CHECK(virt.Snapshot().id == rec.id, "binding still owns the physical id after filter drop");
        CHECK(fake.removals.empty(), "in-callback unsubscribe does not call RemoveHook");
        CHECK(!S2Hook_DrainRetirement(), "Unload/drain on an active callback stack is rejected");
    }
    CHECK(S2Hook_DrainRetirement(), "drain is allowed once the Observe guard unwinds");

}

static void test_delayed_completion_keeps_context() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    bool died = false;
    auto* fn = new WatchFn(&FnPre, nullptr);
    fn->died = &died;
    auto rec = fn->Configure(reinterpret_cast<void*>(&FnTarget));
    {
        auto obs = fn->Observe();
        fn->BeginRemove();
        CHECK(fn->Snapshot().state == S2HookState::Removing, "BeginRemove marks Removing");
        CHECK(fake.removals.size() == 1, "BeginRemove schedules one RemoveHook");
        CHECK(fake.removals[0].id == rec.id, "RemoveHook uses the actual KHook id");
        CHECK(fake.removals[0].async == true, "physical removal is async");
        CHECK(fake.removals[0].fn != nullptr, "BeginRemove passes a completion callback");
        CHECK(S2Hook_RetirementPending() == 1, "retirement queue retains the binding");
        CHECK(!died, "binding is not destroyed when RemoveHook is scheduled");

        fake.FireLastCompletion();
        CHECK(!died, "completion worker does not free the binding");
        CHECK(fn->Snapshot().state == S2HookState::Removing,
              "completion with an outstanding invocation stays Removing");
        CHECK(!S2Hook_DrainRetirement(), "drain/unload on the callback stack is deferred");
    }
    CHECK(fn->Snapshot().state == S2HookState::Removed,
          "Removed only after completion and Observe guard release");
    CHECK(S2Hook_DrainRetirement(), "drain outside the callback stack succeeds");
    CHECK(S2Hook_RetirementPending() == 0, "drain drops completed retirement entries");
    CHECK(!died, "caller still owns the binding after drain");
    delete fn;
    CHECK(died, "caller may destroy the binding after Removed");

}

static void test_idempotent_add_one_completion_per_id() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    Dummy a;
    Dummy b;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
    auto r1 = virt.Add(&a);
    auto r2 = virt.Add(&a);
    CHECK(r1.id == r2.id, "idempotent Add of the same object keeps one id");
    CHECK(fake.setup_virtual_calls == 1, "re-add does not create a second capsule");

    auto r3 = virt.Add(&b);
    CHECK(r3.id == r1.id, "second this-filter on the same vtable shares the physical id");
    CHECK(fake.setup_virtual_calls == 1, "shared capsule does not SetupVirtualHook again");

    virt.BeginRemove();
    virt.BeginRemove();
    CHECK(fake.removals.size() == 1, "BeginRemove schedules one RemoveHook per physical id");
    fake.FireLastCompletion();
    CHECK(virt.Snapshot().state == S2HookState::Removed, "one completion retires the shared id");
    CHECK(S2Hook_DrainRetirement(), "drain after completion with no callback stack");
    CHECK(S2Hook_RetirementPending() == 0, "one completion clears retirement");

    Dummy holder;
    S2CheckedVirtual<Dummy, void> glob(0u, &DummyPre, nullptr);
    auto g1 = glob.AddGlobal(&holder);
    auto g2 = glob.AddGlobal(&holder);
    CHECK(g1.Accepted() && g2.id == g1.id, "idempotent AddGlobal keeps one id");
    CHECK(glob.HasGlobalFilter(&holder), "accepted AddGlobal retains the global filter");
    glob.BeginRemove();
    CHECK(fake.removals.size() == 2, "Function/Virtual ids are retired independently");
    fake.FireLastCompletion();
    CHECK(S2Hook_DrainRetirement(), "drain drops completed global-filter retirement");

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto f1 = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    auto f2 = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(f1.id == f2.id, "idempotent Function Configure keeps one id");
    CHECK(fake.setup_hook_calls == 1, "re-Configure of the same address does not SetupHook again");
    fn.BeginRemove();
    fn.BeginRemove();
    CHECK(fake.removals.size() == 3, "one Function id produces one RemoveHook");
    fake.FireLastCompletion();
    CHECK(S2Hook_DrainRetirement(), "drain after Function completion");
    CHECK(S2Hook_RetirementPending() == 0, "idempotent path leaves no pending retirement");

}

static void test_discarded_observe_does_not_leak_invocation() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto rec = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    (void)rec;
    // Intentionally discard: destructor still LeaveObserves. This is the
    // documented footgun (not a handler), not a leaked invocation hold.
    static_cast<void>(fn.Observe());
    CHECK(fn.Snapshot().state == S2HookState::Active, "discarded Observe still marks Active once");
    CHECK(S2Hook_DrainRetirement(), "discarded Observe does not leak callback-stack depth");

    Dummy obj;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
    (void)virt.Add(&obj);
    static_cast<void>(virt.Observe(&obj));
    CHECK(virt.Snapshot().state == S2HookState::Active, "discarded Virtual Observe still marks Active");
    CHECK(S2Hook_DrainRetirement(), "discarded Virtual Observe does not leak invocation");

    {
        auto obs = fn.Observe();
        auto moved = std::move(obs);
        CHECK(!obs, "moved-from Observe guard is disarmed");
        CHECK(static_cast<bool>(moved), "move transfers the invocation hold");
        CHECK(!S2Hook_DrainRetirement(), "moved-to guard still holds the callback stack");
        fn.BeginRemove();
        CHECK(S2Hook_RetirementPending() == 1, "BeginRemove sees the live guard's invocation");
        CHECK(!S2Hook_DrainRetirement(), "Drain still sees invocations while the guard is live");
    }
    CHECK(S2Hook_DrainRetirement(), "guard destructor releases the invocation hold");
    CHECK(S2Hook_RetirementPending() == 1, "pending remains until completion after the guard dies");
    fake.FireLastCompletion();
    CHECK(S2Hook_DrainRetirement(), "drain after discarded-then-guarded Observe completion");
    CHECK(S2Hook_RetirementPending() == 0, "completion clears retirement after guard release");
}

static void test_drain_true_can_leave_retirement_pending() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    (void)fn.Configure(reinterpret_cast<void*>(&FnTarget));
    fn.BeginRemove();
    CHECK(S2Hook_DrainRetirement(), "DrainRetirement true means not on a callback stack");
    CHECK(S2Hook_RetirementPending() == 1,
          "DrainRetirement true can still leave RetirementPending while completion is delayed");
    fake.FireLastCompletion();
    CHECK(S2Hook_DrainRetirement(), "drain after delayed completion");
    CHECK(S2Hook_RetirementPending() == 0, "pending drops only after Removed");
}

static void test_global_active_count_and_cross_thread_drain() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    (void)fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(S2Hook_ActiveCount() == 0, "no Observe means zero global active count");
    {
        auto obs = fn.Observe();
        CHECK(S2Hook_ActiveCount() >= 1, "Observe increments the global active-callback count");
        CHECK(!S2Hook_NoActiveDispatch(), "same-thread Observe is an active dispatch");
        std::atomic<bool> other_thread_drain{true};
        std::thread t([&] {
            other_thread_drain.store(S2Hook_DrainRetirement(), std::memory_order_relaxed);
        });
        t.join();
        CHECK(!other_thread_drain.load(std::memory_order_relaxed),
              "TLS depth alone is not a cross-thread drain");
    }
    CHECK(S2Hook_ActiveCount() == 0, "LeaveObserve drops the global active count");
    CHECK(S2Hook_DrainRetirement(), "drain is allowed after the Observe guard unwinds");

    {
        S2HookDispatchGuard g;
        CHECK(static_cast<bool>(g), "direct dispatch guard arms while Running");
        CHECK(S2Hook_ActiveCount() >= 1, "DispatchGuard holds a global active count");
        std::atomic<bool> other{true};
        std::thread t([&] { other.store(S2Hook_DrainRetirement(), std::memory_order_relaxed); });
        t.join();
        CHECK(!other.load(std::memory_order_relaxed),
              "ConCommand-style dispatch is visible to a cross-thread drain");
    }
    CHECK(S2Hook_NoActiveDispatch(), "DispatchGuard destructor releases the hold");
}

static int g_guarded_hook_calls = 0;
static int StubDispatchHook(int hookId, void* argView) {
    (void)hookId;
    (void)argView;
    ++g_guarded_hook_calls;
    CHECK(S2Hook_ActiveCount() >= 1, "guarded inbound dispatch is counted before JS");
    CHECK(!S2Hook_DrainRetirement(), "Unload cannot finish during guarded inbound JS");
    return 2;
}
static int StubDispatchHookPost(int hookId, void* argView, int skipped) {
    (void)hookId;
    (void)argView;
    (void)skipped;
    ++g_guarded_hook_calls;
    CHECK(S2Hook_ActiveCount() >= 1, "guarded inbound POST is counted before JS");
    return 1;
}

static void test_guarded_inbound_dispatch_stops_js_while_retiring() {
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
    g_guarded_hook_calls = 0;
    CHECK(S2Hook_GuardedDispatchHook(&StubDispatchHook, 7, nullptr) == 2,
          "Running inbound dispatch reaches JS");
    CHECK(g_guarded_hook_calls == 1, "Running inbound dispatch invoked the core op once");
    CHECK(S2Hook_NoActiveDispatch(), "guarded inbound dispatch releases the hold");

    S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
    CHECK(S2Hook_GuardedDispatchHook(&StubDispatchHook, 7, nullptr) == 0,
          "Retiring inbound dispatch returns Continue without JS");
    CHECK(S2Hook_GuardedDispatchHookPost(&StubDispatchHookPost, 7, nullptr, 0) == 0,
          "Retiring inbound POST returns Continue without JS");
    CHECK(g_guarded_hook_calls == 1, "Retiring does not invoke the raw core dispatch ops");
    CHECK(S2Hook_ActiveCount() == 0, "skipped inbound dispatch does not leak the active count");
    CHECK(S2Hook_GuardedDispatchHook(nullptr, 1, nullptr) == 0,
          "null core op degrades to Continue");
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
}

static void test_active_count_leave_does_not_underflow() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    (void)fn.Configure(reinterpret_cast<void*>(&FnTarget));

    constexpr int kIters = 20000;
    std::atomic<int> saw_negative{0};
    auto worker = [&]() {
        for (int i = 0; i < kIters; ++i) {
            {
                auto obs = fn.Observe();
                S2HookDispatchGuard g;
                if (S2Hook_ActiveCount() < 0) {
                    saw_negative.fetch_add(1, std::memory_order_relaxed);
                }
            }
            if (S2Hook_ActiveCount() < 0) {
                saw_negative.fetch_add(1, std::memory_order_relaxed);
            }
        }
    };
    std::thread a(worker);
    std::thread b(worker);
    a.join();
    b.join();
    CHECK(saw_negative.load(std::memory_order_relaxed) == 0,
          "LeaveObserve/DispatchGuard never take the active count negative");
    CHECK(S2Hook_ActiveCount() == 0, "paired leaves restore a zero active count");
    CHECK(S2Hook_DrainRetirement(), "drain is allowed after concurrent paired leaves");
}

static void test_callback_guard_rejects_dispatch_and_registration() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto rec = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(rec.Accepted(), "Configure is accepted while Running");
    Dummy obj;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
    CHECK(virt.Add(&obj).Accepted(), "Virtual Add is accepted while Running");

    S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
    {
        auto obs = fn.Observe();
        CHECK(static_cast<bool>(obs), "Observe still arms an owned id during retirement");
        CHECK(!S2Hook_EnterDispatch(obs),
              "call sites must honor a false EnterDispatch; constructing Observe is not enough");
        CHECK(!S2Hook_MayDispatch(), "JS dispatch is rejected while retiring");
    }
    {
        S2HookDispatchGuard g;
        CHECK(!g, "direct dispatch guard refuses to arm while retiring");
    }

    auto again = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(again.state == S2HookState::Failed, "Function Configure is rejected while retiring");
    CHECK(!again.reason.empty(), "rejected Configure while retiring has a named reason");
    CHECK(fn.Snapshot().id == rec.id, "rejected retiring Configure does not drop the owned id");

    Dummy extra;
    auto vadd = virt.Add(&extra);
    CHECK(vadd.state == S2HookState::Failed, "Virtual Add is rejected while retiring");
    CHECK(!vadd.reason.empty(), "rejected Add while retiring has a named reason");
    CHECK(fake.setup_hook_calls == 1, "retiring Configure does not SetupHook again");

    S2Hook_SetLifecycle(S2HookLifecycle::Running);
}

static void test_same_address_configure_is_idempotent() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto r1 = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    auto r2 = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(r1.Accepted() && r2.Accepted(), "same Function address yields an accepted receipt");
    CHECK(r1.id == r2.id, "same Function address keeps the existing id");
    CHECK(fake.setup_hook_calls == 1, "same-address Configure does not SetupHook again");
    CHECK(fn.Snapshot().id == r1.id, "snapshot stays on the existing accepted receipt");
    {
        auto obs = fn.Observe();
        auto r3 = fn.Configure(reinterpret_cast<void*>(&FnTarget));
        CHECK(r3.id == r1.id, "same-address Configure while Active is still the existing receipt");
        CHECK(fake.setup_hook_calls == 1, "Active same-address Configure does not SetupHook");
    }
}

static void test_different_address_configure_rejected_old_id_still_retires() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto r1 = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(r1.Accepted(), "first Function address is accepted");
    const auto removals_before = fake.removals.size();
    auto bad = fn.Configure(reinterpret_cast<void*>(&FnTargetB));
    CHECK(bad.state == S2HookState::Failed, "different Function address while owned is Failed");
    CHECK(!bad.Accepted(), "retarget receipt is not Accepted");
    CHECK(!bad.reason.empty(), "retarget failure is named");
    CHECK(fn.Snapshot().id == r1.id, "owned id is unchanged after rejected retarget");
    CHECK(fn.Snapshot().state == S2HookState::Pending || fn.Snapshot().state == S2HookState::Active,
          "rejected retarget does not mark the old binding Failed");
    CHECK(fake.setup_hook_calls == 1, "rejected retarget does not SetupHook a second address");
    CHECK(fake.removals.size() == removals_before,
          "rejected retarget does not silently RemoveHook the owned id");

    fn.BeginRemove();
    CHECK(fn.Snapshot().state == S2HookState::Removing, "old binding still enters Removing");
    CHECK(fake.removals.size() == removals_before + 1, "BeginRemove retires the original id once");
    fake.FireLastCompletion();
    CHECK(S2Hook_DrainRetirement(), "drain after rejected-retarget retirement");
    CHECK(fn.Snapshot().state == S2HookState::Removed, "old binding reaches Removed");
    CHECK(S2Hook_RetirementPending() == 0, "ownership is not stuck on a silently removed id");

    auto rnew = fn.Configure(reinterpret_cast<void*>(&FnTargetB));
    CHECK(rnew.Accepted(), "a completed old binding may be replaced by a new address");
    CHECK(rnew.id != r1.id, "replacement uses a new KHook id");
}

static void test_last_subscriber_removed_still_physically_retired() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);

    Dummy obj;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, &DummyPost);
    auto rec = virt.Add(&obj);
    CHECK(rec.Accepted(), "kind-level Virtual accepted a subscriber");
    virt.Remove(&obj);
    CHECK(!virt.HasThisFilter(&obj), "last subscriber already removed the this-filter");
    CHECK(virt.Snapshot().id == rec.id, "physical id is still owned after last-subscriber Remove");
    CHECK(fake.removals.empty(), "filter drop is not physical removal");
    virt.BeginRemove();
    CHECK(fake.removals.size() == 1, "empty subscriber rows still BeginRemove the kind-level object");
    CHECK(virt.Snapshot().state == S2HookState::Removing, "kind-level object is Removing");
    virt.BeginRemove();
    CHECK(fake.removals.size() == 1, "repeated BeginRemove on an empty-row object is idempotent");
    fake.FireLastCompletion();
    CHECK(S2Hook_DrainRetirement(), "drain after empty-row physical retirement");
    CHECK(S2Hook_RetirementPending() == 0, "empty-row retirement completes");
    CHECK(virt.Snapshot().state == S2HookState::Removed, "kind-level object reaches Removed");
}

static void test_deferred_completion_retry_shape() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    (void)fn.Configure(reinterpret_cast<void*>(&FnTarget));
    fn.BeginRemove();
    CHECK(S2Hook_DrainRetirement(), "first retry drain is allowed off the callback stack");
    CHECK(S2Hook_RetirementPending() == 1, "first retry still sees pending completion");
    const auto removals_before_sync_upgrade = fake.removals.size();
    CHECK(!fn.BeginRemove(false), "pending async removal cannot be upgraded to synchronous");
    CHECK(fake.removals.size() == removals_before_sync_upgrade,
          "rejected sync upgrade does not duplicate the provider removal");
    auto late = fn.Configure(reinterpret_cast<void*>(&FnTargetB));
    CHECK(late.state == S2HookState::Failed, "Configure while Removing is a named failure");
    CHECK(fn.Snapshot().state == S2HookState::Removing, "failed Configure leaves Removing intact");
    fake.FireLastCompletion();
    CHECK(S2Hook_DrainRetirement(), "external retry drain after late completion");
    CHECK(S2Hook_RetirementPending() == 0, "retry sees no pending retirement after completion");
}

// Removing completed-ID cleanup would make base ~Virtual call the provider
// again for every historical ID, despite physical removal already finishing.
static void test_completed_virtual_destruction_does_not_remove_again() {
    FakeKHook fake;
    fake.invoke_typed_removal = true;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
    Dummy obj;
    {
        S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
        (void)virt.Add(&obj);
        virt.Remove(&obj);
        virt.BeginRemove();
        fake.FireLastCompletion();
        CHECK(virt.Snapshot().state == S2HookState::Removed,
              "actual Virtual removal helper and checked receipt finished");
        CHECK(S2Hook_DrainRetirement() && S2Hook_RetirementPending() == 0,
              "completed Virtual with empty filters has drained");
    }
    CHECK(fake.removals.size() == 1,
          "completed Virtual destruction makes no second provider removal");
}

static void test_virtual_destruction_keeps_live_id_cleanup() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
    Dummy obj;
    KHook::HookID_t id;
    {
        S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
        id = virt.Add(&obj).id;
    }
    CHECK(fake.removals.size() == 1 && fake.removals[0].id == id && !fake.removals[0].async,
          "live Virtual ID retains the base helper's synchronous destructor cleanup");
}

static void test_synchronous_remove_completes_without_retirement_queue() {
    FakeKHook fake;
    fake.invoke_typed_removal = true;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
    Dummy obj;
    {
        S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, &DummyPost);
        const auto rec = virt.Add(&obj);
        {
            auto observation = virt.Observe(&obj);
            CHECK(!virt.BeginRemove(false),
                  "synchronous BeginRemove rejects an active callback stack");
            CHECK(fake.removals.empty(),
                  "rejected synchronous BeginRemove does not mutate provider state");
        }
        virt.Remove(&obj);
        CHECK(virt.BeginRemove(false),
              "synchronous BeginRemove is accepted after the callback unwinds");
        CHECK(fake.removals.size() == 1 && fake.removals[0].id == rec.id,
              "synchronous BeginRemove removes the checked physical id once");
        CHECK(!fake.removals[0].async,
              "synchronous BeginRemove requests an off-stack provider removal");
        CHECK(virt.Snapshot().state == S2HookState::Removed,
              "synchronous provider completion marks the checked binding Removed");
        CHECK(S2Hook_RetirementPending() == 0,
              "synchronous BeginRemove creates no delayed retirement entry");
    }
    CHECK(fake.removals.size() == 1,
          "completed synchronous removal leaves no live id for the base destructor");
}

static void test_terminal_permit_excludes_current_capsule() {
    FakeKHook fake;
    fake.invoke_typed_removal = true;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
    Dummy obj;
    S2CheckedVirtual<Dummy, void> marker(0u, &DummyPre, nullptr);
    S2CheckedVirtual<Dummy, void> alias(0u, &DummyPre, nullptr);
    S2CheckedVirtual<Dummy, void> other(1u, &DummyPre, nullptr);
    marker.Add(&obj); alias.Add(&obj); other.Add(&obj);
    {
        auto observation = marker.Observe(&obj);
        {
            S2HookDispatchGuard concurrent_direct;
            CHECK(!S2Hook_CurrentTerminalPermit().IsValid(),
                  "direct dispatch beside the marker blocks a terminal permit");
        }
        S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
        const auto permit = S2Hook_CurrentTerminalPermit();
        CHECK(permit.IsValid(), "sole observed terminal marker yields a permit");
        CHECK(!alias.CanBeginRemove(false, &permit),
              "terminal preflight rejects a separate wrapper on the current capsule");
        CHECK(other.CanBeginRemove(false, &permit),
              "terminal preflight permits a distinct vtable slot");
        const auto before = fake.removals.size();
        CHECK(!alias.BeginRemove(false, &permit),
              "same-capsule terminal removal is rejected");
        CHECK(fake.removals.size() == before,
              "same-capsule rejection performs no provider removal");
        other.Remove(&obj);
        CHECK(other.BeginRemove(false, &permit),
              "distinct-capsule terminal removal completes under the marker");
    }
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
}

static void test_inventory_requires_provider_completion() {
    FakeKHook fake;
    fake.invoke_typed_removal = true;
    fake.complete_sync = false;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
    S2CheckedFunction<void> marker(&FnPre, nullptr);
    S2CheckedFunction<void> first(&FnPre, nullptr);
    S2CheckedFunction<void> second(&FnPre, nullptr);
    marker.Configure(reinterpret_cast<void*>(&FnTarget));
    first.Configure(reinterpret_cast<void*>(&FnTargetB));
    second.Configure(reinterpret_cast<void*>(&FnTargetC));
    const std::array<S2CheckedBindingOps*, 2> inventory{{&first, &second}};
    {
        auto observation = marker.Observe();
        S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
        const auto permit = S2Hook_CurrentTerminalPermit();
        CHECK(S2HookInventoryCanRemoveSync(inventory, permit),
              "injected inventory preflights every distinct capsule");
        CHECK(!S2HookInventoryBeginRemoveSync(inventory, permit),
              "withheld synchronous completion rejects inventory success");
        CHECK(fake.removals.size() == 2,
              "injected inventory attempted every preflighted binding");
        CHECK(!S2HookInventoryRemovalComplete(inventory),
              "inventory remains incomplete until provider callbacks arrive");
        fake.FireCompletion(0);
        fake.FireCompletion(1);
        CHECK(S2HookInventoryRemovalComplete(inventory),
              "all provider callbacks complete the injected inventory");
    }
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
}

static void test_inventory_preflight_prevents_partial_mutation() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
    S2CheckedFunction<void> marker(&FnPre, nullptr);
    S2CheckedFunction<void> eligible(&FnPre, nullptr);
    S2CheckedFunction<void> same_capsule(&FnPre, nullptr);
    marker.Configure(reinterpret_cast<void*>(&FnTarget));
    eligible.Configure(reinterpret_cast<void*>(&FnTargetB));
    same_capsule.Configure(reinterpret_cast<void*>(&FnTarget));
    const std::array<S2CheckedBindingOps*, 2> inventory{{&eligible, &same_capsule}};
    {
        auto observation = marker.Observe();
        S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
        const auto permit = S2Hook_CurrentTerminalPermit();
        CHECK(!S2HookInventoryBeginRemoveSync(inventory, permit),
              "late ineligible inventory binding rejects the whole mutation");
        CHECK(fake.removals.empty(),
              "inventory performs zero backend removals when preflight fails late");
        CHECK(eligible.Snapshot().state == S2HookState::Pending,
              "eligible prefix remains owned after inventory preflight failure");
    }
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
}

static void test_fourteen_binding_inventory_visits_every_kind() {
    FakeKHook fake;
    fake.invoke_typed_removal = true;
    KHook::__exported__khook = &fake;
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
    S2CheckedFunction<void> marker(&FnPre, nullptr);
    marker.Configure(reinterpret_cast<void*>(&FnTarget));
    std::array<std::unique_ptr<S2CheckedFunction<void>>, 14> owned;
    const std::array<void*, 14> targets{{
        reinterpret_cast<void*>(&InventoryTarget<0>), reinterpret_cast<void*>(&InventoryTarget<1>),
        reinterpret_cast<void*>(&InventoryTarget<2>), reinterpret_cast<void*>(&InventoryTarget<3>),
        reinterpret_cast<void*>(&InventoryTarget<4>), reinterpret_cast<void*>(&InventoryTarget<5>),
        reinterpret_cast<void*>(&InventoryTarget<6>), reinterpret_cast<void*>(&InventoryTarget<7>),
        reinterpret_cast<void*>(&InventoryTarget<8>), reinterpret_cast<void*>(&InventoryTarget<9>),
        reinterpret_cast<void*>(&InventoryTarget<10>), reinterpret_cast<void*>(&InventoryTarget<11>),
        reinterpret_cast<void*>(&InventoryTarget<12>), reinterpret_cast<void*>(&InventoryTarget<13>),
    }};
    std::array<S2CheckedBindingOps*, 14> inventory{};
    for (std::size_t i = 0; i < owned.size(); ++i) {
        owned[i] = std::make_unique<S2CheckedFunction<void>>(&FnPre, nullptr);
        owned[i]->Configure(targets[i]);
        inventory[i] = owned[i].get();
    }
    {
        auto observation = marker.Observe();
        S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
        const auto permit = S2Hook_CurrentTerminalPermit();
        CHECK(S2HookInventoryCanRemoveSync(inventory, permit),
              "fourteen-kind inventory preflights as one owned set");
        const auto before = fake.removals.size();
        CHECK(S2HookInventoryBeginRemoveSync(inventory, permit),
              "fourteen-kind inventory retires with provider completion");
        CHECK(fake.removals.size() - before == 14,
              "fourteen-kind inventory visits every binding exactly once");
        CHECK(S2HookInventoryRemovalComplete(inventory),
              "fourteen-kind inventory verifies every completion");
    }
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
}

}  // namespace

int main() {
    test_action_helpers();
    test_setup_invalid_is_failed_named_no_filter();
    test_valid_id_is_pending_until_observe();
    test_first_matching_callback_becomes_active_once();
    test_pre_post_share_one_virtual_id();
    test_both_unsubscribe_orders_keep_filter_until_last_remove();
    test_unsubscribe_in_callback_drops_filter_without_destroying();
    test_delayed_completion_keeps_context();
    test_idempotent_add_one_completion_per_id();
    test_discarded_observe_does_not_leak_invocation();
    test_drain_true_can_leave_retirement_pending();
    test_global_active_count_and_cross_thread_drain();
    test_guarded_inbound_dispatch_stops_js_while_retiring();
    test_active_count_leave_does_not_underflow();
    test_callback_guard_rejects_dispatch_and_registration();
    test_same_address_configure_is_idempotent();
    test_different_address_configure_rejected_old_id_still_retires();
    test_last_subscriber_removed_still_physically_retired();
    test_deferred_completion_retry_shape();
    test_completed_virtual_destruction_does_not_remove_again();
    test_virtual_destruction_keeps_live_id_cleanup();
    test_synchronous_remove_completes_without_retirement_queue();
    test_terminal_permit_excludes_current_capsule();
    test_inventory_requires_provider_completion();
    test_inventory_preflight_prevents_partial_mutation();
    test_fourteen_binding_inventory_visits_every_kind();

    if (g_fail) {
        std::cerr << g_fail << " check(s) failed\n";
        return 1;
    }
    std::cout << "khook_binding_test: all checks passed\n";
    return 0;
}
