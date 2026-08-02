// Unit test for the REAL deferred-dispatch queue (shim/src/defer_queue.cpp) — the drain, the double
// buffer, the bound, and the exactly-once free of every queued IGameEvent duplicate.
//
// WHY THIS EXISTS. The drain's original defect — a flush called from INSIDE a replay clearing the
// very buffer the drain was range-for'ing over — survived a whole workflow because the only test
// coverage was a Rust MOCK drain that took its batch into a LOCAL vector. A mock like that is
// structurally incapable of reproducing a globals-plus-swap interaction. So this drives the shipped
// code: the same globals, the same swap, the same free path, with only the engine and core calls
// faked through S2DeferOps.
//
// Self-contained: no SDK, no engine, no isolate, no V8. scripts/test-defer-queue.sh compiles it
// with _GLIBCXX_DEBUG (which turns an invalidated-iterator walk into a deterministic abort instead
// of luck) and with ASan/UBSan when the toolchain has them.
#include "../src/defer_queue.h"
#include "../include/s2script_core.h"

#include <cstdio>
#include <cstring>
#include <functional>
#include <iostream>
#include <map>
#include <string>
#include <vector>

static int g_fail = 0;
#define CHECK(cond, msg)                                                        \
    do {                                                                        \
        if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; g_fail++; }      \
        else         { std::cout << "ok:   " << (msg) << "\n"; }                \
    } while (0)

// --- the fake engine -------------------------------------------------------------------------
// A "duplicate" is a heap object with a name. Every mint and every free is accounted for, so a
// double free, a free of something never minted, and a leak are all assertable.
namespace {

struct FakeEvent {
    std::string name;
};

struct Observed {
    S2DeferredKind kind;
    std::string    a;
    std::string    b;
    std::string    c;
    int            i;
    bool           hadDup;
};

std::map<FakeEvent*, int>   g_freeCount;    // every pointer ever handed to free_event
std::vector<FakeEvent*>     g_minted;       // every pointer ever returned by duplicate_event
std::vector<Observed>       g_replays;      // what the drain actually replayed, in order
std::vector<std::string>    g_log;
int                         g_dupCalls = 0;
bool                        g_dupFails = false;   // simulate "game-event deferral degraded"
int                         g_freeOfUnknown = 0;  // a free of a pointer we never minted == corruption

// What the next replay should do, keyed by replay index. Lets a test script a flush / push / drain
// / throw / re-defer from INSIDE the drain, which is the whole point.
std::function<int(const S2Deferred&, int)> g_onReplay;

void* FakeDuplicate(void* ev) {
    g_dupCalls++;
    if (g_dupFails) return nullptr;
    FakeEvent* src = static_cast<FakeEvent*>(ev);
    FakeEvent* dup = new FakeEvent{src ? src->name : std::string("?")};
    g_minted.push_back(dup);
    g_freeCount[dup] = 0;
    return dup;
}

void FakeFree(void* p) {
    FakeEvent* e = static_cast<FakeEvent*>(p);
    auto it = g_freeCount.find(e);
    if (it == g_freeCount.end()) { g_freeOfUnknown++; return; }
    it->second++;
    if (it->second == 1) delete e;   // a second free would be a use-after-free; counted, not performed
}

int FakeReplay(const S2Deferred& e) {
    const int index = static_cast<int>(g_replays.size());
    g_replays.push_back(Observed{e.kind, e.a, e.b, e.c, e.i, e.dup != nullptr});
    return g_onReplay ? g_onReplay(e, index) : 0;
}

void FakeLog(const char* line) { g_log.push_back(line ? line : ""); }

void ResetWorld() {
    S2Defer_ResetForTest();
    g_freeCount.clear();
    g_minted.clear();
    g_replays.clear();
    g_log.clear();
    g_dupCalls = 0;
    g_dupFails = false;
    g_freeOfUnknown = 0;
    g_onReplay = nullptr;

    S2DeferOps ops;
    ops.duplicate_event = &FakeDuplicate;
    ops.free_event      = &FakeFree;
    ops.replay          = &FakeReplay;
    ops.log             = &FakeLog;
    S2Defer_SetOps(ops);
}

bool EveryDuplicateFreedExactlyOnce() {
    for (FakeEvent* p : g_minted) {
        auto it = g_freeCount.find(p);
        if (it == g_freeCount.end() || it->second != 1) return false;
    }
    return g_freeOfUnknown == 0;
}

bool LogContains(const char* needle) {
    for (const std::string& l : g_log)
        if (l.find(needle) != std::string::npos) return true;
    return false;
}

// Names are deliberately LONGER than libstdc++'s small-string buffer, so a std::string destroyed
// out from under the drain really does free its heap buffer — which is what makes a
// use-after-destroy detectable rather than accidentally benign.
std::string LongName(const char* prefix, int n) {
    char buf[64];
    snprintf(buf, sizeof(buf), "%s_deferred_probe_%04d", prefix, n);
    return buf;
}

}  // namespace

