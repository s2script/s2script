// Real bounded ABI tests. The portable mode checks normalization only; it is never
// reported as a CIF, stock-provider, V8, or live-server proof.
#include "engine_function_abi.h"
#include <cassert>
#include <iostream>
using namespace s2fn;
static void signatures() {
    AbiSignature s;
    s.parameters = {{"i32"}, {"f64"}, {"ptr"}};
    s.returns = {"u8", "bool"};
    auto info = Validate(s);
    assert(info && info.value.fingerprint == "linux-x86_64-sysv:none:u8(i32,f64,ptr)");
    assert(info.value.stack_bytes == 128);
    auto different = s; different.receiver = "entity";
    assert(Validate(different).value.fingerprint != info.value.fingerprint);
    different = s; different.returns = {"u32"};
    assert(Validate(different).value.fingerprint != info.value.fingerprint);
    different = s; std::swap(different.parameters[0], different.parameters[1]);
    assert(Validate(different).value.fingerprint != info.value.fingerprint);
    different = s; different.parameters[0] = {"i64"};
    assert(Validate(different).value.fingerprint != info.value.fingerprint);
    for (const auto& atom : {"u8", "i32", "u32", "i64", "u64", "f32", "f64", "ptr"}) {
        AbiSignature vector; vector.parameters.assign(32, {atom, std::string(atom) == "u8" ? "bool" : ""});
        assert(Validate(vector));
    }
    auto reject = [](AbiSignature bad, const char* feature) {
        auto r = Validate(bad); assert(!r); assert(r.error.find(feature) != std::string::npos);
    };
    different = s; different.platform = "windows"; different.receiver = "struct"; reject(different, "platform: windows");
    different = s; different.receiver = "struct"; reject(different, "receiver: struct");
    different = s; different.varargs = true; reject(different, "varargs");
    different = s; different.returns = {"aggregate"}; reject(different, "return: aggregate");
    different = s; different.parameters[0] = {"i16"}; reject(different, "parameter[0]: i16");
    different = s; different.parameters[0] = {"u8", "integer"}; reject(different, "u8 projection: integer");
    different = s; different.parameters.assign(33, {"i32"}); reject(different, "parameter count");
    s = {}; s.parameters.assign(6, {"i64"}); assert(Validate(s).value.stack_bytes == 128);
    s.parameters.assign(32, {"i64"}); assert(Validate(s).value.stack_bytes == 208);
    s.receiver = "entity"; assert(Validate(s).value.stack_bytes == 224);
    s = {}; s.parameters.assign(32, {"f64"}); assert(Validate(s).value.stack_bytes == 192);
    s = {}; for (int i = 0; i < 16; ++i) { s.parameters.push_back({"f64"}); s.parameters.push_back({"i64"}); }
    assert(Validate(s).value.stack_bytes == 144);
    assert(!StackCopyBytes(33)); // 264 bytes exceeds the independent stack budget.
    assert(StackCopyBytes(32).value == 256);
    auto b = NativeValue::From<std::uint8_t>(1); assert(b.Get<std::uint8_t>() == 1);
    assert(b.bytes[1] == 0 && b.bytes[7] == 0);
    std::cout << "PASS bounded signatures, named refusals, SysV stack-copy bounds\n";
}

