// Portable real-DSO layout regression; no provider, CIF, or detour proof.
#include "engine_function_fixture_guard.h"
#include "engine_function_member_fixture.h"
#include <cassert>
#include <cstring>
#include <iostream>
#include <utility>

__attribute__((noinline)) static std::int32_t local_target(std::int32_t value) { return value; }
template<class T, std::size_t... I> static T call_spill(S2FnSpillTarget<T> target, std::index_sequence<I...>) {
    return target(static_cast<T>(I + 1)...);
}
int main(int argc, char** argv) {
    const auto local = reinterpret_cast<const void*>(&local_target);
    const S2FnFixtureBoundary boundary{
        reinterpret_cast<const void*>(&s2fn_fixture_targets), local, local};
    assert(!boundary.Accepts(local));
    assert(!boundary.Accepts(nullptr));
    // Run this mode as an expected failure: the very same pre-Configure guard
    // must refuse a compiler-authored target accidentally put in the consumer.
    if (argc == 2 && std::strcmp(argv[1], "--reject-local") == 0) {
        boundary.Require("deliberately-local-target", local);
        std::cerr << "FAIL local target reached Configure boundary\n";
        return 1;
    }
    auto targets = s2fn_fixture_targets();
    if (argc == 2 && std::strcmp(argv[1], "--reject-local-inventory") == 0) {
        // Recreate the captured architecture defect without timing or hooks.
        targets.identity_i32 = &local_target;
        S2FnRequireFixtureInventory(targets, boundary);
        std::cerr << "FAIL local inventory reached Configure boundary\n";
        return 1;
    }
    S2FnRequireFixtureInventory(targets, boundary);
    const auto target = reinterpret_cast<const void*>(targets.identity_i32);
    assert(!(S2FnFixtureBoundary{boundary.fixture, target, local}).Accepts(target));
    assert(!(S2FnFixtureBoundary{boundary.fixture, local, target}).Accepts(target));
    assert(targets.member == s2fn_member_fixture_target());

    // Actual calls catch PLT/accessor addresses masquerading as target entries,
    // signature drift, shared peer counters, and a detached F using the wrong G.
    volatile std::uint64_t calls = 0;
    s2fn_fixture_set_original_calls(&calls);
    assert(!targets.identity_bool(false) && targets.identity_bool(true));
    assert(targets.identity_u8(1) == 1);
    assert(targets.identity_i32(-123456) == -123456);
    assert(targets.identity_u32(0xf2345678) == 0xf2345678);
    assert(targets.identity_i64(-0x123456781234LL) == -0x123456781234LL);
    assert(targets.identity_u64(0xf123456789abcdefULL) == 0xf123456789abcdefULL);
    assert(targets.identity_f32(1.25f) == 1.25f);
    assert(targets.identity_f64(-77.125) == -77.125);
    int pointer = 1; assert(targets.identity_ptr(&pointer) == &pointer);
    assert(calls == 10);
    targets.void_target(7); assert(calls == 17); // Preserve the original input-weighted void counter.
    assert(targets.mixed(1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17) == 1785.0);
    assert(targets.novel(2, 1.25f, &pointer, 4) == 10.25f);
    assert(targets.noncanonical(0) == 2);
    assert(call_spill(targets.spill_gp, std::make_index_sequence<32>{}) == 11440);
    assert(call_spill(targets.spill_sse, std::make_index_sequence<32>{}) == 11440.0);
    assert(calls == 22);
    assert(targets.detached_nested(1) == 2 && targets.detached_nested(0) == 1);
    assert(calls == 26); // Each F calls the same i32 G, so two originals per F.
    S2FnMemberFixture object{12, &calls};
    assert(reinterpret_cast<std::int32_t(*)(S2FnMemberFixture*, std::int32_t)>(targets.member)(&object, 5) == 17);
    assert(calls == 27);
    const auto peers = s2fn_fixture_peer_calls();
    assert(targets.peer_boolean != targets.identity_bool && targets.peer_mixed != targets.mixed);
    assert(!targets.peer_boolean(false) && targets.peer_boolean(true));
    assert(targets.peer_mixed(1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17) == 153.0);
    assert(s2fn_fixture_peer_calls() == peers + 3 && calls == 27);
    s2fn_fixture_set_original_calls(nullptr);
    std::cout << "PASS 19 native target layouts/signatures/counters and local-target rejection before Configure\n";
}
