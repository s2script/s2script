// Engine-free host test for tools/khook-probe/acceptance_observer.h.
//
// F2: FireEvent consumes its event before POST. The old probe POST called
// EventNameIs(ev) after that delete. This harness:
//   * invokes POST after an original that deletes the event
//   * ASan-aborts the legacy dereference (legacy_post_uaf)
//   * proves the shared observer never touches consumed memory
// Nested matching/nonmatching, peer-skipped automatic original, one/double
// listener delivery, and copied observation metadata vs object lifetime are
// covered against the same header the live probe includes.

#include "acceptance_observer.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>

static int g_fail = 0;
#define CHECK(cond, msg)                                                      \
    do {                                                                      \
        if (!(cond)) {                                                        \
            std::cerr << "FAIL: " << (msg) << "\n";                         \
            g_fail++;                                                         \
        } else {                                                              \
            std::cout << "ok:   " << (msg) << "\n";                          \
        }                                                                     \
        std::cout.flush();                                                     \
        std::cerr.flush();                                                     \
    } while (0)

namespace {

struct NamedEvent {
    char name[32];
    virtual const char* GetName() const { return name; }
    virtual ~NamedEvent() = default;
};

NamedEvent* NewEvent(const char* name) {
    auto* ev = new NamedEvent{};
    std::snprintf(ev->name, sizeof(ev->name), "%s", name ? name : "");
    return ev;
}

// Runtime-selected dummy: the compiler cannot prove which object is live.
__attribute__((noinline, noclone)) NamedEvent* SelectDummy(NamedEvent* a, NamedEvent* b) {
    volatile int selector = 0;
    return selector ? a : b;
}

// Old Hook_FireEventPost shape: dereference the event after original consumed it.
bool LegacyEventNameIs(const NamedEvent* ev, const char* want) {
    if (!ev || !want) {
        return false;
    }
    return std::strcmp(ev->name, want) == 0;
}

int RunLegacyPostUaf() {
    NamedEvent* storage = NewEvent("player_activate");
    NamedEvent* ev = SelectDummy(storage, storage);
    s2khook::FireEventInvocationScope scope("run-legacy", "fire_event_no_suppression", ev);
    s2khook::ObserveFireEventPre(ev);
    delete ev;  // original consumed the event
    // POST after deletion — ASan must catch this read.
    const bool match = LegacyEventNameIs(ev, "player_activate");
    std::cout << "legacy matched=" << (match ? 1 : 0) << " (should have aborted)\n";
    return match ? 0 : 1;
}

void TestConsumedPostDoesNotTouchMemory() {
    NamedEvent* storage = NewEvent("player_activate");
    NamedEvent* ev = SelectDummy(storage, storage);
    s2khook::FireEventObservation copy;
    {
        s2khook::FireEventInvocationScope scope("run-safe", "fire_event_no_suppression", ev);
        s2khook::ObserveFireEventPre(ev);
        s2khook::ObserveFireEventListener(ev);
        delete ev;
        s2khook::ObserveFireEventPost(ev, false);
        copy = scope.Copy();
        CHECK(scope.pre_count() == 1, "consumed: PRE counted by pointer");
        CHECK(scope.post_count() == 1, "consumed: POST counted by pointer");
        CHECK(scope.automatic_original_count() == 1, "consumed: automatic original once");
        CHECK(scope.automatic_skip_count() == 0, "consumed: not skipped");
        CHECK(scope.listener_count() == 1, "consumed: one listener during valid callback");
    }
    CHECK(copy.pre_count == 1, "copy survives scope: pre");
    CHECK(copy.post_count == 1, "copy survives scope: post");
    CHECK(copy.listener_count == 1, "copy survives event+scope lifetime");
    CHECK(copy.run_id == "run-safe", "copy run token");
    CHECK(copy.case_token == "fire_event_no_suppression", "copy case token");
}

void TestNestedMatchingDoesNotSatisfyOuter() {
    NamedEvent* outer = NewEvent("player_activate");
    s2khook::FireEventInvocationScope outer_scope("run-nest", "outer", outer);
    s2khook::ObserveFireEventPre(outer);

    NamedEvent* inner = NewEvent("player_activate");
    {
        s2khook::FireEventInvocationScope inner_scope("run-nest", "inner", inner);
        s2khook::ObserveFireEventPre(inner);
        s2khook::ObserveFireEventListener(inner);
        delete inner;
        s2khook::ObserveFireEventPost(inner, false);
        CHECK(inner_scope.pre_count() == 1, "nested matching: inner PRE");
        CHECK(inner_scope.listener_count() == 1, "nested matching: inner listener");
        CHECK(inner_scope.post_count() == 1, "nested matching: inner POST");
    }

    s2khook::ObserveFireEventListener(outer);
    delete outer;
    s2khook::ObserveFireEventPost(outer, false);
    CHECK(outer_scope.pre_count() == 1, "nested matching: outer PRE not inflated");
    CHECK(outer_scope.post_count() == 1, "nested matching: outer POST not inflated");
    CHECK(outer_scope.listener_count() == 1, "nested matching: outer listener not the inner one");
    CHECK(outer_scope.depth() == 0, "outer depth is 0");
}

void TestNestedNonmatchingDoesNotSatisfyOuter() {
    NamedEvent* outer = NewEvent("player_activate");
    s2khook::FireEventInvocationScope outer_scope("run-nest2", "outer", outer);
    s2khook::ObserveFireEventPre(outer);

    NamedEvent* inner = NewEvent("player_changename");
    {
        s2khook::FireEventInvocationScope inner_scope("run-nest2", "inner-other", inner);
        CHECK(inner_scope.depth() == 1, "inner nesting link depth=1");
        CHECK(inner_scope.parent() == &outer_scope, "inner restores to outer");
        s2khook::ObserveFireEventPre(inner);
        delete inner;
        s2khook::ObserveFireEventPost(inner, false);
    }

    CHECK(s2khook::CurrentScope() == &outer_scope, "enclosing scope restored after inner exit");
    delete outer;
    s2khook::ObserveFireEventPost(outer, false);
    CHECK(outer_scope.pre_count() == 1, "nonmatching nested: outer PRE");
    CHECK(outer_scope.post_count() == 1, "nonmatching nested: outer POST");
    CHECK(outer_scope.listener_count() == 0, "nested nonmatching did not deliver outer listener");
}

void TestPeerSkippedAutomaticOriginal() {
    NamedEvent* ev = NewEvent("player_activate");
    s2khook::FireEventInvocationScope scope("run-skip", "skip", ev);
    s2khook::ObserveFireEventPre(ev);
    // Peer already consumed; automatic original skipped. POST must not read ev.
    delete ev;
    s2khook::ObserveFireEventPost(ev, true);
    CHECK(scope.pre_count() == 1, "skipped: PRE");
    CHECK(scope.post_count() == 1, "skipped: POST");
    CHECK(scope.automatic_skip_count() == 1, "skipped: automatic skip recorded");
    CHECK(scope.automatic_original_count() == 0, "skipped: automatic original did not run");
    CHECK(scope.listener_count() == 0, "skipped: no listener delivery");
}

void TestExplicitOriginalThenAutomaticSkip() {
    NamedEvent* ev = NewEvent("player_activate");
    s2khook::FireEventInvocationScope scope("run-explicit", "handled-mask", ev);
    s2khook::ObserveFireEventPre(ev);
    // Explicit original inside PRE delivers to the listener, then automatic original is skipped.
    s2khook::ObserveFireEventListener(ev);
    delete ev;
    s2khook::ObserveFireEventPost(ev, true);
    CHECK(scope.automatic_skip_count() == 1, "explicit+skip: automatic skip");
    CHECK(scope.automatic_original_count() == 0, "explicit+skip: automatic original is 0");
    CHECK(scope.listener_count() == 1, "explicit+skip: engine delivery is not classified as zero");
}

void TestOneAndDoubleListenerDelivery() {
    NamedEvent* once = NewEvent("player_activate");
    s2khook::FireEventInvocationScope one("run-l1", "one", once);
    s2khook::ObserveFireEventPre(once);
    s2khook::ObserveFireEventListener(once);
    delete once;
    s2khook::ObserveFireEventPost(once, false);
    CHECK(one.listener_count() == 1, "one listener delivery");

    NamedEvent* twice = NewEvent("player_activate");
    s2khook::FireEventInvocationScope two("run-l2", "double", twice);
    s2khook::ObserveFireEventPre(twice);
    s2khook::ObserveFireEventListener(twice);
    s2khook::ObserveFireEventListener(twice);
    delete twice;
    s2khook::ObserveFireEventPost(twice, false);
    CHECK(two.listener_count() == 2, "double listener delivery is visible");
    CHECK(two.listener_count() != 1, "double is not collapsed to one");
}

void TestNonfixtureDoesNotInspectConsumed() {
    NamedEvent* foreign = NewEvent("round_start");
    s2khook::ObserveFireEventPre(foreign);
    delete foreign;
    // No fixture scope owns this pointer. POST must be a no-op, not a name read.
    s2khook::ObserveFireEventPost(foreign, false);
    CHECK(s2khook::CurrentScope() == nullptr, "nonfixture: no current scope");
}

void TestScopeRestoresOnEveryExit() {
    CHECK(s2khook::CurrentScope() == nullptr, "idle: no scope");
    NamedEvent* a = NewEvent("player_activate");
    NamedEvent* b = NewEvent("player_activate");
    {
        s2khook::FireEventInvocationScope outer("r", "a", a);
        CHECK(s2khook::CurrentScope() == &outer, "enter outer");
        {
            s2khook::FireEventInvocationScope inner("r", "b", b);
            CHECK(s2khook::CurrentScope() == &inner, "enter inner");
        }
        CHECK(s2khook::CurrentScope() == &outer, "inner destructor restored outer");
    }
    CHECK(s2khook::CurrentScope() == nullptr, "outer destructor restored idle");
    delete a;
    delete b;
}

void TestOriginalBoundaryAndVoiceRecall() {
    s2khook::OriginalObservation original;
    CHECK(!original.Once(), "no original invocation is not success");
    original.Pre(); original.Post(true);
    CHECK(!original.Once(), "suppressed function original is not success");
    original = {};
    original.Pre(); original.Post(false);
    CHECK(original.Once(), "one observed original boundary passes");
    original.Pre(); original.Post(false);
    CHECK(!original.Once(), "duplicate original remains visible");
    original = {}; original.Pre();
    CHECK(!original.Once(), "missing original POST is not success");

    for (bool probe_first : {false, true}) {
        s2khook::VoiceObservation voice;
        // Policy changes true -> false either before or after the probe PRE.
        voice.Begin(probe_first ? true : false);
        voice.original.Pre();
        voice.OriginalPost(false, false, false);
        voice.End();
        CHECK(voice.Exact(false), "Recall effective false/original once in either probe order");
    }
    s2khook::VoiceObservation skipped;
    skipped.Begin(true); skipped.End();
    CHECK(!skipped.Exact(true), "virtual Supersede before original cannot pass voice");
    s2khook::VoiceObservation wrong;
    wrong.Begin(true); wrong.original.Pre(); wrong.OriginalPost(false, false, true); wrong.End();
    CHECK(!wrong.Exact(false), "effective engine bit mismatch cannot pass voice");
}

}  // namespace

int main(int argc, char** argv) {
    const char* mode = (argc > 1) ? argv[1] : "";
    if (std::strcmp(mode, "legacy_post_uaf") == 0) {
        return RunLegacyPostUaf();
    }

    TestConsumedPostDoesNotTouchMemory();
    TestNestedMatchingDoesNotSatisfyOuter();
    TestNestedNonmatchingDoesNotSatisfyOuter();
    TestPeerSkippedAutomaticOriginal();
    TestExplicitOriginalThenAutomaticSkip();
    TestOneAndDoubleListenerDelivery();
    TestNonfixtureDoesNotInspectConsumed();
    TestScopeRestoresOnEveryExit();
    TestOriginalBoundaryAndVoiceRecall();

    if (g_fail) {
        std::cerr << "FAILED " << g_fail << " check(s)\n";
        return 1;
    }
    std::cout << "PASS: khook_acceptance_observer_test\n";
    return 0;
}
