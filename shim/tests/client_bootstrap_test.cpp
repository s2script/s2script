#include "client_bootstrap.h"
#include <array>
#include <cassert>
#include <cstdint>
#include <cstdio>

int main() {
    const auto absent = static_cast<uint16_t>(-1);
    assert(absent == 65535);
    // Empty slots, a bot, a human and an unauthenticated client. SteamID/pawn are irrelevant.
    const std::array<uint16_t, 6> user_ids{absent, 0, 23, 42, absent, absent};
    std::array<int, 6> signon{0, 0, 6, 0, 0, 0};
    std::array<int, 6> tokens{0, 0, 17, 0, 0, 0};
    int next_token = 18, reads = 0;
    auto bootstrap = [&] {
        S2ClientBootstrap(static_cast<int>(user_ids.size()), absent,
            [&](int slot) { ++reads; return user_ids.at(slot); },
            [&](int slot) {
                if (signon.at(slot) < 2) signon.at(slot) = 2;
                if (!tokens.at(slot)) tokens.at(slot) = next_token++;
            });
    };
    bootstrap();
    assert((signon == std::array<int, 6>{0, 2, 6, 2, 0, 0}));
    assert((tokens == std::array<int, 6>{0, 18, 17, 19, 0, 0}));
    bootstrap();
    assert(reads == 12 && next_token == 20); // repeated late-load bootstrap preserves live generations
    // A map can remove an active client without a disconnect hook. The engine reports its
    // unsigned absent sentinel while our previous-map phase/token still say active.
    const std::array<uint16_t, 5> map_user_ids{absent, 0, 23, 42, absent};
    std::array<int, 5> map_signon{6, 6, 5, 0, 0};
    std::array<int, 5> map_tokens{71, 72, 73, 0, 0};
    std::array<int, 5> map_muted{1, 1, 0, 0, 0};
    std::array<uint64_t, 5> map_audible{0xff, 0xf0, 0x0f, 0, 0};
    uint64_t map_has_rule = 0b00111;
    int map_reads = 0, map_next_token = 74;
    auto reconcile = [&] {
        S2ClientReconcile(5, absent,
            [&](int slot) { ++map_reads; return map_user_ids.at(slot); },
            [&](int slot) {
                if (map_signon.at(slot) < 2) map_signon.at(slot) = 2;
                if (!map_tokens.at(slot)) map_tokens.at(slot) = map_next_token++;
            },
            [&](int slot) {
                map_signon.at(slot) = 0;
                map_tokens.at(slot) = 0;
                map_muted.at(slot) = 0;
                map_audible.at(slot) = 0;
                map_has_rule &= ~(1ull << slot);
            });
    };
    reconcile();
    // StartupServer can repeat in the same session without any intervening active callback.
    reconcile();
    assert(map_tokens.at(0) == 0); // removed slot must not remain a live phantom
    assert((map_signon == std::array<int, 5>{0, 6, 5, 2, 0})); // preserve observed full/spawn phases
    assert((map_tokens == std::array<int, 5>{0, 72, 73, 74, 0})); // ensure once, never replace survivors
    assert((map_muted == std::array<int, 5>{0, 1, 0, 0, 0}));
    assert((map_audible == std::array<uint64_t, 5>{0, 0xf0, 0x0f, 0, 0}));
    assert(map_has_rule == 0b00110); // only departed policy is cleared
    assert(map_reads == 10 && map_next_token == 75);
    std::puts("client-bootstrap: unsigned sentinel, repeated reconciliation, survivor phase/identity/policy and absent-slot retirement passed");
}
