// Console-output capture for `Server.commandEx` (SourceMod ServerCommandEx parity) — the engine-free
// half.
//
// SourceMod captures a command's output by flushing the command buffer, hooking console spew, and
// executing the command synchronously. Source 2 exposes no synchronous "execute the buffer now"
// (IVEngineServer2 has ServerCommand but no ServerExecute), so the shim brackets the command with two
// marker commands of its own instead:
//
//     s2s_capture_begin <cookie>
//     <the command>
//     s2s_capture_end <cookie>
//
// The engine runs its command buffer in order, so every line the logging system emits between the
// two markers is the command's output, including what `exec`, aliases and `;`-separated commands
// print. Output emitted before the begin marker (other queued commands, game logic) is never
// attributed to the capture.
//
// This TU owns that bookkeeping and nothing else: the queue of armed captures, which one is open,
// the size cap, the timeout, and the finished results. The tier0 logging listener, the marker
// ConCommands and the ServerCommand calls live in s2script_mm.cpp; shim/tests/console_capture_test.cpp
// drives this policy directly.
//
// Single-threaded by contract: every method is called on the server's main thread (the listener
// drops lines logged from any other thread before they reach Append).
#ifndef S2SCRIPT_CONSOLE_CAPTURE_H
#define S2SCRIPT_CONSOLE_CAPTURE_H

#include <cstddef>
#include <deque>
#include <string>
#include <vector>

struct S2CaptureResult {
    int cookie = 0;
    std::string text;
    bool truncated = false;   // the output exceeded the cap; `text` holds the first `cap` bytes
};

class S2ConsoleCapture {
public:
    // `cap` bounds one capture's text. `timeout` (seconds) finishes a capture whose markers never
    // ran — the buffer was cleared by a map change, or a stray quote swallowed the end marker.
    explicit S2ConsoleCapture(std::size_t cap = 16 * 1024, double timeout = 10.0, std::size_t max_pending = 64)
        : cap_(cap), timeout_(timeout), max_pending_(max_pending) {}

    // Arm a capture for a command just queued. False (nothing armed) for cookie 0, a cookie already
    // in flight, or when `max_pending` captures are outstanding.
    bool Arm(int cookie, double now);

    // The begin marker ran. Opens `cookie`'s capture. Armed captures queued before it never had
    // their begin marker run, so they finish empty. A capture still open (its end marker was lost)
    // finishes with what it has. Unknown cookies (forged, or already expired) are ignored.
    void Begin(int cookie);

    // One logged message. Appended to the open capture, if any; otherwise ignored.
    void Append(const char* msg);

    // The end marker ran. Finishes `cookie`'s capture if it is the open one.
    void End(int cookie);

    // Finish every capture armed more than `timeout` seconds before `now`, with what it collected.
    void Expire(double now);

    // Whether a capture is armed or open, i.e. whether the logging listener needs to be registered.
    bool Active() const { return open_ || !armed_.empty(); }

    // Whether a capture is open right now (lines are being collected).
    bool Open() const { return open_; }

    // Finished captures, oldest first. Clears the list.
    std::vector<S2CaptureResult> TakeDone();

    // Drop everything (shim unload). Nothing is reported.
    void Reset();

private:
    struct Entry { int cookie; double armed_at; S2CaptureResult result; };

    void Finish(Entry& e);

    std::size_t cap_;
    double timeout_;
    std::size_t max_pending_;
    std::deque<Entry> armed_;      // armed, begin marker not yet run; oldest first
    bool open_ = false;
    Entry current_{};              // valid while open_
    std::vector<S2CaptureResult> done_;
};

#endif
