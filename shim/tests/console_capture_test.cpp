// Drives the shipped console-capture policy (shim/src/console_capture.cpp). Run by
// scripts/test-console-capture.sh from scripts/ci-native.sh.
#include "console_capture.h"

#include <cstdio>
#include <cstdlib>
#include <string>

static int g_failures = 0;
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); ++g_failures; } } while (0)

// Only lines between the markers are kept; the listener can come and go.
static void brackets_the_command() {
    S2ConsoleCapture c;
    CHECK(!c.Active());
    CHECK(c.Arm(7, 0.0));
    CHECK(c.Active());
    CHECK(!c.Open());
    c.Append("before the begin marker\n");          // other queued commands, game logic
    c.Begin(7);
    CHECK(c.Open());
    c.Append("hostname: test\n");
    c.Append("players : 1\n");
    c.End(7);
    c.Append("after the end marker\n");
    CHECK(!c.Active());
    auto done = c.TakeDone();
    CHECK(done.size() == 1);
    CHECK(done[0].cookie == 7);
    CHECK(done[0].text == "hostname: test\nplayers : 1\n");
    CHECK(!done[0].truncated);
    CHECK(c.TakeDone().empty());
}

// Two captures queued back to back keep their own output.
static void sequential_captures_do_not_mix() {
    S2ConsoleCapture c;
    CHECK(c.Arm(1, 0.0));
    CHECK(c.Arm(2, 0.0));
    c.Begin(1); c.Append("one\n"); c.End(1);
    c.Begin(2); c.Append("two\n"); c.End(2);
    auto done = c.TakeDone();
    CHECK(done.size() == 2);
    CHECK(done[0].cookie == 1 && done[0].text == "one\n");
    CHECK(done[1].cookie == 2 && done[1].text == "two\n");
}

// A lost marker never wedges later captures.
static void lost_markers_finish_what_they_have() {
    S2ConsoleCapture c;
    CHECK(c.Arm(1, 0.0));
    CHECK(c.Arm(2, 0.0));
    CHECK(c.Arm(3, 0.0));
    c.Begin(2);                                      // 1's begin marker never ran
    c.Append("two\n");                               // 2's end marker is lost too
    c.Begin(3);
    c.Append("three\n");
    c.End(3);
    auto done = c.TakeDone();
    CHECK(done.size() == 3);
    CHECK(done[0].cookie == 1 && done[0].text.empty());      // finished when 2's begin ran
    CHECK(done[1].cookie == 2 && done[1].text == "two\n");   // finished when 3's begin ran
    CHECK(done[2].cookie == 3 && done[2].text == "three\n");
    CHECK(!c.Active());
}

// Forged or stale markers do nothing.
static void unknown_markers_are_ignored() {
    S2ConsoleCapture c;
    c.Begin(9); c.End(9);
    CHECK(!c.Open());
    CHECK(c.Arm(4, 0.0));
    c.Begin(5);
    CHECK(!c.Open());
    c.Begin(4);
    c.End(5);
    CHECK(c.Open());
    c.End(4);
    CHECK(c.TakeDone().size() == 1);
}

static void arming_refusals() {
    S2ConsoleCapture c(16, 10.0, 2);
    CHECK(!c.Arm(0, 0.0));
    CHECK(c.Arm(1, 0.0));
    CHECK(!c.Arm(1, 0.0));
    CHECK(c.Arm(2, 0.0));
    CHECK(!c.Arm(3, 0.0));                           // max_pending
    c.Begin(1);
    CHECK(!c.Arm(1, 0.0));                           // the open cookie is still in flight
    c.End(1);
    CHECK(c.Arm(3, 0.0));
}

static void output_is_capped() {
    S2ConsoleCapture c(8);
    CHECK(c.Arm(1, 0.0));
    c.Begin(1);
    c.Append("12345");
    c.Append("67890");
    c.Append("more");
    c.End(1);
    auto done = c.TakeDone();
    CHECK(done.size() == 1);
    CHECK(done[0].text == "12345678");
    CHECK(done[0].truncated);
}

static void timeout_finishes_armed_and_open() {
    S2ConsoleCapture c(64, 10.0);
    CHECK(c.Arm(1, 0.0));
    CHECK(c.Arm(2, 5.0));
    c.Begin(1);
    c.Append("partial\n");
    c.Expire(9.9);
    CHECK(c.TakeDone().empty());
    c.Expire(10.0);
    auto done = c.TakeDone();
    CHECK(done.size() == 1 && done[0].cookie == 1 && done[0].text == "partial\n");
    CHECK(!c.Open());
    c.Append("dropped\n");
    c.Expire(15.0);
    done = c.TakeDone();
    CHECK(done.size() == 1 && done[0].cookie == 2 && done[0].text.empty());
    CHECK(!c.Active());
}

static void reset_drops_everything() {
    S2ConsoleCapture c;
    CHECK(c.Arm(1, 0.0));
    CHECK(c.Arm(2, 0.0));
    c.Begin(1);
    c.Append("x");
    c.Reset();
    CHECK(!c.Active());
    CHECK(c.TakeDone().empty());
}

int main() {
    brackets_the_command();
    sequential_captures_do_not_mix();
    lost_markers_finish_what_they_have();
    unknown_markers_are_ignored();
    arming_refusals();
    output_is_capped();
    timeout_finishes_armed_and_open();
    reset_drops_everything();
    if (g_failures) { std::fprintf(stderr, "console_capture_test: %d failure(s)\n", g_failures); return 1; }
    std::printf("console_capture_test: all passed\n");
    return 0;
}
