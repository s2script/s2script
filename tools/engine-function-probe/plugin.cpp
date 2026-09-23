// Test-only independent typed KHook peer. This fixture is linked into the stock
// provider executable; it is not a second production provider or an ABI thunk.
#include "engine_function_abi.h"
#include <cassert>
#include <chrono>
#include <thread>
#include <iostream>
using namespace s2fn;
namespace {
template<class R, class... Args> struct Peer {
    KHook::HookID_t id = KHook::INVALID_HOOK;
    R override_value{}, observed{};
    bool skipped = false;
    int observations = 0;
    static void Removed(KHook::HookID_t) { KHook::GetContext<Peer>()->id = KHook::INVALID_HOOK; }
    static R Pre(Args...) {
        auto self = KHook::GetContext<Peer>();
        KHook::__internal__savereturnvalue(KHook::Return<R>{KHook::Action::Override, self->override_value}, false);
        return R{};
    }
    static R Post(Args...) {
        auto self = KHook::GetContext<Peer>();
        self->observed = KHook::GetCurrentReturn<R>();
        self->skipped = KHook::WasOriginalFunctionSkipped(); ++self->observations;
        KHook::__internal__savereturnvalue(KHook::Return<R>{KHook::Action::Ignore, R{}}, false);
        return R{};
    }
    static R MakeReturn(Args...) {
        R value = KHook::GetCurrentReturn<R>(true); KHook::DestroyReturnValue(); return value;
    }
    static R Original(Args... args) {
        auto fn = reinterpret_cast<R(*)(Args...)>(KHook::GetOriginalFunction());
        R value = fn(args...);
        KHook::__internal__savereturnvalue(KHook::Return<R>{KHook::Action::Ignore, value}, true); return value;
    }
    void Install(void* target, unsigned stack) {
        id = KHook::SetupHook(target, this, reinterpret_cast<void*>(&Removed), reinterpret_cast<void*>(&Pre),
            reinterpret_cast<void*>(&Post), reinterpret_cast<void*>(&MakeReturn), reinterpret_cast<void*>(&Original), stack, false);
        assert(id != KHook::INVALID_HOOK);
    }
    ~Peer() { if (id != KHook::INVALID_HOOK) KHook::RemoveHook(id, false); }
};
static volatile unsigned calls = 0;
__attribute__((noinline)) bool boolean(bool value) { ++calls; return value; }
__attribute__((noinline)) double mixed(std::int64_t a, double b, std::int64_t c, double d,
    std::int64_t e, double f, std::int64_t g, double h, std::int64_t i, double j,
    std::int64_t k, double l, std::int64_t m, double n, double o, double p, double q) {
    ++calls; return a+b+c+d+e+f+g+h+i+j+k+l+m+n+o+p+q;
}
struct Observer : DispatchSink {
    bool suppress = false;
    NativeValue decision, expected;
    std::size_t width = 0;
    unsigned seen = 0;
    void Dispatch(DispatchFrame& f) override {
        if (f.phase == Phase::Pre && suppress) { f.action = KHook::Action::Supersede; f.result = decision; }
        if (f.phase == Phase::Post) {
            assert(std::memcmp(f.result.bytes.data(), expected.bytes.data(), width) == 0);
            assert(f.original_skipped == suppress); ++seen;
        }
    }
    void Error(const char* error) noexcept override { std::cerr << error << '\n'; std::abort(); }
};
template<class R, class... Args> void order(bool peer_first, AbiSignature signature, R(*target)(Args...),
    std::vector<NativeValue> values, R override_value, R suppressed_value) {
    Peer<R, Args...> peer; peer.override_value = override_value;
    Observer sink; sink.width = sizeof(R); sink.expected = NativeValue::From(override_value);
    sink.decision = NativeValue::From(suppressed_value);
    auto made = RuntimeBinding::Create(signature, sink); assert(made); auto b = std::move(made.value);
    if (peer_first) peer.Install(reinterpret_cast<void*>(target), b->Info().stack_bytes);
    assert(b->Configure(reinterpret_cast<void*>(target)).Accepted());
    if (!peer_first) peer.Install(reinterpret_cast<void*>(target), b->Info().stack_bytes);
    auto r = b->Call(values.data(), values.size()); assert(r && r.value.Get<R>() == override_value);
    assert(peer.observed == override_value && !peer.skipped);
    if (signature.returns.native == "u8") for (std::size_t i = 1; i < r.value.bytes.size(); ++i) assert(r.value.bytes[i] == 0);
    sink.suppress = true; sink.expected = sink.decision; const auto originals = calls;
    r = b->Call(values.data(), values.size()); assert(r && r.value.Get<R>() == suppressed_value);
    assert(calls == originals && peer.observed == suppressed_value && peer.skipped && sink.seen == 2);
    b->BeginRemove(); b->BeginRemove();
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
    while (!b->RemovalComplete() && std::chrono::steady_clock::now() < deadline) std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(b->RemovalComplete()); S2Hook_DrainRetirement();
    const auto fingerprint = b->Info().fingerprint; b.reset();
    assert(peer.id != KHook::INVALID_HOOK); // removing s2script leaves peer registered
    // Call through libffi on the still-live target using a new runtime binding;
    // peer observations must advance independently of the retired subscription.
    Observer second; second.width = sizeof(R); second.expected = NativeValue::From(override_value);
    auto next = RuntimeBinding::Create(signature, second); assert(next);
    assert(next.value->Configure(reinterpret_cast<void*>(target)).Accepted());
    r = next.value->Call(values.data(), values.size()); assert(r && r.value.Get<R>() == override_value);
    assert(peer.observations == 3 && sink.seen == 2);
    next.value->BeginRemove();
    while (!next.value->RemovalComplete() && std::chrono::steady_clock::now() < deadline) std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(next.value->RemovalComplete()); S2Hook_DrainRetirement(); next.value.reset();
    std::cout << "PASS peer-order=" << (peer_first ? "peer-first" : "s2-first") << " vector=" << fingerprint
              << " effective-result=observed suppression=observed removal=independent\n";
}
}
void s2fn_peer_fixtures() {
    for (bool first : {true, false}) {
        AbiSignature s; s.parameters = {{"u8", "bool"}}; s.returns = {"u8", "bool"};
        order(first, s, &boolean, {NativeValue::From<bool>(false)}, true, false);
        s = {}; s.returns = {"f64"}; std::vector<NativeValue> v;
        for (int i = 1; i <= 14; ++i) {
            s.parameters.push_back({i % 2 ? "i64" : "f64"});
            v.push_back(i % 2 ? NativeValue::From<std::int64_t>(i) : NativeValue::From<double>(i));
        }
        for (int i = 15; i <= 17; ++i) { s.parameters.push_back({"f64"}); v.push_back(NativeValue::From<double>(i)); }
        order(first, s, &mixed, v, 998.5, -13.25);
    }
}