#ifndef S2FN_VALIDATION_ONLY
#include <atomic>
#include <chrono>
#include <thread>
#include <functional>
#include <csignal>
#include <sys/wait.h>
#include <unistd.h>
#include <type_traits>
#include <condition_variable>
#include "engine_function_fixture_guard.h"
// Allocation fault injection wraps only the allocator; all successful closures,
// CIFs, provider detours and callbacks are the pinned real implementations.
static int fail_allocation = -1, allocations = 0, frees = 0;
extern "C" void* __real_ffi_closure_alloc(std::size_t, void**);
extern "C" void __real_ffi_closure_free(void*);
extern "C" void* __wrap_ffi_closure_alloc(std::size_t n, void** code) {
    if (fail_allocation == 0) return nullptr;
    if (fail_allocation > 0) --fail_allocation;
    void* result = __real_ffi_closure_alloc(n, code); if (result) ++allocations; return result;
}
extern "C" void __wrap_ffi_closure_free(void* p) { if (p) ++frees; __real_ffi_closure_free(p); }
struct Sink : DispatchSink {
    std::function<void(DispatchFrame&)> dispatch;
    int pre = 0, post = 0, errors = 0;
    std::string last_error;
    void Dispatch(DispatchFrame& f) override {
        if (f.phase == Phase::Pre) ++pre; else ++post;
        if (dispatch) dispatch(f);
    }
    void Error(const char* s) noexcept override { ++errors; last_error = s; }
};
static void retire(std::unique_ptr<RuntimeBinding>& b) {
    const int prior = frees;
    b->BeginRemove(); b->BeginRemove();
    assert(frees == prior); // OnKHookRemoved never frees the closure or binding.
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
    while (!b->RemovalComplete() && std::chrono::steady_clock::now() < deadline)
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(b->RemovalComplete()); assert(S2Hook_DrainRetirement());
    b.reset(); assert(frees == prior + 4);
}
// Counter storage stays with the assertions; native bodies live only in the DSO.
static volatile std::uint64_t original_calls = 0;
__attribute__((noinline)) static std::int32_t local_dummy_target(std::int32_t value) { return value; }
static S2FnFixtureBoundary fixture_boundary() {
    return {reinterpret_cast<const void*>(&s2fn_fixture_targets),
        reinterpret_cast<const void*>(&KHook::Shutdown), reinterpret_cast<const void*>(&signatures)};
}
static const S2FnFixtureTargets& fixture_targets() {
    static const auto targets = [] {
        const auto boundary = fixture_boundary();
        // Regression: old consumer-local targets must be refused BEFORE Configure.
        assert(!boundary.Accepts(reinterpret_cast<const void*>(&local_dummy_target)));
        auto resolved = s2fn_fixture_targets();
        S2FnRequireFixtureInventory(resolved, boundary);
        s2fn_fixture_set_original_calls(&original_calls);
        return resolved;
    }();
    return targets;
}
static const void* checked_target(const void* target) {
    (void)fixture_targets();
    fixture_boundary().Require("hook-install", target);
    return target;
}
// This selects pointers; it never defines or instantiates a target body here.
template<class T> static auto identity_target() {
    const auto& t = fixture_targets();
    if constexpr (std::is_same_v<T, bool>) return t.identity_bool;
    else if constexpr (std::is_same_v<T, std::uint8_t>) return t.identity_u8;
    else if constexpr (std::is_same_v<T, std::int32_t>) return t.identity_i32;
    else if constexpr (std::is_same_v<T, std::uint32_t>) return t.identity_u32;
    else if constexpr (std::is_same_v<T, std::int64_t>) return t.identity_i64;
    else if constexpr (std::is_same_v<T, std::uint64_t>) return t.identity_u64;
    else if constexpr (std::is_same_v<T, float>) return t.identity_f32;
    else if constexpr (std::is_same_v<T, double>) return t.identity_f64;
    else { static_assert(std::is_same_v<T, void*>); return t.identity_ptr; }
}
static std::unique_ptr<RuntimeBinding> bind(AbiSignature s, Sink& sink, const void* target) {
    target = checked_target(target);
    std::cout << "event=create-begin target=" << target << "\n";
    auto r = RuntimeBinding::Create(std::move(s), sink); assert(r);
    std::cout << "vector=" << r.value->Info().fingerprint << " stack=" << r.value->Info().stack_bytes << " closures=4\n";
    std::cout << "event=configure-begin\n";
    assert(r.value->Configure(target).Accepted());
    std::cout << "event=configure-returned\n";
    return std::move(r.value);
}
template<class T> static void atom(const char* name, T value) {
    AbiSignature s; s.parameters = {{name, std::string(name) == "u8" ? "bool" : ""}}; s.returns = s.parameters[0];
    Sink sink; sink.dispatch = [&](DispatchFrame& f) {
        assert(f.arguments[0].Get<T>() == value);
        if (f.phase == Phase::Post) assert(f.result.Get<T>() == value);
    };
    auto b = bind(s, sink, reinterpret_cast<void*>(identity_target<T>()));
    NativeValue v = NativeValue::From(value); std::fill(v.bytes.begin()+sizeof(T), v.bytes.end(), 0xa5);
    auto r = b->Call(&v, 1); assert(r); assert(r.value.Get<T>() == value);
    if (std::string(name) == "u8") {
        for (std::size_t i = 1; i < r.value.bytes.size(); ++i) assert(r.value.bytes[i] == 0);
        NativeValue invalid = NativeValue::From<std::uint8_t>(2);
        assert(!b->Call(&invalid, 1));
    }
    assert(sink.pre == 1 && sink.post == 1 && sink.errors == 0);
    retire(b);
}

