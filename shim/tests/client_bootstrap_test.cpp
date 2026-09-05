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
    assert(reads == 12 && next_token == 20); // repeat/map bootstrap preserves live generations
    std::puts("client-bootstrap: unsigned sentinel, bounded occupancy, conservative phase and ensure passed");
}