// --- the tests --------------------------------------------------------------------------------

static void test_replays_in_push_order_and_empties_the_queue() {
    ResetWorld();
    S2Defer_PushScalar(S2_DEFERRED_CLIENT_EVENT, "disconnect", nullptr, nullptr, 3);
    S2Defer_PushScalar(S2_DEFERRED_MAP_START, "de_dust2", nullptr, nullptr, 0);
    S2Defer_PushScalar(S2_DEFERRED_CVAR_CHANGE, "mp_freezetime", "5", "15", 0);
    CHECK(S2Defer_QueuedCount() == 3, "three pushes queue three entries");

    S2Defer_Drain();
    CHECK(g_replays.size() == 3, "the drain replays all three");
    CHECK(g_replays[0].kind == S2_DEFERRED_CLIENT_EVENT && g_replays[0].i == 3,
          "FIFO: the client event replays first, with its slot");
    CHECK(g_replays[1].kind == S2_DEFERRED_MAP_START && g_replays[1].a == "de_dust2",
          "FIFO: map_start second, with its map name");
    CHECK(g_replays[2].kind == S2_DEFERRED_CVAR_CHANGE && g_replays[2].b == "5" && g_replays[2].c == "15",
          "FIFO: cvar_change third, with new AND old value");
    CHECK(S2Defer_QueuedCount() == 0 && S2Defer_BatchCount() == 0, "both buffers are empty afterwards");
}

static void test_a_defer_from_inside_a_replay_lands_in_the_next_drain() {
    ResetWorld();
    S2Defer_PushScalar(S2_DEFERRED_CLIENT_EVENT, "active", nullptr, nullptr, 1);
    g_onReplay = [](const S2Deferred& e, int) -> int {
        if (e.a == "active") S2Defer_PushScalar(S2_DEFERRED_CLIENT_EVENT, "disconnect", nullptr, nullptr, 1);
        return 0;
    };
    S2Defer_Drain();
    CHECK(g_replays.size() == 1, "the re-deferred push does NOT extend the batch being walked");
    CHECK(S2Defer_QueuedCount() == 1, "it is queued for the next drain");

    S2Defer_Drain();
    CHECK(g_replays.size() == 2 && g_replays[1].a == "disconnect", "the next drain delivers it");
}

static void test_overflow_drops_the_newest_and_names_it() {
    ResetWorld();
    const int max = S2Defer_Max();
    for (int i = 0; i < max + 2; i++)
        S2Defer_PushScalar(S2_DEFERRED_CLIENT_EVENT, LongName("client", i).c_str(), nullptr, nullptr, i);
    CHECK(static_cast<int>(S2Defer_QueuedCount()) == max, "the queue does not grow past the bound");
    CHECK(LogContains("queue full") && LogContains("(newest)"), "the overflow names what it dropped");

    // A game_event that overflows must not mint a duplicate it would then have to free.
    const int dupsBefore = g_dupCalls;
    FakeEvent ev{"player_death"};
    S2Defer_PushGameEvent("player_death", &ev);
    CHECK(g_dupCalls == dupsBefore, "capacity is checked BEFORE DuplicateEvent — an overflow cannot leak an event");
}

static void test_game_event_duplicates_are_freed_exactly_once() {
    ResetWorld();
    FakeEvent a{"player_death"}, b{"player_hurt"};
    S2Defer_PushGameEvent("player_death", &a);
    S2Defer_PushGameEvent("player_hurt", &b);
    CHECK(g_minted.size() == 2, "each queued game_event carries its own duplicate");

    S2Defer_Drain();
    CHECK(g_replays.size() == 2 && g_replays[0].hadDup && g_replays[1].hadDup,
          "the replay sees the duplicate (handlers read the REAL field values)");
    CHECK(EveryDuplicateFreedExactlyOnce(), "every duplicate is freed exactly once by the drain");
}

