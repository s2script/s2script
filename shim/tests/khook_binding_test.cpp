// Host test for the checked KHook binding (shim/src/khook_binding.h).
//
// Injects KHook::IKHook via KHook::__exported__khook. This proves registration
// receipts, Observe/BeginRemove bookkeeping, this-filter removal, and
// completion-aware retirement. It does not prove native trampolines.
//
// Public FFI conventions (unchanged; not exercised here): SDKHooks VP add uses
// nonzero success; declarative S2_HookInstall uses 0 success / -1 failure.
#include "khook_map.h"

#include <cstdint>
#include <iostream>
#include <string>
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
        if (fail_setup) {
            return KHook::INVALID_HOOK;
        }
        return next_id++;
    }

    KHook::HookID_t SetupVirtualHook(void**, int, void*, void*, void*, void*, void*, void*,
                                    unsigned int, bool = false) override {
        ++setup_virtual_calls;
        if (fail_setup) {
            return KHook::INVALID_HOOK;
        }
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
    fn.Observe();
    CHECK(fn.Snapshot().id == rec.id, "Observe keeps the same KHook id");
    CHECK(fn.Snapshot().state == S2HookState::Active, "first matching Function callback is Active");
    fn.Observe();
    CHECK(fn.Snapshot().state == S2HookState::Active, "Observe is Active once, not a new id");
    fn.EndObserve();
    fn.EndObserve();

    Dummy obj;
    Dummy other;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
    auto vrec = virt.Add(&obj);
    virt.Observe(&other);
    CHECK(virt.Snapshot().state == S2HookState::Pending, "unmatched this pointer does not activate");
    virt.Observe(&obj);
    CHECK(virt.Snapshot().id == vrec.id, "matching Observe keeps the same id");
    CHECK(virt.Snapshot().state == S2HookState::Active, "first matching Virtual callback is Active");
    virt.Observe(&obj);
    CHECK(virt.Snapshot().state == S2HookState::Active, "Virtual Observe is Active once");
    virt.EndObserve();
    virt.EndObserve();

}

static void test_pre_post_share_one_virtual_id() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    Dummy obj;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, &DummyPost);
    auto pre = virt.Add(&obj);
    CHECK(fake.setup_virtual_calls == 1, "PRE+POST share one SetupVirtualHook");
    auto post = virt.Add(&obj);
    CHECK(post.id == pre.id, "PRE/POST re-add reuses the same KHook id");
    CHECK(fake.setup_virtual_calls == 1, "re-add does not install a second physical hook");
    CHECK(virt.HasThisFilter(&obj), "removing PRE (T5 leaves POST live) retains the this-filter");
    virt.Remove(&obj);
    CHECK(!virt.HasThisFilter(&obj), "last phase drops the this-filter immediately");
    CHECK(virt.Snapshot().id == pre.id, "physical id is retained after last-phase filter drop");
    CHECK(virt.Snapshot().state != S2HookState::Removed,
          "filter drop is not physical removal");
    CHECK(fake.removals.empty(), "phase/filter drop does not call RemoveHook");

}

static void test_unsubscribe_in_callback_drops_filter_without_destroying() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    Dummy obj;
    S2CheckedVirtual<Dummy, void> virt(0u, &DummyPre, nullptr);
    auto rec = virt.Add(&obj);
    virt.Observe(&obj);
    virt.Remove(&obj);
    CHECK(!virt.HasThisFilter(&obj), "unsubscribe in callback drops the this-filter immediately");
    CHECK(virt.Snapshot().state != S2HookState::Removed, "unsubscribe is not synchronous destruction");
    CHECK(virt.Snapshot().id == rec.id, "binding still owns the physical id after filter drop");
    CHECK(fake.removals.empty(), "in-callback unsubscribe does not call RemoveHook");
    CHECK(!S2Hook_DrainRetirement(), "Unload/drain on an active callback stack is rejected");
    virt.EndObserve();
    CHECK(S2Hook_DrainRetirement(), "drain is allowed once the callback stack unwinds");

}

static void test_delayed_completion_keeps_context() {
    FakeKHook fake;
    KHook::__exported__khook = &fake;

    bool died = false;
    auto* fn = new WatchFn(&FnPre, nullptr);
    fn->died = &died;
    auto rec = fn->Configure(reinterpret_cast<void*>(&FnTarget));
    fn->Observe();
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

    fn->EndObserve();
    CHECK(fn->Snapshot().state == S2HookState::Removed,
          "Removed only after completion and invocation release");
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
    glob.EndObserve();

    S2CheckedFunction<void> fn(&FnPre, nullptr);
    auto f1 = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    auto f2 = fn.Configure(reinterpret_cast<void*>(&FnTarget));
    CHECK(f1.id == f2.id, "idempotent Function Configure keeps one id");
    CHECK(fake.setup_hook_calls == 1, "re-Configure of the same address does not SetupHook again");
    fn.BeginRemove();
    fn.BeginRemove();
    CHECK(fake.removals.size() == 3, "one Function id produces one RemoveHook");

}

}  // namespace

int main() {
    test_action_helpers();
    test_setup_invalid_is_failed_named_no_filter();
    test_valid_id_is_pending_until_observe();
    test_first_matching_callback_becomes_active_once();
    test_pre_post_share_one_virtual_id();
    test_unsubscribe_in_callback_drops_filter_without_destroying();
    test_delayed_completion_keeps_context();
    test_idempotent_add_one_completion_per_id();

    if (g_fail) {
        std::cerr << g_fail << " check(s) failed\n";
        return 1;
    }
    std::cout << "khook_binding_test: all checks passed\n";
    return 0;
}
