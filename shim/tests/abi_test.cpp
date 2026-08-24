#include "../src/call_abi.h"
#include "../src/hook_abi.h"
#include "../src/hook_dispatch.h"

#include <cmath>
#include <cstdint>
#include <cstring>
#include <iostream>

#if defined(S2_ABI_PROBE_MICROSOFT) && !defined(_WIN32)
#define S2_TEST_ABI __attribute__((ms_abi))
#else
#define S2_TEST_ABI
#endif

static int g_fail = 0;
#define CHECK(cond, msg)                                                        \
    do {                                                                        \
        if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; g_fail++; }      \
        else         { std::cout << "ok:   " << (msg) << "\n"; }                \
    } while (0)

namespace {

uint64_t S2_TEST_ABI MixedReceiver(uint64_t self, float a, uint64_t b, float c) {
    return self + static_cast<uint64_t>(a * 10.0f) + b * 100 + static_cast<uint64_t>(c * 1000.0f);
}

float S2_TEST_ABI MixedReceiverless(float a, uint64_t b, float c) {
    return a + static_cast<float>(b) + c;
}

uint64_t S2_TEST_ABI StackSpill(uint64_t a0, float f0, uint64_t a1, uint64_t a2,
                                uint64_t a3, uint64_t a4, uint64_t a5, uint64_t a6,
                                float f1) {
    return a0 + a1 * 10 + a2 * 100 + a3 * 1000 + a4 * 10000 + a5 * 100000 +
           a6 * 1000000 + static_cast<uint64_t>(f0 * 10.0f) +
           static_cast<uint64_t>(f1 * 100.0f);
}

bool Invoke(void* fn, bool receiver, uint64_t self, const uint8_t* classes, int classCount,
            const uint64_t* gp, int gpCount, const float* fp, int fpCount,
            int retKind, uint64_t* out) {
    s2callabi::InvokeRequest req{};
    req.fn = fn;
    req.hasReceiver = receiver;
    req.receiver = self;
    req.argClasses = classes;
    req.argCount = classCount;
    req.gp = gp;
    req.gpCount = gpCount;
    req.fp = fp;
    req.fpCount = fpCount;
    req.retKind = static_cast<s2callabi::ReturnClass>(retKind);
    return s2callabi::Invoke(req, out);
}

void test_outbound_mixed_receiver_and_returns() {
    const uint8_t classes[] = { s2callabi::kArgF32, s2callabi::kArgGp, s2callabi::kArgF32 };
    const uint64_t gp[] = { 7 };
    const float fp[] = { 1.5f, 2.0f };
    uint64_t out = 0;
    CHECK(Invoke(reinterpret_cast<void*>(&MixedReceiver), true, 3, classes, 3,
                 gp, 1, fp, 2, s2callabi::kRetU64, &out),
          "outbound: mixed GP/float receiver call is invoked");
    CHECK(out == 2718, "outbound: mixed positional values reach the receiver call");

    out = 0;
    CHECK(Invoke(reinterpret_cast<void*>(&MixedReceiverless), false, 0, classes, 3,
                 gp, 1, fp, 2, s2callabi::kRetF32, &out),
          "outbound: mixed receiverless call is invoked");
    float result = 0.0f;
    const uint32_t bits = static_cast<uint32_t>(out);
    std::memcpy(&result, &bits, sizeof result);
    CHECK(std::fabs(result - 10.5f) < 0.001f,
          "outbound: float return and receiverless positional assignment survive");
}

void test_outbound_stack_spill() {
    const uint8_t classes[] = {
        s2callabi::kArgGp, s2callabi::kArgF32, s2callabi::kArgGp,
        s2callabi::kArgGp, s2callabi::kArgGp, s2callabi::kArgGp,
        s2callabi::kArgGp, s2callabi::kArgGp, s2callabi::kArgF32,
    };
    const uint64_t gp[] = { 1, 2, 3, 4, 5, 6, 7 };
    const float fp[] = { 1.25f, 2.5f };
    uint64_t out = 0;
    CHECK(Invoke(reinterpret_cast<void*>(&StackSpill), false, 0, classes, 9,
                 gp, 7, fp, 2, s2callabi::kRetU64, &out),
          "outbound: arguments beyond the register budget are invoked");
    CHECK(out == 7'654'583, "outbound: stack-spilled GP/float positions retain their values");
}

struct HookCapture {
    int dispatches = 0;
    int posts = 0;
    int skipped = -1;
    int mode = 0;
    void* self = nullptr;
    float f = 0;
    int32_t i[3] = {};
    int64_t q[2] = {};
} g_hook;

int HookDispatch(int, void* raw) {
    auto* v = static_cast<s2hookabi::ArgView*>(raw);
    g_hook.dispatches++;
    if (g_hook.mode == 1) {
        v->f[0] += 1.0f;
        v->i[0] += 10;
    } else if (g_hook.mode == 2) {
        v->i[1] = 3;
        v->i[2] = 1;
        return 2;
    }
    return 0;
}

int HookPost(int, void* raw, int skipped) {
    auto* v = static_cast<s2hookabi::ArgView*>(raw);
    g_hook.posts++;
    g_hook.skipped = skipped;
    g_hook.i[1] = v->i[1];
    return 0;
}

void S2_TEST_ABI OrigThisVoid(void* self) {
    g_hook.self = self;
}

void S2_TEST_ABI OrigMixed(void* self, float f, int32_t i0, int32_t i1, int32_t i2) {
    g_hook.self = self; g_hook.f = f;
    g_hook.i[0] = i0; g_hook.i[1] = i1; g_hook.i[2] = i2;
}

void S2_TEST_ABI OrigWide(void* self, float f, int32_t i, int64_t q0, int64_t q1) {
    g_hook.self = self; g_hook.f = f; g_hook.i[0] = i; g_hook.q[0] = q0; g_hook.q[1] = q1;
}

int32_t S2_TEST_ABI OrigAcquire(void* self, int64_t q0, int32_t i, int64_t q1) {
    g_hook.self = self; g_hook.i[0] = i; g_hook.q[0] = q0; g_hook.q[1] = q1;
    return 2;
}

void ResetHook(int mode) {
    g_hook = HookCapture{};
    g_hook.mode = mode;
    s2hookabi::Reset();
    S2HookOps ops{};
    ops.dispatch = &HookDispatch;
    ops.dispatch_post = &HookPost;
    S2Hook_SetOps(ops);
}

template <typename Fn>
Fn HookFn(int shape, int id = 0) {
    return reinterpret_cast<Fn>(s2hookabi::ThunkFor(shape, id));
}

void test_all_four_hook_shapes() {
    using ThisVoid = void (S2_TEST_ABI *)(void*);
    using Mixed = void (S2_TEST_ABI *)(void*, float, int32_t, int32_t, int32_t);
    using Wide = void (S2_TEST_ABI *)(void*, float, int32_t, int64_t, int64_t);
    using Acquire = int32_t (S2_TEST_ABI *)(void*, int64_t, int32_t, int64_t);

    ResetHook(0);
    s2hookabi::SetOriginal(0, reinterpret_cast<void*>(&OrigThisVoid));
    HookFn<ThisVoid>(S2_HOOK_SHAPE_THIS_VOID)(reinterpret_cast<void*>(0x1234));
    CHECK(g_hook.dispatches == 1 && g_hook.self == reinterpret_cast<void*>(0x1234),
          "hooks: this_void dispatches and forwards its receiver");

    ResetHook(1);
    s2hookabi::SetOriginal(0, reinterpret_cast<void*>(&OrigMixed));
    HookFn<Mixed>(S2_HOOK_SHAPE_THIS_F32_I32_I32_I32)(
        reinterpret_cast<void*>(0x2345), 1.5f, 2, 3, 4);
    CHECK(g_hook.f == 2.5f && g_hook.i[0] == 12 && g_hook.i[1] == 3 && g_hook.i[2] == 4,
          "hooks: mixed shape writes are relayed through the typed original call");

    ResetHook(0);
    s2hookabi::SetOriginal(0, reinterpret_cast<void*>(&OrigWide));
    HookFn<Wide>(S2_HOOK_SHAPE_THIS_F32_I32_I64_I64)(
        reinterpret_cast<void*>(0x3456), 3.5f, 9, INT64_C(0x1234567887654321),
        INT64_C(0x2345678998765432));
    CHECK(g_hook.q[0] == INT64_C(0x1234567887654321) &&
              g_hook.q[1] == INT64_C(0x2345678998765432),
          "hooks: opaque i64 arguments survive without narrowing");

    ResetHook(2);
    s2hookabi::SetOriginal(0, reinterpret_cast<void*>(&OrigAcquire));
    const int32_t acquired = HookFn<Acquire>(S2_HOOK_SHAPE_THIS_I64_I32_I64)(
        reinterpret_cast<void*>(0x4567), 99, 7, 101);
    CHECK(acquired == 3 && g_hook.posts == 1 && g_hook.skipped == 1,
          "hooks: i32-return shape suppresses and runs post with the plugin result");
}

void test_validator_register_maps() {
    const s2hookabi::ValidatorSpec mixed =
        s2hookabi::ValidatorFor(S2_HOOK_SHAPE_THIS_F32_I32_I32_I32);
    CHECK(mixed.wide != nullptr && mixed.slots == 4,
          "validator: mixed shape exposes all integer widths");
#if defined(_WIN32) || defined(S2_ABI_PROBE_MICROSOFT)
    CHECK(mixed.registerMap.slotOfGpr[1] == 0, "validator/MS: rcx maps to receiver slot 0");
    CHECK(mixed.registerMap.slotOfGpr[2] == -1, "validator/MS: rdx is skipped by positional float");
    CHECK(mixed.registerMap.slotOfGpr[8] == 1, "validator/MS: r8 maps to first integer param");
    CHECK(mixed.registerMap.slotOfGpr[9] == 2, "validator/MS: r9 maps to second integer param");
#else
    CHECK(mixed.registerMap.slotOfGpr[7] == 0, "validator/SysV: rdi maps to receiver slot 0");
    CHECK(mixed.registerMap.slotOfGpr[6] == 1, "validator/SysV: rsi maps to first integer param");
    CHECK(mixed.registerMap.slotOfGpr[2] == 2, "validator/SysV: rdx maps to second integer param");
    CHECK(mixed.registerMap.slotOfGpr[1] == 3, "validator/SysV: rcx maps to third integer param");
#endif
}

}  // namespace

int main() {
    std::cout << std::unitbuf;
    test_outbound_mixed_receiver_and_returns();
    test_outbound_stack_spill();
    test_all_four_hook_shapes();
    test_validator_register_maps();
    if (g_fail) { std::cerr << "abi_test: " << g_fail << " FAILURE(S)\n"; return 1; }
    std::cout << "abi_test: all checks passed\n";
    return 0;
}
