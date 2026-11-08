// Console-output capture policy. See console_capture.h.
#include "console_capture.h"

#include <cstring>

bool S2ConsoleCapture::Arm(int cookie, double now) {
    if (cookie == 0) return false;
    if (armed_.size() + (open_ ? 1 : 0) >= max_pending_) return false;
    if (open_ && current_.cookie == cookie) return false;
    for (const auto& e : armed_) if (e.cookie == cookie) return false;
    Entry e;
    e.cookie = cookie;
    e.armed_at = now;
    e.result.cookie = cookie;
    armed_.push_back(std::move(e));
    return true;
}

void S2ConsoleCapture::Begin(int cookie) {
    bool known = false;
    for (const auto& e : armed_) if (e.cookie == cookie) { known = true; break; }
    if (!known) return;
    if (open_) { Finish(current_); open_ = false; }
    // Markers run in queue order, so anything armed before `cookie` lost its begin marker.
    while (!armed_.empty() && armed_.front().cookie != cookie) {
        Finish(armed_.front());
        armed_.pop_front();
    }
    current_ = std::move(armed_.front());
    armed_.pop_front();
    open_ = true;
}

void S2ConsoleCapture::Append(const char* msg) {
    if (!open_ || !msg) return;
    std::string& text = current_.result.text;
    const std::size_t n = std::strlen(msg);
    if (text.size() + n <= cap_) { text.append(msg, n); return; }
    text.append(msg, cap_ - text.size());
    current_.result.truncated = true;
}

void S2ConsoleCapture::End(int cookie) {
    if (!open_ || current_.cookie != cookie) return;
    Finish(current_);
    open_ = false;
}

void S2ConsoleCapture::Expire(double now) {
    if (open_ && now - current_.armed_at >= timeout_) { Finish(current_); open_ = false; }
    while (!armed_.empty() && now - armed_.front().armed_at >= timeout_) {
        Finish(armed_.front());
        armed_.pop_front();
    }
}

std::vector<S2CaptureResult> S2ConsoleCapture::TakeDone() {
    std::vector<S2CaptureResult> out;
    out.swap(done_);
    return out;
}

void S2ConsoleCapture::Reset() {
    armed_.clear();
    open_ = false;
    current_ = Entry{};
    done_.clear();
}

void S2ConsoleCapture::Finish(Entry& e) {
    done_.push_back(std::move(e.result));
    e.result = S2CaptureResult{};
}
