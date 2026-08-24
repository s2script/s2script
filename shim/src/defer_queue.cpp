// The deferred-dispatch queue's policy half. See defer_queue.h for the shape and why it is its own
// TU. Engine-free by construction: the only outside contact is S2DeferOps.
#include "defer_queue.h"
#include "s2script_core.h"   // S2_DISPATCH_DEFERRED (the one cross-language constant this reads)

#include <cstdarg>
#include <cstdio>
#include <utility>
#include <vector>

// A fixed cap, like the hand-rolled per-op pending sets this queue replaced.
static const int kDeferredQueueMax = 256;

// DOUBLE BUFFERED. The drain swaps buffers BEFORE replaying, so a dispatch deferred *by* a deferred
// handler lands in the NEXT drain instead of extending the batch being walked. Without the swap, a
// handler that re-fires its own event spins the frame forever. (Same discipline as the engine's own
// RunFrameHooks frame_queue/frame_actions swap.)
static std::vector<S2Deferred> s_queue;   // push target
static std::vector<S2Deferred> s_batch;   // the batch being replayed this frame

// The drain's re-entrancy state. A replay runs arbitrary JS, and JS can reach back into the shim —
// the map-start flush exists precisely because "a plugin-declared engine call that reaches a
// changelevel from JS" happens. So both of the queue's other entry points can fire from INSIDE the
// drain loop, and neither may disturb the batch that loop is walking.
static bool s_draining   = false;   // true between the swap and the trailing clear
static bool s_abandoned  = false;   // a flush landed mid-drain: stop replaying, free the tail

static S2DeferOps s_ops;

const char* S2DeferredKindName(S2DeferredKind k) {
    switch (k) {
        case S2_DEFERRED_GAME_EVENT:   return "game_event";
        case S2_DEFERRED_CLIENT_EVENT: return "client_event";
        case S2_DEFERRED_MAP_START:    return "map_start";
        case S2_DEFERRED_ENTITY_EVENT: return "entity_event";
        case S2_DEFERRED_CVAR_CHANGE:  return "cvar_change";
    }
    return "?";
}

void S2Defer_SetOps(const S2DeferOps& ops) { s_ops = ops; }
int  S2Defer_Max() { return kDeferredQueueMax; }

std::size_t S2Defer_QueuedCount() { return s_queue.size(); }
std::size_t S2Defer_BatchCount()  { return s_batch.size(); }
bool        S2Defer_Draining()    { return s_draining; }