template<class T, std::size_t... I> static void spill_case(const char* name, std::index_sequence<I...>) {
    AbiSignature s; s.returns = {name}; s.parameters.assign(sizeof...(I), {name});
    std::vector<NativeValue> values{NativeValue::From<T>(static_cast<T>(I + 1))...};
    Sink sink;
    auto b = bind(s, sink, reinterpret_cast<void*>([] {
        static_assert(sizeof...(I) == 32);
        if constexpr (std::is_same_v<T, std::int64_t>) return fixture_targets().spill_gp;
        else { static_assert(std::is_same_v<T, double>); return fixture_targets().spill_sse; }
    }()));
    auto result = b->Call(values.data(), values.size());
    assert(result && result.value.Get<T>() == static_cast<T>(11440)); // sum of squares 1..32
    retire(b);
}
static void queued_remove_and_destroy_refusal() {
    AbiSignature s; s.parameters = {{"i32"}}; s.returns = {"i32"}; Sink sink;
    auto b = bind(s, sink, reinterpret_cast<void*>(identity_target<std::int32_t>()));
    const pid_t child = fork(); assert(child >= 0);
    if (child == 0) { std::set_terminate([] { _exit(86); }); b.reset(); _exit(0); }
    int status = 0; assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 86);
    // The native target's shared callback lock makes stock provider insertion
    // queue deterministically. Remove before that queue can acquire the lock.
    std::unique_ptr<S2CheckedFunction<std::int32_t, std::int32_t>> queued;
    sink.dispatch = [&](DispatchFrame& f) {
        if (f.phase != Phase::Pre) return;
        using Checked = S2CheckedFunction<std::int32_t, std::int32_t>;
        using Callback = KHook::Return<std::int32_t>(*)(std::int32_t);
        queued = std::make_unique<Checked>(static_cast<Callback>(nullptr), static_cast<Callback>(nullptr));
        auto receipt = queued->Configure(checked_target(reinterpret_cast<void*>(identity_target<std::int32_t>())));
        assert(receipt.Accepted() && receipt.state == S2HookState::Pending);
        queued->BeginRemove(); queued->BeginRemove();
        assert(queued->RemovalComplete());
    };
    auto value = NativeValue::From<std::int32_t>(17); auto r = b->Call(&value, 1);
    assert(r && r.value.Get<std::int32_t>() == 17); queued.reset(); retire(b);
}
static void reject_noncanonical_output() {
    AbiSignature s; s.parameters = {{"u8", "bool"}}; s.returns = {"u8", "bool"}; Sink sink;
    auto b = bind(s, sink, reinterpret_cast<void*>(fixture_targets().noncanonical));
    auto input = NativeValue::From<std::uint8_t>(0);
    auto result = b->Call(&input, 1);
    assert(!result && result.error.find("noncanonical u8 return") != std::string::npos);
    assert(sink.errors > 0); retire(b);
}
namespace s2fn {
struct RuntimeBindingTestAccess {
    static bool BothAcknowledged(RuntimeBinding& binding) {
        if (!binding.provider_detached_.load(std::memory_order_acquire)) return false;
        std::lock_guard<std::mutex> lock(binding.state_->mu);
        for (const auto& item : binding.state_->owned) if (!item.second.complete) return false;
        return true;
    }
};
static std::function<void(RuntimeBinding*)> return_unlocked, invoke_returned, before_invocation_id;
void TestBeforeInvocationId(RuntimeBinding* binding) { if(before_invocation_id)before_invocation_id(binding); }
void TestReturnUnlocked(RuntimeBinding* binding) { if (return_unlocked) return_unlocked(binding); }
void TestInvokeReturned(RuntimeBinding* binding) { if (invoke_returned) invoke_returned(binding); }
}
static void return_phase_lifetime(bool outbound) {
    AbiSignature signature; signature.parameters = {{"u8", "bool"}}; signature.returns = {"u8", "bool"};
    Sink sink;
    auto binding = bind(signature, sink, reinterpret_cast<void*>(identity_target<bool>()));
    sink.dispatch = [&](DispatchFrame& frame) { if (frame.phase == Phase::Pre) binding->BeginRemove(); };
    std::mutex mutex; std::condition_variable condition; bool unlocked = false, resume = false, invoke_done = false, resume_invoke = false;
    std::thread::id outbound_thread;
    s2fn::return_unlocked = [&](RuntimeBinding* observed) {
        assert(observed == binding.get());
        std::unique_lock<std::mutex> lock(mutex); unlocked = true; condition.notify_all();
        assert(condition.wait_for(lock, std::chrono::seconds(5), [&] { return resume; }));
    };
    s2fn::invoke_returned = [&](RuntimeBinding* observed) {
        std::unique_lock<std::mutex> lock(mutex);
        if (!outbound || !resume || std::this_thread::get_id()!=outbound_thread) return; // ignore MakeOriginal and the reuse probe
        assert(observed == binding.get()); invoke_done = true; condition.notify_all();
        assert(condition.wait_for(lock, std::chrono::seconds(5), [&] { return resume_invoke; }));
    };
    const int before_frees = frees; bool result = false;
    // Enter from an independent native caller, not RuntimeBinding::Call: its
    // lifetime must be protected even without an outbound call lease.
    std::thread caller([&] {
        outbound_thread=std::this_thread::get_id();
        if (outbound) {
            auto input = NativeValue::From<std::uint8_t>(1);
            const auto returned = binding->Call(&input, 1); assert(returned);
            result = returned.value.Get<std::uint8_t>() == 1;
        } else { auto volatile target = identity_target<bool>(); result = target(true); }
    });
    {
        std::unique_lock<std::mutex> lock(mutex);
        assert(condition.wait_for(lock, std::chrono::seconds(5), [&] { return unlocked; }));
    }
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
    while (!RuntimeBindingTestAccess::BothAcknowledged(*binding) && std::chrono::steady_clock::now() < deadline)
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(RuntimeBindingTestAccess::BothAcknowledged(*binding));
    assert(!binding->RemovalComplete()); // RED: provider acknowledgements are not a return-phase lease.
    assert(frees == before_frees);
    { std::lock_guard<std::mutex> lock(mutex); resume = true; } condition.notify_all();
    if (outbound) {
        std::unique_lock<std::mutex> lock(mutex);
        assert(condition.wait_for(lock, std::chrono::seconds(5), [&] { return invoke_done; }));
        assert(RuntimeBindingTestAccess::BothAcknowledged(*binding));
        assert(!binding->RemovalComplete() && frees == before_frees);
        lock.unlock();
        // Both removal acknowledgements are complete, but the first outbound
        // Call still owns its lease. A second call must not reuse this target.
        auto input=NativeValue::From<std::uint8_t>(1);
        const auto early=binding->Call(&input,1);
        assert(!early && early.error=="binding not callable");
        lock.lock();
        resume_invoke = true; lock.unlock(); condition.notify_all();
    }
    caller.join(); s2fn::return_unlocked = {}; s2fn::invoke_returned = {};
    assert(result && sink.errors == 0 && sink.pre == 1 && sink.post == 1);
    if (outbound) {
        assert(binding->RemovalComplete());
        auto input=NativeValue::From<std::uint8_t>(1);
        const auto reused=binding->Call(&input,1);
        assert(reused && reused.value.Get<std::uint8_t>()==1);
    }
    retire(binding);
    std::cout << "PASS return phase held across both provider acknowledgements; outbound=" << outbound << " caller finished before reclamation\n";
}
static void completed_detachment_allows_nested_calls() {
    AbiSignature signature;signature.parameters={{"i32"}};signature.returns={"i32"};
    Sink detached_sink, hooked_sink;
    auto detached=bind(signature,detached_sink,reinterpret_cast<void*>(fixture_targets().detached_nested));
    detached->BeginRemove();
    const auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while (!detached->RemovalComplete() && std::chrono::steady_clock::now()<deadline)
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(detached->RemovalComplete() && detached->Receipt().state==S2HookState::Removed);
    assert(S2Hook_DrainRetirement());
    auto hooked=bind(signature,hooked_sink,reinterpret_cast<void*>(identity_target<std::int32_t>()));
    int nested_calls=0;
    hooked_sink.dispatch=[&](DispatchFrame& frame) {
        if (frame.phase!=Phase::Pre || frame.arguments[0].Get<std::int32_t>()!=1) return;
        // Detached F(1) -> hooked G(1) -> F(0) -> G(0), bounded by its input.
        auto input=NativeValue::From<std::int32_t>(0);
        const auto nested=detached->Call(&input,1);
        assert(nested && nested.value.Get<std::int32_t>()==1);
        ++nested_calls;
    };
    auto input=NativeValue::From<std::int32_t>(1);
    const auto result=detached->Call(&input,1);
    assert(result && result.value.Get<std::int32_t>()==2 && nested_calls==1);
    assert(detached_sink.pre==0 && detached_sink.post==0 && detached_sink.errors==0);
    assert(hooked_sink.pre==2 && hooked_sink.post==2 && hooked_sink.errors==0);
    retire(hooked);retire(detached);
    std::cout << "PASS completed detachment permits bounded nested retained calls\n";
}
static void allocations_and_retirement() {
    Sink sink;
    for (int i = 0; i < 4; ++i) {
        const int before_a = allocations, before_f = frees;
        fail_allocation = i;
        auto r = RuntimeBinding::Create({}, sink); assert(!r);
        assert(r.error == "ffi_closure_alloc phase " + std::to_string(i));
        assert(allocations - before_a == i && frees - before_f == i);
    }
    fail_allocation = -1;
    auto r = RuntimeBinding::Create({}, sink); assert(r);
    assert(r.value->RemovalComplete()); // partial construction never needs provider removal
    r.value.reset(); assert(allocations == frees);
}
static void recall_suppression_nested() {
    AbiSignature s; s.parameters = {{"i32"}}; s.returns = {"i32"};
    Sink sink; int mode = 0, depth = 0; RuntimeBinding* current = nullptr;
    sink.dispatch = [&](DispatchFrame& f) {
        if (f.phase == Phase::Pre) {
            assert(!S2Hook_NoActiveDispatch());
            if (mode == 1) { f.arguments[0] = NativeValue::From<std::int32_t>(41); f.changed = true; }
            if (mode == 2) { f.action = KHook::Action::Supersede; f.result = NativeValue::From<std::int32_t>(79); }
            if (mode == 3 && !depth) {
                ++depth; auto nested = NativeValue::From<std::int32_t>(13);
                auto r = current->Call(&nested, 1); assert(r && r.value.Get<std::int32_t>() == 13); --depth;
            }
            if (mode == 4) { current->BeginRemove(); assert(!current->RemovalComplete()); }
        } else {
            assert(f.original_skipped == (mode == 2));
            if (mode == 2) assert(f.result.Get<std::int32_t>() == 79);
        }
    };
    auto b = bind(s, sink, reinterpret_cast<void*>(identity_target<std::int32_t>())); current = b.get();
    auto input = NativeValue::From<std::int32_t>(8);
    mode = 1; auto mutated = b->Call(&input, 1); assert(mutated && mutated.value.Get<std::int32_t>() == 41);
    const auto before = original_calls;
    mode = 2; auto suppressed = b->Call(&input, 1); assert(suppressed && suppressed.value.Get<std::int32_t>() == 79);
    assert(original_calls == before);
    mode = 3; auto nested = b->Call(&input, 1); assert(nested && nested.value.Get<std::int32_t>() == 8);
    assert(original_calls == before + 2);
    mode = 4; assert(b->Call(&input, 1)); assert(sink.errors == 0);
    retire(b);
    s.returns = {"void"}; Sink vs; vs.dispatch = [](DispatchFrame& f) {
        if (f.phase == Phase::Pre) f.action = KHook::Action::Supersede;
        else assert(f.original_skipped);
    };
    auto v = bind(s, vs, reinterpret_cast<void*>(fixture_targets().void_target));
    auto old = original_calls; assert(v->Call(&input, 1)); assert(original_calls == old); retire(v);
}
static void receiver_spills_novel() {
    AbiSignature s; s.receiver = "entity"; s.parameters = {{"i32"}}; s.returns = {"i32"};
    S2FnMemberFixture object{12, &original_calls}; Sink sink;
    const auto member_target = fixture_targets().member;
    sink.dispatch = [&](DispatchFrame& f) { assert(f.receiver.Get<void*>() == &object); };
    auto b = bind(s, sink, member_target);
    NativeValue args[] = {NativeValue::From(&object), NativeValue::From<std::int32_t>(5)};
    const auto member_before = original_calls;
    auto result = b->Call(args, 2);
    assert(result && result.value.Get<std::int32_t>() == 17 && original_calls == member_before + 1);
    assert(sink.pre == 1 && sink.post == 1 && sink.errors == 0);
    retire(b);
    s = {}; s.returns = {"f64"};
    std::vector<NativeValue> values;
    for (int x = 1; x <= 14; ++x) {
        s.parameters.push_back({x % 2 ? "i64" : "f64"});
        values.push_back(x % 2 ? NativeValue::From<std::int64_t>(x) : NativeValue::From<double>(x));
    }
    for (int x = 15; x <= 17; ++x) { s.parameters.push_back({"f64"}); values.push_back(NativeValue::From<double>(x)); }
    Sink mixed_sink;
    auto m = bind(s, mixed_sink, reinterpret_cast<void*>(fixture_targets().mixed));
    result = m->Call(values.data(), values.size()); assert(result && result.value.Get<double>() == 1785.0); retire(m);
    s = {}; s.returns = {"f32"}; s.parameters = {{"u32"}, {"f32"}, {"ptr"}, {"u64"}};
    Sink ns; auto n = bind(s, ns, reinterpret_cast<void*>(fixture_targets().novel));
    NativeValue nv[] = {NativeValue::From<std::uint32_t>(2), NativeValue::From<float>(1.25f), NativeValue::From(&object), NativeValue::From<std::uint64_t>(4)};
    auto nr = n->Call(nv, 4); assert(nr && nr.value.Get<float>() == 10.25f); retire(n);
}

