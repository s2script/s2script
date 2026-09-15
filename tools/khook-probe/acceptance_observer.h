#pragma once

// Probe FireEvent bookkeeping only. The same header is included by the live
// probe and by shim/tests/khook_acceptance_observer_test.cpp.
//
// A caller-owned invocation scope surrounds the entire FireEvent call. PRE
// and POST correlate by the pointer value saved by that caller and never
// dereference a consumed IGameEvent*. Nested scopes keep a foreign event from
// satisfying the outer case. Copied FireEventObservation metadata outlives the
// event object and the scope.

#include <string>

namespace s2khook {

// Count the independent Function detour at the real engine implementation.
// A virtual entry or a caller returning does not imply this boundary ran.
struct OriginalObservation {
    int pre = 0;
    int post = 0;
    int original = 0;
    int skipped = 0;
    void Pre() { ++pre; }
    void Post(bool skip) { ++post; if (skip) ++skipped; else ++original; }
    bool Once() const { return pre == 1 && post == 1 && original == 1 && skipped == 0; }
};

struct VoiceObservation {
    int requests = 0;
    int completions = 0;
    bool incoming = false;
    bool effective = false;
    bool stored = false;
    OriginalObservation original;
    void Begin(bool value) { ++requests; incoming = value; }
    void OriginalPost(bool skip, bool argument, bool engine_value) {
        original.Post(skip); effective = argument; stored = engine_value;
    }
    void End() { ++completions; }
    bool Exact(bool expected) const {
        return requests == 1 && completions == 1 && original.Once() && effective == expected && stored == expected;
    }
};

// Generation tags are written by the actual JS callbacks into one trace scoped
// to a native invocation. Foreign/malformed tokens remain visible as failures.
struct ScriptReloadInvocation {
    OriginalObservation original;
    int pre = 0, post = 0, stale = 0;
    void ReadTrace(const std::string& trace, int generation) {
        const std::string prefix = std::to_string(generation) + ":";
        std::size_t start = 0;
        while (start < trace.size()) {
            const auto end = trace.find(',', start);
            if (end == std::string::npos) { ++stale; break; }
            const auto token = trace.substr(start, end - start);
            if (token == prefix + "pre") ++pre;
            else if (token == prefix + "post") ++post;
            else ++stale;
            start = end + 1;
        }
    }
    bool Exact() const { return original.Once() && pre == 1 && post == 1 && stale == 0; }
};

struct ScriptReloadObservation {
    bool armed = false, before_seen = false, after_seen = false, old_resource_removed = false;
    int target_index = -1, target_serial = -1, marker_index = -1, marker_serial = -1;
    int old_generation = 0, generation = 0, settle_frames = 0;
    ScriptReloadInvocation before, after;
    bool Passed() const {
        return before_seen && after_seen && old_generation > 0 && generation > old_generation &&
               before.Exact() && after.Exact() && old_resource_removed;
    }
};

struct FireEventObservation {
    std::string run_id;
    std::string case_token;
    int nest_depth = 0;
    int pre_count = 0;
    int post_count = 0;
    int automatic_skip_count = 0;
    int automatic_original_count = 0;
    int listener_count = 0;
    void* event_ptr = nullptr;
};

class FireEventInvocationScope {
public:
    FireEventInvocationScope(const char* run_id, const char* case_token, void* event_ptr)
        : parent_(tls_current_),
          run_id_(run_id ? run_id : ""),
          case_token_(case_token ? case_token : ""),
          event_ptr_(event_ptr),
          depth_(parent_ ? parent_->depth_ + 1 : 0) {
        tls_current_ = this;
    }

    ~FireEventInvocationScope() {
        if (tls_current_ == this) {
            tls_current_ = parent_;
        }
    }

    FireEventInvocationScope(const FireEventInvocationScope&) = delete;
    FireEventInvocationScope& operator=(const FireEventInvocationScope&) = delete;

    FireEventObservation Copy() const {
        FireEventObservation o;
        o.run_id = run_id_;
        o.case_token = case_token_;
        o.nest_depth = depth_;
        o.pre_count = pre_;
        o.post_count = post_;
        o.automatic_skip_count = skip_;
        o.automatic_original_count = orig_;
        o.listener_count = listener_;
        o.event_ptr = event_ptr_;
        return o;
    }

    const char* run_id() const { return run_id_.c_str(); }
    const char* case_token() const { return case_token_.c_str(); }
    void* event_ptr() const { return event_ptr_; }
    int pre_count() const { return pre_; }
    int post_count() const { return post_; }
    int automatic_skip_count() const { return skip_; }
    int automatic_original_count() const { return orig_; }
    int listener_count() const { return listener_; }
    int depth() const { return depth_; }
    FireEventInvocationScope* parent() const { return parent_; }

    void NotePre() { pre_++; }
    void NotePost(bool automatic_skipped) {
        post_++;
        if (automatic_skipped) {
            skip_++;
        } else {
            orig_++;
        }
    }
    void NoteListener() { listener_++; }

    static FireEventInvocationScope* Current() { return tls_current_; }

    static FireEventInvocationScope* FindByPointer(void* event_ptr) {
        for (FireEventInvocationScope* s = tls_current_; s; s = s->parent_) {
            if (s->event_ptr_ == event_ptr) {
                return s;
            }
        }
        return nullptr;
    }

private:
    static thread_local FireEventInvocationScope* tls_current_;

    FireEventInvocationScope* parent_ = nullptr;
    std::string run_id_;
    std::string case_token_;
    void* event_ptr_ = nullptr;
    int depth_ = 0;
    int pre_ = 0;
    int post_ = 0;
    int skip_ = 0;
    int orig_ = 0;
    int listener_ = 0;
};

#if defined(S2_KHOOK_OBSERVER_TLS_DEFINED)
// Translation unit already defined the TLS object.
#else
inline thread_local FireEventInvocationScope* FireEventInvocationScope::tls_current_ = nullptr;
#endif

inline FireEventInvocationScope* CurrentScope() {
    return FireEventInvocationScope::Current();
}

// PRE/POST/listener: match the innermost live scope by pointer value. Never
// dereference event_ptr. A pointer that matches no fixture scope is ignored
// (nonfixture / already-consumed peer event).
inline void ObserveFireEventPre(void* event_ptr) {
    if (FireEventInvocationScope* s = FireEventInvocationScope::FindByPointer(event_ptr)) {
        s->NotePre();
    }
}

inline void ObserveFireEventPost(void* event_ptr, bool automatic_skipped) {
    if (FireEventInvocationScope* s = FireEventInvocationScope::FindByPointer(event_ptr)) {
        s->NotePost(automatic_skipped);
    }
}

inline void ObserveFireEventListener(void* event_ptr) {
    if (FireEventInvocationScope* s = FireEventInvocationScope::FindByPointer(event_ptr)) {
        s->NoteListener();
    }
}

}  // namespace s2khook