static void test_degraded_duplication_drops_the_game_event_by_name() {
    ResetWorld();
    g_dupFails = true;
    FakeEvent a{"player_death"};
    S2Defer_PushGameEvent("player_death", &a);
    CHECK(S2Defer_QueuedCount() == 0, "a game_event is not queued without a duplicate");
    CHECK(LogContains("DuplicateEvent unavailable") && LogContains("player_death"),
          "the drop names the event (scalar deferral is unaffected)");
}

// THE REGRESSION TEST for the drain-vs-flush defect.
//
// Hook_StartupServer flushes the queue at the top of a map start, and a map start is reachable FROM
// a replay: the shim's own comment says the deferral exists because "a plugin-declared engine call
// that reaches a changelevel from JS" happens. That puts the flush INSIDE the drain loop. Clearing
// the batch there collapses end() onto begin() while the loop sits at begin()+k, and the drain then
// walks destroyed entries: switching on garbage, handing freed strings to core, and FreeEvent-ing
// any word that decodes as a duplicate.
static void test_a_flush_from_inside_a_replay_does_not_corrupt_the_drain() {
    ResetWorld();
    std::vector<FakeEvent> events;
    events.reserve(8);
    for (int i = 0; i < 8; i++) events.push_back(FakeEvent{LongName("evt", i)});
    for (int i = 0; i < 8; i++) S2Defer_PushGameEvent(events[i].name.c_str(), &events[i]);
    CHECK(S2Defer_QueuedCount() == 8 && g_minted.size() == 8, "eight game events queued, eight duplicates minted");

    // The third replay changes the level.
    g_onReplay = [](const S2Deferred&, int index) -> int {
        if (index == 2) S2Defer_Flush("map start");
        return 0;
    };
    S2Defer_Drain();

    CHECK(g_replays.size() == 3, "the drain stops at the flush — entries from the old world are abandoned");
    for (size_t i = 0; i < g_replays.size() && i < 3; i++)
        CHECK(g_replays[i].a == LongName("evt", static_cast<int>(i)),
              "every replay before the flush is a real entry, in order");
    CHECK(EveryDuplicateFreedExactlyOnce(),
          "every duplicate — replayed and abandoned — is freed exactly once across the flush");
    CHECK(g_freeOfUnknown == 0, "nothing that was never minted is handed to FreeEvent");
    CHECK(S2Defer_QueuedCount() == 0 && S2Defer_BatchCount() == 0, "both buffers end empty");
    CHECK(!S2Defer_Draining(), "the drain flag is cleared");
    CHECK(LogContains("in-flight drain abandoned"), "the flush names what it did to the running drain");
}

// The same hazard from the other direction: anything pushed by a replay that then gets flushed must
// not be replayed, and a drain re-entered from a replay must not swap the batch out from under the
// loop that is walking it.
static void test_a_drain_re_entered_from_a_replay_is_a_no_op() {
    ResetWorld();
    S2Defer_PushScalar(S2_DEFERRED_CLIENT_EVENT, LongName("client", 0).c_str(), nullptr, nullptr, 0);
    S2Defer_PushScalar(S2_DEFERRED_CLIENT_EVENT, LongName("client", 1).c_str(), nullptr, nullptr, 1);
    g_onReplay = [](const S2Deferred&, int index) -> int {
        if (index == 0) {
            CHECK(S2Defer_Draining(), "a replay observes that a drain is in progress");
            S2Defer_PushScalar(S2_DEFERRED_MAP_START, "de_inferno", nullptr, nullptr, 0);
            S2Defer_Drain();   // must NOT nest
        }
        return 0;
    };
    S2Defer_Drain();
    CHECK(g_replays.size() == 2, "the nested drain replayed nothing extra");
    CHECK(S2Defer_QueuedCount() == 1, "what the replay pushed is still waiting for the next drain");
    S2Defer_Drain();
    CHECK(g_replays.size() == 3 && g_replays[2].kind == S2_DEFERRED_MAP_START, "and arrives one drain later");
}