#ifdef S2FN_NO_MAIN
#include "engine_function_bridge.h"
#include <dlfcn.h>
using BridgeCallback = int (*)(int, std::uint64_t, std::int32_t, std::int32_t*);
static BridgeCallback bridge_callback = nullptr;
static thread_local std::uint64_t bridge_owner = 0;
static Sink bridge_sink;
static std::unique_ptr<RuntimeBinding> bridge_binding;
extern "C" int s2fn_probe_create(BridgeCallback callback) {
    bridge_callback = callback;
    AbiSignature s; s.parameters = {{"i32"}}; s.returns = {"i32"};
    bridge_sink.dispatch = [](DispatchFrame& f) {
        std::int32_t output = f.result.Get<std::int32_t>();
        const auto action = bridge_callback(f.phase == Phase::Pre ? 0 : 1, bridge_owner,
            f.arguments[0].Get<std::int32_t>(), &output);
        if (f.phase == Phase::Pre && action == 2) {
            f.action = KHook::Action::Supersede; f.result = NativeValue::From(output);
        }
    };
    bridge_binding = bind(s, bridge_sink, reinterpret_cast<void*>(identity_target<std::int32_t>()));
    return bridge_binding ? 1 : 0;
}
extern "C" int s2fn_probe_call(std::uint64_t owner, std::int32_t input, std::int32_t* output) {
    const auto old = bridge_owner; bridge_owner = owner;
    auto value = NativeValue::From(input); auto result = bridge_binding->Call(&value, 1);
    bridge_owner = old;
    if (!result || bridge_sink.errors) return 0;
    *output = result.value.Get<std::int32_t>(); return 1;
}
extern "C" int s2fn_probe_remove() {
    retire(bridge_binding); KHook::Shutdown(); return allocations == frees ? 1 : 0;
}
// Production proof: no callback Globals/registration are supplied by this DSO.
// It supplies only the real Service ops and the real checked outer frame.
static std::unique_ptr<s2bridge::Service> production_service;
static std::unique_ptr<s2bridge::CoreDispatchSink> production_sink;
static void (*production_step)()=nullptr;
static bool production_requested=true;
static int production_peer_calls=0;
static KHook::Return<std::int32_t> production_peer_pre(std::int32_t);
static S2CheckedFunction<std::int32_t,std::int32_t> production_peer(production_peer_pre,nullptr);
static KHook::Return<std::int32_t> production_peer_pre(std::int32_t value){
    auto observation=production_peer.Observe();assert(observation);++production_peer_calls;
    return {KHook::Action::Ignore,value};
}
extern "C" int s2fn_production_add_peer(){
    return production_peer.Configure(checked_target(reinterpret_cast<void*>(identity_target<std::int32_t>()))).Accepted();
}
extern "C" int s2fn_production_peer_calls(){return production_peer_calls;}
static KHook::Return<void> production_frame_pre(std::int32_t);
static S2CheckedFunction<void,std::int32_t> production_frame(production_frame_pre,nullptr);
static KHook::Return<void> production_frame_pre(std::int32_t) {
    auto observation=production_frame.Observe();
    assert(s2hook_detail::g_callback_depth>0);
    struct Maintenance { ~Maintenance() { production_service->Collect(); } } maintenance;
    if (production_requested && S2Hook_EnterDispatch(observation)) production_step();
    return {KHook::Action::Ignore};
}
static long long production_prepare(const char* name,const char* target,const char* abi,const char* fingerprint,char* why,int cap) {
    auto result=production_service->Prepare(name,target,abi,fingerprint);
    if(why && cap>0) std::snprintf(why,cap,"%s",result.error.c_str());return result ? result.value : 0;
}
static int production_call(long long id,unsigned long long owner,const S2FunctionValue* args,int argc,S2FunctionValue* out,char* why,int cap) {
    auto result=production_service->Call(id,owner,args,argc,*out);
    if(why && cap>0) std::snprintf(why,cap,"%s",result.error.c_str());if(!result)return 0;*out=result.value;return 1;
}
static long long production_acquire(long long id,char* why,int cap) {
    assert(s2hook_detail::g_callback_depth>0); // actual outer or peer observation; never fabricated
    auto result=production_service->HookAcquire(id);
    if(why && cap>0) std::snprintf(why,cap,"%s",result.error.c_str());return result ? result.value : 0;
}
static int production_release(long long id){return production_service->HookRelease(id);}
static int production_target_release(long long id){return production_service->TargetRelease(id);}
static int production_status(long long id,S2FunctionHookStatus* out,char* why,int cap){auto result=production_service->HookStatus(id);if(why && cap>0)std::snprintf(why,cap,"%s",result.error.c_str());if(!result)return 0;*out=result.value;return 1;}
extern "C" int s2fn_production_create(s2bridge::CoreDispatch dispatch,void(*step)(),S2EngineOps* ops) {
    production_step=step;production_requested=true;
    Dl_info module{};auto address=checked_target(reinterpret_cast<void*>(identity_target<std::int32_t>()));
    assert(dladdr(address,&module));std::string why;
    auto image=s2original::OpenLoadedModule(module.dli_fname,why);assert(image);
    production_service=std::make_unique<s2bridge::Service>([image,address](const auto&,auto& result,auto&){
        result.image=image;result.address=reinterpret_cast<uintptr_t>(address);
        result.recipe="separated compiler-authored scalar fixture";result.validation_receipt="fixture module guard";return true;
    });
    // An uninitialized Service refuses hook registration; the actual Rust export
    // is then bound, in this same process/runtime, without another core DSO.
    production_sink=std::make_unique<s2bridge::CoreDispatchSink>(dispatch);
    assert(production_service->SetDispatchSink(production_sink.get()));
    ops->function_prepare=production_prepare;ops->function_call=production_call;
    ops->function_hook_acquire=production_acquire;ops->function_hook_release=production_release;
    ops->function_target_release=production_target_release;ops->function_hook_status=production_status;
    ops->function_frame_read=S2_FunctionFrameRead;ops->function_frame_write=S2_FunctionFrameWrite;
    ops->function_frame_commit=S2_FunctionFrameCommit;
    assert(production_frame.Configure(checked_target(reinterpret_cast<void*>(fixture_targets().void_target))).Accepted());
    return 1;
}
extern "C" int s2fn_production_frame(int requested) {
    production_requested=requested!=0;
    auto volatile target=fixture_targets().void_target;target(0);return 1;
}
extern "C" int s2fn_production_empty(){
    if(!production_service->Empty() || allocations!=frees)return 0;
    std::lock_guard<std::mutex> lock(s2hook_detail::g_retire_mu);
    return s2hook_detail::g_retire.empty();
}
extern "C" int s2fn_production_close(){
    if(!production_service->Collect())return 0;
    production_peer.BeginRemove(true);
    production_frame.BeginRemove(true);
    const auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while((!production_frame.RemovalComplete() || !production_peer.RemovalComplete()) && std::chrono::steady_clock::now()<deadline)std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(production_frame.RemovalComplete() && production_peer.RemovalComplete());assert(S2Hook_DrainRetirement());
    production_service.reset();production_sink.reset();return 1;
}

