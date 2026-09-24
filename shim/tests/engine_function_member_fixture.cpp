#include "engine_function_member_fixture.h"
#include <khook.hpp>
#include <utility>
#include <cstring>

namespace {
// No adapter, callbacks, provider workers, or shared std helper bodies live in
// this image. Every native target has hidden linkage and an actual noinline body.
volatile std::uint64_t default_original_calls = 0;
volatile std::uint64_t* original_calls = &default_original_calls;
volatile unsigned peer_calls = 0;
template<class T> __attribute__((noinline)) T identity(T value) {
    ++*original_calls; return value;
}
__attribute__((noinline)) void void_target(std::int32_t value) { *original_calls += value; }
// Seven GP plus nine SSE values: the GP and SSE exhaustion points differ.
__attribute__((noinline)) double mixed(
    std::int64_t a, double b, std::int64_t c, double d, std::int64_t e, double f,
    std::int64_t g, double h, std::int64_t i, double j, std::int64_t k, double l,
    std::int64_t m, double n, double o, double p, double q) {
    ++*original_calls;
    return a + b*2 + c*3 + d*4 + e*5 + f*6 + g*7 + h*8 + i*9 + j*10 + k*11 + l*12 + m*13 + n*14 + o*15 + p*16 + q*17;
}
__attribute__((noinline)) float novel(std::uint32_t a, float b, void* p, std::uint64_t c) {
    ++*original_calls; return a + b + (p ? 3.0f : 0.0f) + c;
}
__attribute__((noinline)) std::uint8_t noncanonical(std::uint8_t value) { ++*original_calls; return value + 2; }
template<class T, std::size_t> struct Indexed { using type = T; };
template<class T, std::size_t... I>
__attribute__((noinline)) T spill_target(typename Indexed<T, I>::type... args) {
    ++*original_calls; return ((args * static_cast<T>(I + 1)) + ...);
}
template<class T, std::size_t... I> constexpr S2FnSpillTarget<T> spill_address(std::index_sequence<I...>) {
    return &spill_target<T, I...>;
}
__attribute__((noinline)) std::int32_t detached_nested_target(std::int32_t value) {
    ++*original_calls;
    // This must call the same hookable G entry exported as identity_i32, even
    // after F's detour is removed. Volatile preserves the real indirect call.
    auto volatile next = &identity<std::int32_t>;
    return next(value) + 1;
}
__attribute__((noinline)) std::int32_t copied_length(const char* value) { ++*original_calls; return static_cast<std::int32_t>(std::strlen(value)); }
__attribute__((noinline)) bool peer_boolean(bool value) { ++peer_calls; return value; }
__attribute__((noinline)) double peer_mixed(std::int64_t a, double b, std::int64_t c, double d,
    std::int64_t e, double f, std::int64_t g, double h, std::int64_t i, double j,
    std::int64_t k, double l, std::int64_t m, double n, double o, double p, double q) {
    ++peer_calls; return a+b+c+d+e+f+g+h+i+j+k+l+m+n+o+p+q;
}
}

std::int32_t S2FnMemberFixture::Call(std::int32_t value) {
    ++*original_calls;
    return base + value;
}
extern "C" void* s2fn_member_fixture_target() {
    return KHook::ExtractMFP(&S2FnMemberFixture::Call);
}
extern "C" S2FnFixtureTargets s2fn_fixture_targets() {
    return {&identity<bool>, &identity<std::uint8_t>, &identity<std::int32_t>,
        &identity<std::uint32_t>, &identity<std::int64_t>, &identity<std::uint64_t>,
        &identity<float>, &identity<double>, &identity<void*>, &void_target, &mixed,
        &novel, &noncanonical, spill_address<std::int64_t>(std::make_index_sequence<32>{}),
        spill_address<double>(std::make_index_sequence<32>{}), &detached_nested_target,
        s2fn_member_fixture_target(), &peer_boolean, &peer_mixed};
}
extern "C" void s2fn_fixture_set_original_calls(volatile std::uint64_t* calls) {
    original_calls = calls ? calls : &default_original_calls;
}
extern "C" unsigned s2fn_fixture_peer_calls() { return peer_calls; }

extern "C" void* s2fn_fixture_copy_length_target() { return reinterpret_cast<void*>(&copied_length); }