#if defined(__GNUC__) || defined(__clang__)
__attribute__((format(printf, 1, 2)))
#endif
static void Logf(const char* fmt, ...) {
    if (!s_ops.log) return;
    char line[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(line, sizeof(line), fmt, ap);
    va_end(ap);
    s_ops.log(line);
}

static void FreeDup(void* dup) {
    if (dup && s_ops.free_event) s_ops.free_event(dup);
}

// Empty a buffer, freeing any duplicate an entry still owns. Used EVERY time a buffer is emptied,
// so a batch that was only partly drained can never strand a duplicate when the buffer is reused.
static void ClearBuffer(std::vector<S2Deferred>& v) {
    for (S2Deferred& e : v) {
        FreeDup(e.dup);
        e.dup = nullptr;
    }
    v.clear();
}

// Frees the ONE duplicate an in-flight replay owns, on every exit path including a C++ throw out of
// a handler. The drain moves each entry onto its own stack before replaying it, so this guard — not
// the batch — is that duplicate's sole owner for the duration of the call.
namespace {
struct DupGuard {
    void* dup;
    explicit DupGuard(void* d) : dup(d) {}
    ~DupGuard() { FreeDup(dup); }
    DupGuard(const DupGuard&) = delete;
    DupGuard& operator=(const DupGuard&) = delete;
};
}  // namespace

void S2Defer_PushScalar(S2DeferredKind kind, const char* a, const char* b, const char* c, int i) {
    if (static_cast<int>(s_queue.size()) >= kDeferredQueueMax) {
        Logf("[s2script] deferred-dispatch: queue full (%d) — dropped %s '%s' (newest)\n",
             kDeferredQueueMax, S2DeferredKindName(kind), a ? a : "");
        return;
    }
    S2Deferred e;
    e.kind = kind;
    if (a) e.a = a;
    if (b) e.b = b;
    if (c) e.c = c;
    e.i = i;
    s_queue.push_back(std::move(e));
}

// game_event is the ONE variant with a payload. Its handlers must read the REAL field values, and
// the engine's IGameEvent is valid only across the original call — so the queue takes its own copy.
// Capacity is checked BEFORE the duplication, so an overflow can never leak an event. Both failure
// paths (queue full, duplication unavailable) name what was dropped.
void S2Defer_PushGameEvent(const char* name, void* ev) {
    if (!name) return;
    if (static_cast<int>(s_queue.size()) >= kDeferredQueueMax) {
        Logf("[s2script] deferred-dispatch: queue full (%d) — dropped game_event '%s' (newest)\n",
             kDeferredQueueMax, name);
        return;
    }
    S2Deferred e;
    e.kind = S2_DEFERRED_GAME_EVENT;
    e.a = name;                      // copy the name BEFORE minting the duplicate we would have to free
    e.dup = (s_ops.duplicate_event && ev) ? s_ops.duplicate_event(ev) : nullptr;
    if (!e.dup) {
        Logf("[s2script] deferred-dispatch: DuplicateEvent unavailable — dropped game_event '%s'\n", name);
        return;
    }
    s_queue.push_back(std::move(e));
}

void S2Defer_Drain() {
    // A drain re-entered from inside a replay would swap the batch out from under the outer loop.
    // Never nest: the outer drain is already walking this batch, and anything a replay pushes is
    // the NEXT drain's work by design.
    if (s_draining) return;

    // Empty the previous batch FIRST — unconditionally, ahead of the empty-queue early-out. A batch
    // that was only partly drained (a handler that unwound past the loop) must not strand a
    // duplicate, and no early-out may be able to skip that free.
    ClearBuffer(s_batch);
    if (s_queue.empty()) return;
    s_batch.swap(s_queue);

    s_draining  = true;
    s_abandoned = false;
    {
    // Clear the flag even if a replay unwinds past this loop (a C++ throw out of a handler): a
    // stuck s_draining would make every later drain a no-op. The abandoned tail is then freed by
    // the unconditional ClearBuffer at the top of the NEXT drain, which is why that one comes
    // before the early-out.
    struct DrainScope { ~DrainScope() { s_draining = false; } } drainScope;

    for (std::size_t i = 0; i < s_batch.size(); ++i) {
        // A flush from INSIDE a replay (a changelevel reached from JS) abandons the rest of the
        // batch: those entries name a world that no longer exists. The flush cannot clear the batch
        // itself without invalidating this walk, so it sets this flag and the trailing
        // ClearBuffer below performs the exactly-once free microseconds later.
        if (s_abandoned) break;

        // Take the entry OFF the batch before entering JS. A replay can re-enter the shim, so no
        // reference INTO the batch may be assumed live across it — moving makes the in-flight
        // entry's strings AND its duplicate drain-stack-owned for the whole call, which is also
        // what lets the args below be passed by pointer safely.
        S2Deferred e = std::move(s_batch[i]);
        s_batch[i].dup = nullptr;   // a moved-from raw pointer is still a COPY: the batch must not free it too
        DupGuard guard(e.dup);      // exactly-once free, on the throwing path too

        const int r = s_ops.replay ? s_ops.replay(e) : 0;

        // A replay that itself defers can only mean a bug — the drain runs with the isolate free.
        // Drop it with a named reason; re-queueing would spin the same entry across frames forever.
        if (r == S2_DISPATCH_DEFERRED) {
            Logf("[s2script] deferred-dispatch: replay re-deferred (HOST busy at drain) — dropped %s '%s'\n",
                 S2DeferredKindName(e.kind), e.a.c_str());
        }
    }
    }   // s_draining is false from here — the trailing free is not part of the walk
    ClearBuffer(s_batch);   // the abandoned tail, plus every husk the loop left behind
}

void S2Defer_Flush(const char* why) {
    const std::size_t nq = s_queue.size();
    ClearBuffer(s_queue);

    std::size_t nb = 0;
    const bool inDrain = s_draining;
    if (inDrain) {
        // NEVER touch the batch the drain is walking. clear() keeps capacity, so end() would
        // collapse onto begin() while the loop still sits at begin()+k, and the drain would walk
        // destroyed entries — switching on garbage, handing freed strings to core and freeing
        // whatever decoded as a duplicate. Abandon instead: the drain breaks at its next iteration
        // and frees the whole tail through its own trailing ClearBuffer.
        s_abandoned = true;
    } else {
        nb = s_batch.size();
        ClearBuffer(s_batch);
    }

    if (nq + nb > 0 || inDrain) {
        Logf("[s2script] deferred-dispatch: %s — flushed %d queued dispatch(es)%s\n",
             why ? why : "?", static_cast<int>(nq + nb),
             inDrain ? "; in-flight drain abandoned" : "");
    }
}

void S2Defer_ResetForTest() {
    ClearBuffer(s_queue);
    ClearBuffer(s_batch);
    s_draining  = false;
    s_abandoned = false;
}