#endif
static void lazy_target_lifecycle() {
    Sink sink; AbiSignature s; s.parameters={{"i32"}}; s.returns={"i32"};
    auto made=RuntimeBinding::Create(s,sink); assert(made); auto& b=made.value;
    auto address=checked_target(reinterpret_cast<void*>(identity_target<std::int32_t>()));
    assert(b->BindTarget(address).empty());
    assert(!b->BindTarget(reinterpret_cast<void*>(fixture_targets().void_target)).empty());
    auto value=NativeValue::From<std::int32_t>(42);
    assert(b->Call(&value,1).value.Get<std::int32_t>()==42 && sink.pre==0);
    S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
    assert(!b->Configure(address).Accepted());
    assert(b->Call(&value,1).value.Get<std::int32_t>()==42);
    S2Hook_SetLifecycle(S2HookLifecycle::Running);
    assert(b->Configure(address).state==S2HookState::Pending);
    assert(b->Call(&value,1) && b->Receipt().state==S2HookState::Active);
    b->BeginRemove();
    if (!b->RemovalComplete()) { assert(!b->Call(&value,1)); assert(!b->Configure(address).Accepted()); }
    const auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while (!b->RemovalComplete() && std::chrono::steady_clock::now()<deadline)
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(b->RemovalComplete()); assert(S2Hook_DrainRetirement());
    const auto observed=sink.pre;
    assert(b->Call(&value,1).value.Get<std::int32_t>()==42 && sink.pre==observed);
    assert(b->Configure(address).state==S2HookState::Pending);
    assert(b->Call(&value,1) && sink.pre==observed+1);
    retire(b);
    std::cout << "PASS immutable target lazy call/detach/reacquire\n";
}
// Same-target admission uses the stock insertion queue while another checked
// binding owns the capsule lock. No callback depth counter is manufactured.
static void busy_capsule_runtime_insertion() {
    AbiSignature signature;signature.parameters={{"i32"}};signature.returns={"i32"};
    Sink outer_sink,queued_sink;auto address=checked_target(reinterpret_cast<void*>(identity_target<std::int32_t>()));
    auto outer=bind(signature,outer_sink,const_cast<void*>(address));
    auto made=RuntimeBinding::Create(signature,queued_sink);assert(made);auto queued=std::move(made.value);
    bool inserted=false;
    outer_sink.dispatch=[&](DispatchFrame& frame){
        if(frame.phase!=Phase::Pre || inserted)return;inserted=true;
        assert(s2hook_detail::g_callback_depth>0);
        const auto receipt=queued->Configure(address);
        assert(receipt.Accepted() && receipt.state==S2HookState::Pending);
        assert(queued_sink.pre==0); // Cannot insert while this capsule owns its lock.
    };
    auto input=NativeValue::From<std::int32_t>(4);assert(outer->Call(&input,1));
    auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while(queued_sink.pre==0 && std::chrono::steady_clock::now()<deadline){
        std::this_thread::sleep_for(std::chrono::milliseconds(1));assert(outer->Call(&input,1));
    }
    assert(queued->Receipt().state==S2HookState::Active && queued_sink.pre>0);
    // Retire another still-Pending runtime registration before the provider can
    // insert it. Its storage stays until both acknowledgements/activity drain.
    auto cancelled_result=RuntimeBinding::Create(signature,queued_sink);assert(cancelled_result);
    auto cancelled=std::move(cancelled_result.value);bool cancelled_once=false;
    outer_sink.dispatch=[&](DispatchFrame& frame){
        if(frame.phase!=Phase::Pre || cancelled_once)return;cancelled_once=true;
        assert(cancelled->Configure(address).state==S2HookState::Pending);
        cancelled->BeginRemove();
    };
    assert(outer->Call(&input,1));
    deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while(!cancelled->RemovalComplete() && std::chrono::steady_clock::now()<deadline)std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(cancelled->RemovalComplete());assert(cancelled->PruneCompletedTicket());cancelled.reset();
    retire(queued);retire(outer);
    std::cout<<"PASS real same-target busy capsule runtime Pending -> Observe and pending cancellation\n";
}
static void invocation_pairing_regression() {
    AbiSignature signature;signature.parameters={{"i32"}};signature.returns={"i32"};Sink sink;
    auto binding=bind(signature,sink,reinterpret_cast<void*>(identity_target<std::int32_t>()));
    std::vector<std::uint64_t> stack;std::vector<std::uint64_t> seen;
    bool nested=false;int mode=0;
    sink.dispatch=[&](DispatchFrame& frame){
        assert(frame.invocation_id!=0);
        if(frame.phase==Phase::Pre){
            assert(std::find(seen.begin(),seen.end(),frame.invocation_id)==seen.end());seen.push_back(frame.invocation_id);stack.push_back(frame.invocation_id);
            if(!nested && mode==1){nested=true;auto input=NativeValue::From<std::int32_t>(3);assert(binding->Call(&input,1));nested=false;}
            if(mode==2){frame.arguments[0]=NativeValue::From<std::int32_t>(8);frame.changed=true;}
            if(mode==3){frame.action=KHook::Action::Supersede;frame.result=NativeValue::From<std::int32_t>(91);}
            if(mode==4)throw std::runtime_error("proof PRE exception after pairing insertion");
        }else{
            assert(!stack.empty() && stack.back()==frame.invocation_id);stack.pop_back();
            if(mode==5)throw std::runtime_error("proof POST exception before pairing cleanup");
        }
    };
    auto value=NativeValue::From<std::int32_t>(7);
    for(mode=0;mode<=5;++mode){auto result=binding->Call(&value,1);assert(bool(result)==(mode<4));assert(stack.empty());}
    // Neutral dispatch refusal still runs the native pairing book for this row.
    const auto count=seen.size();S2Hook_SetLifecycle(S2HookLifecycle::Retiring);assert(binding->Call(&value,1));S2Hook_SetLifecycle(S2HookLifecycle::Running);assert(seen.size()==count);
    mode=0;assert(binding->Call(&value,1));assert(stack.empty());
    // Failure before id mint/Observe must leave a neutral nested row; the inner
    // POST cannot consume the still-open outer PRE state.
    bool fail_nested=true;
    sink.dispatch=[&](DispatchFrame& frame){
        if(frame.phase==Phase::Pre){stack.push_back(frame.invocation_id);if(fail_nested){fail_nested=false;s2fn::before_invocation_id=[](RuntimeBinding*){throw std::runtime_error("before id mint");};auto nested_result=binding->Call(&value,1);assert(!nested_result);s2fn::before_invocation_id={};}}
        else{assert(stack.back()==frame.invocation_id);stack.pop_back();}
    };
    assert(binding->Call(&value,1));assert(stack.empty());retire(binding);
    std::cout<<"PASS invocation ids pair across neutral, nested, recall, suppression and PRE/POST exceptions\n";
}
static void stock_tests() {
    std::cout << "phase=allocations-and-retirement\n";
    lazy_target_lifecycle();
    busy_capsule_runtime_insertion();
    invocation_pairing_regression();
    allocations_and_retirement();
    std::cout << "phase=return-phase-lifetime\n";
    return_phase_lifetime(false);
    return_phase_lifetime(true);
    completed_detachment_allows_nested_calls();
    std::cout << "phase=noncanonical-output\n";
    reject_noncanonical_output();
    std::cout << "phase=queued-remove-and-destroy-refusal\n";
    queued_remove_and_destroy_refusal();
    std::cout << "phase=32-GP-spill\n";
    spill_case<std::int64_t>("i64", std::make_index_sequence<32>{});
    std::cout << "phase=32-SSE-spill\n";
    spill_case<double>("f64", std::make_index_sequence<32>{});
    std::cout << "phase=scalar-atoms\n";
    atom<bool>("u8", false); atom<bool>("u8", true);
    atom<std::uint8_t>("u8", 0); atom<std::uint8_t>("u8", 1);
    atom<std::int32_t>("i32", -123456); atom<std::uint32_t>("u32", 0xf2345678);
    atom<std::int64_t>("i64", -0x123456781234LL); atom<std::uint64_t>("u64", 0xf123456789abcdefULL);
    atom<float>("f32", 1.25f); atom<double>("f64", -77.125); int pointer = 1; atom<void*>("ptr", &pointer);
    std::cout << "phase=recall-suppression-nested\n";
    recall_suppression_nested();
    std::cout << "phase=receiver-mixed-novel\n";
    receiver_spills_novel();
    assert(allocations == frees && S2Hook_RetirementPending() == 0);
    std::cout << "PASS stock provider atoms/member/mixed/novel/recall/suppression/reentry/removal closures=" << frees << "\n";
}
#endif
#ifndef S2FN_NO_MAIN
int main(int argc, char** argv) {
    // CI captures stdout through a pipe: retain the last completed boundary on a crash.
    std::cout << std::unitbuf;
#ifdef S2FN_VALIDATION_ONLY
    (void)argc; (void)argv;
#endif
    signatures();
#ifndef S2FN_VALIDATION_ONLY
    (void)fixture_targets();
    if (argc == 2 && std::string(argv[1]) == "--peers") {
        extern void s2fn_peer_fixtures(); s2fn_peer_fixtures();
    } else stock_tests();
    std::cout << "phase=provider-shutdown\n";
    KHook::Shutdown();
    std::cout << "phase=provider-shutdown-complete\n";
#endif
}
#endif
