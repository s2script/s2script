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
#include <dlfcn.h>
#include "engine_function_member_fixture.h"
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
// These are compiler-authored native targets, never signature-specific adapters.
// Volatile state and noinline keep an actual function entry for SafetyHook.
static volatile std::uint64_t original_calls = 0;
template<class T> __attribute__((noinline)) T identity(T value) {
    ++original_calls; return value;
}
__attribute__((noinline)) static void void_target(std::int32_t value) { original_calls += value; }
// Seven GP plus nine SSE values: the GP and SSE exhaustion points differ.
__attribute__((noinline)) static double mixed(
    std::int64_t a, double b, std::int64_t c, double d, std::int64_t e, double f,
    std::int64_t g, double h, std::int64_t i, double j, std::int64_t k, double l,
    std::int64_t m, double n, double o, double p, double q) {
    ++original_calls;
    return a + b*2 + c*3 + d*4 + e*5 + f*6 + g*7 + h*8 + i*9 + j*10 + k*11 + l*12 + m*13 + n*14 + o*15 + p*16 + q*17;
}
// Authored independently after the runtime adapter: no change to Type/Enter.
__attribute__((noinline)) static float novel(std::uint32_t a, float b, void* p, std::uint64_t c) {
    ++original_calls; return a + b + (p ? 3.0f : 0.0f) + c;
}
static std::unique_ptr<RuntimeBinding> bind(AbiSignature s, Sink& sink, const void* target) {
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
    auto b = bind(s, sink, reinterpret_cast<void*>(&identity<T>));
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

template<class T, std::size_t> struct Indexed { using type = T; };
template<class T, std::size_t... I>
__attribute__((noinline)) static T spill_target(typename Indexed<T, I>::type... args) {
    ++original_calls; return ((args * static_cast<T>(I + 1)) + ...);
}
template<class T, std::size_t... I> static void spill_case(const char* name, std::index_sequence<I...>) {
    AbiSignature s; s.returns = {name}; s.parameters.assign(sizeof...(I), {name});
    std::vector<NativeValue> values{NativeValue::From<T>(static_cast<T>(I + 1))...};
    Sink sink;
    auto b = bind(s, sink, reinterpret_cast<void*>(&spill_target<T, I...>));
    auto result = b->Call(values.data(), values.size());
    assert(result && result.value.Get<T>() == static_cast<T>(11440)); // sum of squares 1..32
    retire(b);
}
static void queued_remove_and_destroy_refusal() {
    AbiSignature s; s.parameters = {{"i32"}}; s.returns = {"i32"}; Sink sink;
    auto b = bind(s, sink, reinterpret_cast<void*>(&identity<std::int32_t>));
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
        auto receipt = queued->Configure(reinterpret_cast<void*>(&identity<std::int32_t>));
        assert(receipt.Accepted() && receipt.state == S2HookState::Pending);
        queued->BeginRemove(); queued->BeginRemove();
        assert(queued->RemovalComplete());
    };
    auto value = NativeValue::From<std::int32_t>(17); auto r = b->Call(&value, 1);
    assert(r && r.value.Get<std::int32_t>() == 17); queued.reset(); retire(b);
}
__attribute__((noinline)) static std::uint8_t noncanonical(std::uint8_t value) { ++original_calls; return value + 2; }
static void reject_noncanonical_output() {
    AbiSignature s; s.parameters = {{"u8", "bool"}}; s.returns = {"u8", "bool"}; Sink sink;
    auto b = bind(s, sink, reinterpret_cast<void*>(&noncanonical));
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
static std::function<void(RuntimeBinding*)> return_unlocked, invoke_returned;
void TestReturnUnlocked(RuntimeBinding* binding) { if (return_unlocked) return_unlocked(binding); }
void TestInvokeReturned(RuntimeBinding* binding) { if (invoke_returned) invoke_returned(binding); }
}
static void return_phase_lifetime(bool outbound) {
    AbiSignature signature; signature.parameters = {{"u8", "bool"}}; signature.returns = {"u8", "bool"};
    Sink sink;
    auto binding = bind(signature, sink, reinterpret_cast<void*>(&identity<bool>));
    sink.dispatch = [&](DispatchFrame& frame) { if (frame.phase == Phase::Pre) binding->BeginRemove(); };
    std::mutex mutex; std::condition_variable condition; bool unlocked = false, resume = false, invoke_done = false, resume_invoke = false;
    s2fn::return_unlocked = [&](RuntimeBinding* observed) {
        assert(observed == binding.get());
        std::unique_lock<std::mutex> lock(mutex); unlocked = true; condition.notify_all();
        assert(condition.wait_for(lock, std::chrono::seconds(5), [&] { return resume; }));
    };
    s2fn::invoke_returned = [&](RuntimeBinding* observed) {
        std::unique_lock<std::mutex> lock(mutex);
        if (!outbound || !resume) return; // ignore the earlier MakeOriginal ffi_call
        assert(observed == binding.get()); invoke_done = true; condition.notify_all();
        assert(condition.wait_for(lock, std::chrono::seconds(5), [&] { return resume_invoke; }));
    };
    const int before_frees = frees; bool result = false;
    // Enter from an independent native caller, not RuntimeBinding::Call: its
    // lifetime must be protected even without an outbound call lease.
    std::thread caller([&] {
        if (outbound) {
            auto input = NativeValue::From<std::uint8_t>(1);
            const auto returned = binding->Call(&input, 1); assert(returned);
            result = returned.value.Get<std::uint8_t>() == 1;
        } else { auto volatile target = &identity<bool>; result = target(true); }
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
        resume_invoke = true; lock.unlock(); condition.notify_all();
    }
    caller.join(); s2fn::return_unlocked = {}; s2fn::invoke_returned = {};
    assert(result && sink.errors == 0 && sink.pre == 1 && sink.post == 1);
    retire(binding);
    std::cout << "PASS return phase held across both provider acknowledgements; outbound=" << outbound << " caller finished before reclamation\n";
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
    auto b = bind(s, sink, reinterpret_cast<void*>(&identity<std::int32_t>)); current = b.get();
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
    auto v = bind(s, vs, reinterpret_cast<void*>(&void_target));
    auto old = original_calls; assert(v->Call(&input, 1)); assert(original_calls == old); retire(v);
}
static void receiver_spills_novel() {
    AbiSignature s; s.receiver = "entity"; s.parameters = {{"i32"}}; s.returns = {"i32"};
    S2FnMemberFixture object{12, &original_calls}; Sink sink;
    const auto member_target = s2fn_member_fixture_target();
    Dl_info target_image{}, provider_image{};
    assert(dladdr(member_target, &target_image) != 0);
    assert(dladdr(reinterpret_cast<void*>(&KHook::Shutdown), &provider_image) != 0);
    // Stock SafetyHook temporarily makes the target page RW. Keeping the target
    // in its own DSO prevents a helper needed to install it sharing that NX page.
    assert(target_image.dli_fbase != provider_image.dli_fbase);
    std::cout << "member-target-module=" << target_image.dli_fname
              << " provider-module=" << provider_image.dli_fname << "\n";
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
    auto m = bind(s, mixed_sink, reinterpret_cast<void*>(&mixed));
    result = m->Call(values.data(), values.size()); assert(result && result.value.Get<double>() == 1785.0); retire(m);
    s = {}; s.returns = {"f32"}; s.parameters = {{"u32"}, {"f32"}, {"ptr"}, {"u64"}};
    Sink ns; auto n = bind(s, ns, reinterpret_cast<void*>(&novel));
    NativeValue nv[] = {NativeValue::From<std::uint32_t>(2), NativeValue::From<float>(1.25f), NativeValue::From(&object), NativeValue::From<std::uint64_t>(4)};
    auto nr = n->Call(nv, 4); assert(nr && nr.value.Get<float>() == 10.25f); retire(n);
}

#ifdef S2FN_NO_MAIN
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
    bridge_binding = bind(s, bridge_sink, reinterpret_cast<void*>(&identity<std::int32_t>));
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
#endif
static void stock_tests() {
    std::cout << "phase=allocations-and-retirement\n";
    allocations_and_retirement();
    std::cout << "phase=return-phase-lifetime\n";
    return_phase_lifetime(false);
    return_phase_lifetime(true);
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
    if (argc == 2 && std::string(argv[1]) == "--peers") {
        extern void s2fn_peer_fixtures(); s2fn_peer_fixtures();
    } else stock_tests();
    std::cout << "phase=provider-shutdown\n";
    KHook::Shutdown();
    std::cout << "phase=provider-shutdown-complete\n";
#endif
}
#endif
