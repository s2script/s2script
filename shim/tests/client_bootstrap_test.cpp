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
    const std::array<uint16_t, 4> map_user_ids{absent, 0, 23, absent};
    std::array<int, 4> map_signon{6, 6, 6, 0};
    std::array<int, 4> map_tokens{71, 72, 73, 0};
    std::array<int, 4> map_muted{1, 1, 1, 0};
    S2ClientReconcile(4, absent,
        [&](int slot) { return map_user_ids.at(slot); },
        [&](int slot) { map_signon.at(slot) = 2; },
        [&](int slot) {
            map_signon.at(slot) = 0;
            map_tokens.at(slot) = 0;
            map_muted.at(slot) = 0;
        });
    assert(map_tokens.at(0) == 0); // removed slot must not remain a live phantom
    assert((map_signon == std::array<int, 4>{0, 2, 2, 0}));
    assert((map_tokens == std::array<int, 4>{0, 72, 73, 0})); // actual survivors keep identity
    assert((map_muted == std::array<int, 4>{0, 1, 1, 0})); // only departed policy is cleared
    std::puts("client-bootstrap: unsigned sentinel, bounded occupancy, conservative phase, survivor identity and map retirement passed");
}