// A C++ throw out of a replay must not strand a duplicate, must not leave the drain flag stuck, and
// must not leave the batch holding duplicates nothing will ever free.
static void test_a_throwing_replay_frees_its_duplicate_and_unsticks_the_drain() {
    ResetWorld();
    FakeEvent a{"player_death"}, b{"player_hurt"};
    S2Defer_PushGameEvent("player_death", &a);
    S2Defer_PushGameEvent("player_hurt", &b);
    g_onReplay = [](const S2Deferred&, int index) -> int {
        if (index == 0) throw std::string("a handler unwound past the drain");
        return 0;
    };
    bool threw = false;
    try { S2Defer_Drain(); } catch (const std::string&) { threw = true; }
    CHECK(threw, "the throw propagates (the caller's catch_unwind boundary owns it)");
    CHECK(!S2Defer_Draining(), "the drain flag is cleared on the unwinding path");
    CHECK(g_freeCount[g_minted[0]] == 1, "the in-flight duplicate is freed by its own guard");

    // The next drain's unconditional top-of-function clear frees the stranded tail.
    g_onReplay = nullptr;
    S2Defer_Drain();
    CHECK(EveryDuplicateFreedExactlyOnce(), "the stranded tail is freed exactly once by the next drain");
}

// #5: the early-out must not be able to skip the batch clear.
static void test_the_empty_queue_early_out_still_clears_the_previous_batch() {
    ResetWorld();
    FakeEvent a{"player_death"};
    S2Defer_PushGameEvent("player_death", &a);
    g_onReplay = [](const S2Deferred&, int) -> int { throw 1; };
    try { S2Defer_Drain(); } catch (int) {}
    // Nothing new is queued, so the next drain takes the early-out — and must still free the batch.
    S2Defer_Drain();
    CHECK(EveryDuplicateFreedExactlyOnce(), "an empty-queue drain still empties a stranded batch");
    CHECK(S2Defer_BatchCount() == 0, "the batch is empty after the early-out drain");
}

static void test_a_replay_that_re_defers_is_dropped_and_named() {
    ResetWorld();
    S2Defer_PushScalar(S2_DEFERRED_MAP_START, "de_nuke", nullptr, nullptr, 0);
    g_onReplay = [](const S2Deferred&, int) -> int { return S2_DISPATCH_DEFERRED; };
    S2Defer_Drain();
    CHECK(S2Defer_QueuedCount() == 0, "a re-deferred replay is NOT re-queued (it would spin forever)");
    CHECK(LogContains("replay re-deferred") && LogContains("de_nuke"), "and it is dropped by name");
}

static void test_flush_outside_a_drain_frees_everything() {
    ResetWorld();
    FakeEvent a{"player_death"}, b{"player_hurt"};
    S2Defer_PushGameEvent("player_death", &a);
    S2Defer_PushGameEvent("player_hurt", &b);
    S2Defer_Flush("unload");
    CHECK(S2Defer_QueuedCount() == 0, "the flush empties the queue");
    CHECK(EveryDuplicateFreedExactlyOnce(), "and frees every duplicate exactly once");
    CHECK(LogContains("unload") && LogContains("flushed 2"), "and names the occasion and the count");
}

int main() {
    // Unbuffered: a regression here ABORTS (a debug-iterator or sanitizer trap), and abort() does
    // not flush iostreams — without this the output stops several checks before the real one.
    std::cout << std::unitbuf;

    test_replays_in_push_order_and_empties_the_queue();
    test_a_defer_from_inside_a_replay_lands_in_the_next_drain();
    test_overflow_drops_the_newest_and_names_it();
    test_game_event_duplicates_are_freed_exactly_once();
    test_degraded_duplication_drops_the_game_event_by_name();
    test_a_flush_from_inside_a_replay_does_not_corrupt_the_drain();
    test_a_drain_re_entered_from_a_replay_is_a_no_op();
    test_a_throwing_replay_frees_its_duplicate_and_unsticks_the_drain();
    test_the_empty_queue_early_out_still_clears_the_previous_batch();
    test_a_replay_that_re_defers_is_dropped_and_named();
    test_flush_outside_a_drain_frees_everything();

    S2Defer_ResetForTest();
    if (g_fail) { std::cerr << "defer_queue_test: " << g_fail << " FAILURE(S)\n"; return 1; }
    std::cout << "defer_queue_test: all checks passed\n";
    return 0;
}
