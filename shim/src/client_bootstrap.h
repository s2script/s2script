#ifndef S2SCRIPT_CLIENT_BOOTSTRAP_H
#define S2SCRIPT_CLIENT_BOOTSTRAP_H

// Engine-free scan policy. The caller supplies the engine's typed absent sentinel
// (CPlayerUserId(-1).Get(), an unsigned short), never an assumed signed -1.
// User-id establishes occupancy only; the caller retains authority over signon phase.
template <typename UserId, typename ReadUserId, typename EnsureConnected>
void S2ClientBootstrap(int max_slots, UserId absent, ReadUserId read_user_id, EnsureConnected ensure_connected) {
    for (int slot = 0; slot < max_slots; ++slot) {
        if (read_user_id(slot) != absent) ensure_connected(slot);
    }
}

#endif
