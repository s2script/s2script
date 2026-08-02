// The DEFERRED-DISPATCH QUEUE (deferred-dispatch-queue slice) — the engine-free half.
//
// Core holds the V8 isolate borrow across ALL JS, so a dispatch that re-enters core from a JS
// handler used to be SILENTLY DROPPED. Core now reports S2_DISPATCH_DEFERRED instead, and the shim
// — which still has the arguments on its own stack — queues a replay for the next GameFrame.
//
// This TU owns the QUEUE POLICY and nothing else: FIFO order, the double buffer, the bound, the
// exactly-once free of a queued duplicate, and the drain's re-entrancy discipline. Every engine and
// core touchpoint is INJECTED through S2DeferOps, so the policy compiles and runs with no SDK, no
// engine and no isolate — which is what makes shim/tests/defer_queue_test.cpp able to drive the
// REAL drain (the defect this extraction exists to make testable was a drain-vs-flush interaction
// that a mock drain over a local vector is structurally incapable of reproducing).
//
// NO RAW POINTER IS EVER QUEUED beyond the one the shim itself minted: every string is an owned
// copy, and `dup` is a shim-owned DuplicateEvent copy the engine no longer references.
#ifndef S2SCRIPT_DEFER_QUEUE_H
#define S2SCRIPT_DEFER_QUEUE_H

#include <cstddef>
#include <string>

// One variant per deferrable core entry. Adding a deferrable dispatch means adding a variant —
// the same shape as adding a fan_out channel, deliberately explicit rather than a void* blob.
enum S2DeferredKind {
    S2_DEFERRED_GAME_EVENT = 0,
    S2_DEFERRED_CLIENT_EVENT,
    S2_DEFERRED_MAP_START,
    S2_DEFERRED_ENTITY_EVENT,
    S2_DEFERRED_CVAR_CHANGE,
};

const char* S2DeferredKindName(S2DeferredKind k);

struct S2Deferred {
    S2DeferredKind kind = S2_DEFERRED_GAME_EVENT;
    std::string    a;   // game_event: name | client_event: name | map_start: map | entity_event: kind | cvar_change: name
    std::string    b;   // entity_event: className | cvar_change: newValue                             (else empty)
    std::string    c;   // cvar_change: oldValue                                                       (else empty)
    int            i = 0;          // client_event: slot | entity_event: packed CEntityHandle          (else 0)
    void*          dup = nullptr;  // game_event ONLY: the shim-minted IGameEvent duplicate, opaque here.
                                   // Owned by whoever holds the entry — the queue, or the drain once it
                                   // has moved the entry onto its own stack. Freed exactly once, via
                                   // S2DeferOps::free_event.
};

// The engine + core calls the queue needs, injected. `s2script_mm.cpp` wires them to
// IGameEventManager2 / s2script_core_replay_*; the selftest wires them to fakes.
struct S2DeferOps {
    // Mint a shim-owned copy of a live IGameEvent (opaque `void*` here). Returns null when
    // game-event deferral is degraded — the caller then drops the dispatch BY NAME.
    void* (*duplicate_event)(void* ev) = nullptr;
    // Release a copy minted by duplicate_event. Called exactly once per queued duplicate.
    void  (*free_event)(void* dup) = nullptr;
    // Replay ONE entry through core. The shim's implementation is responsible for the game-event
    // case's s_currentEvent save/restore; the duplicate's LIFETIME is not its business.
    // Returns the C-ABI dispatch result (S2_DISPATCH_DEFERRED == core re-deferred it).
    int   (*replay)(const S2Deferred& e) = nullptr;
    // One preformatted line (already newline-terminated), so this TU needs no META_CONPRINTF.
    void  (*log)(const char* line) = nullptr;
};

// Install the ops. Call once, before any push — `s2script_mm.cpp` does it at the top of Load().
void S2Defer_SetOps(const S2DeferOps& ops);

// The queue bound. An unbounded queue turns a plugin bug (a handler that re-fires its own event
// forever) into an OOM. Overflow drops the NEWEST and names it.
int S2Defer_Max();

// Push the scalar variants (every string is COPIED — the engine buffers they come from are
// call-scoped) and the one payload-carrying variant (capacity is checked BEFORE the duplication,
// so an overflow can never leak an event).
void S2Defer_PushScalar(S2DeferredKind kind, const char* a, const char* b, const char* c, int i);
void S2Defer_PushGameEvent(const char* name, void* ev);

// Replay everything queued since the last drain. Called at the TOP of Hook_GameFramePre, where the
// isolate is provably free.
void S2Defer_Drain();

// Drop everything queued (and abandon an in-flight drain), freeing every duplicate. Called at map
// start — queued entries name a world that no longer exists — and at Unload.
void S2Defer_Flush(const char* why);

// --- observation points (the selftest's assertions; cheap enough to ship) ---------------------
std::size_t S2Defer_QueuedCount();   // entries waiting for the NEXT drain
std::size_t S2Defer_BatchCount();    // entries in the batch being (or last) drained
bool        S2Defer_Draining();      // true while S2Defer_Drain is walking its batch
// Test-harness reset: drop both buffers (freeing duplicates) and clear the drain flags.
void S2Defer_ResetForTest();

#endif  // S2SCRIPT_DEFER_QUEUE_H
