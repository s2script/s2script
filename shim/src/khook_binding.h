#pragma once

// Checked typed bindings over the pinned KHook Function/Virtual helpers.
// Pin-specific: inherit their constructors and inspect protected associated-id /
// slot-to-id maps under those helpers' existing locks. Not an alternate detour
// engine and not a public plugin API.
//
// Public FFI conventions (unchanged): SDKHooks VP add uses nonzero success;
// declarative S2_HookInstall uses 0 success / -1 failure.
//
// Observe: the only public leave path is the returned S2HookObserve guard.
// Keep it alive for the callback (`auto obs = binding.Observe(...)`). A
// discarded temporary is not the handler pattern; the destructor still
// EndObserves, so it does not leak an invocation hold.
//
// Retirement retains S2HookBindingState (shared_ptr bookkeeping), not the
// typed S2CheckedFunction / S2CheckedVirtual object. ~Function / ~Virtual
// still call RemoveHook synchronously. Callers must keep the checked binding
// alive until Snapshot().state == Removed.
//
// S2Hook_DrainRetirement() true means "not on a callback stack", not "queue
// empty". Unload also requires S2Hook_RetirementPending() == 0. Do not
// busy-wait completions on the game thread.

#include <khook.hpp>

#include <atomic>
#include <array>
#include <cstddef>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

enum class S2HookState { Failed, Pending, Active, Removing, Removed };
enum class S2HookLifecycle { Running, Retiring, Ready };

struct S2HookReceipt {
    KHook::HookID_t id = KHook::INVALID_HOOK;
    S2HookState state = S2HookState::Failed;
    std::string reason;
    bool Accepted() const {
        return id != KHook::INVALID_HOOK &&
               (state == S2HookState::Pending || state == S2HookState::Active);
    }
};

struct S2HookBindingState {
    mutable std::mutex mu;
    struct Owned {
        KHook::HookID_t id = KHook::INVALID_HOOK;
        S2HookState state = S2HookState::Failed;
        int invocations = 0;
        bool remove_scheduled = false;
        bool complete = false;
        const void* capsule = nullptr;
    };
    std::unordered_map<KHook::HookID_t, Owned> owned;
    KHook::HookID_t last_id = KHook::INVALID_HOOK;
    S2HookState last_state = S2HookState::Failed;
    std::string reason;
};

namespace s2hook_detail {

struct ObserveFrame {
    S2HookBindingState* state = nullptr;
    KHook::HookID_t id = KHook::INVALID_HOOK;
    const void* capsule = nullptr;
};

inline thread_local int g_callback_depth = 0;
inline thread_local std::vector<ObserveFrame> g_observe_stack;
inline std::atomic<int> g_active_callbacks{0};
inline std::atomic<S2HookLifecycle> g_lifecycle{S2HookLifecycle::Running};

struct RetirementEntry {
    std::shared_ptr<S2HookBindingState> state;
    KHook::HookID_t id = KHook::INVALID_HOOK;
};

inline std::mutex g_retire_mu;
inline std::vector<RetirementEntry> g_retire;

inline bool NoteObserve(const std::shared_ptr<S2HookBindingState>& state, KHook::HookID_t id) {
    if (!state || id == KHook::INVALID_HOOK) {
        return false;
    }
    {
        std::lock_guard<std::mutex> lock(state->mu);
        auto it = state->owned.find(id);
        if (it == state->owned.end()) {
            return false;
        }
        if (it->second.state == S2HookState::Failed || it->second.state == S2HookState::Removed) {
            return false;
        }
        it->second.invocations++;
        if (it->second.state == S2HookState::Pending) {
            it->second.state = S2HookState::Active;
            if (state->last_id == id) {
                state->last_state = S2HookState::Active;
            }
        }
    }
    g_callback_depth++;
    s2hook_detail::g_active_callbacks.fetch_add(1, std::memory_order_acq_rel);
    const void* capsule = nullptr;
    {
        std::lock_guard<std::mutex> lock(state->mu);
        auto it = state->owned.find(id);
        if (it != state->owned.end()) capsule = it->second.capsule;
    }
    g_observe_stack.push_back({state.get(), id, capsule});
    return true;
}

inline void LeaveObserve() {
    if (g_observe_stack.empty()) {
        return;
    }
    ObserveFrame frame = g_observe_stack.back();
    g_observe_stack.pop_back();
    if (g_callback_depth > 0) {
        g_callback_depth--;
    }
    s2hook_detail::g_active_callbacks.fetch_sub(1, std::memory_order_acq_rel);
    if (!frame.state) {
        return;
    }
    std::lock_guard<std::mutex> lock(frame.state->mu);
    auto it = frame.state->owned.find(frame.id);
    if (it == frame.state->owned.end()) {
        return;
    }
    if (it->second.invocations > 0) {
        it->second.invocations--;
    }
    if (it->second.complete && it->second.invocations == 0) {
        it->second.state = S2HookState::Removed;
        if (frame.state->last_id == frame.id) {
            frame.state->last_state = S2HookState::Removed;
        }
    }
}

inline void OnRemoved(KHook::HookID_t id, void* context) {
    auto* st = static_cast<S2HookBindingState*>(context);
    if (!st) {
        return;
    }
    std::lock_guard<std::mutex> lock(st->mu);
    auto it = st->owned.find(id);
    if (it == st->owned.end()) {
        return;
    }
    it->second.complete = true;
    if (it->second.invocations == 0) {
        it->second.state = S2HookState::Removed;
        if (st->last_id == id) {
            st->last_state = S2HookState::Removed;
        }
    }
}

}  // namespace s2hook_detail

struct S2HookTerminalPermit {
    bool IsValid() const { return valid_ && capsule_ != nullptr; }
    const void* Capsule() const { return capsule_; }
private:
    const void* capsule_ = nullptr;
    bool valid_ = false;
    S2HookTerminalPermit(const void* capsule, bool valid) : capsule_(capsule), valid_(valid) {}
    S2HookTerminalPermit() = default;
    friend S2HookTerminalPermit S2Hook_CurrentTerminalPermit();
};

inline S2HookTerminalPermit S2Hook_CurrentTerminalPermit() {
    if (s2hook_detail::g_observe_stack.size() != 1 ||
        s2hook_detail::g_active_callbacks.load(std::memory_order_acquire) != 1) return {};
    const void* capsule = s2hook_detail::g_observe_stack.back().capsule;
    return capsule ? S2HookTerminalPermit(capsule, true) : S2HookTerminalPermit{};
}

// Move-only RAII hold for one Observe entry. Destructor calls LeaveObserve
// once. Nested guards are LIFO, matching the TLS observe stack.
class [[nodiscard]] S2HookObserve {
public:
    S2HookObserve() noexcept = default;
    explicit S2HookObserve(bool armed) noexcept : armed_(armed) {}

    S2HookObserve(S2HookObserve&& other) noexcept : armed_(other.armed_) {
        other.armed_ = false;
    }
    S2HookObserve& operator=(S2HookObserve&& other) noexcept {
        if (this != &other) {
            reset();
            armed_ = other.armed_;
            other.armed_ = false;
        }
        return *this;
    }

    ~S2HookObserve() { reset(); }

    S2HookObserve(const S2HookObserve&) = delete;
    S2HookObserve& operator=(const S2HookObserve&) = delete;

    explicit operator bool() const noexcept { return armed_; }

    void reset() noexcept {
        if (!armed_) {
            return;
        }
        armed_ = false;
        s2hook_detail::LeaveObserve();
    }

private:
    bool armed_ = false;
};

inline void S2Hook_SetLifecycle(S2HookLifecycle state) {
    s2hook_detail::g_lifecycle.store(state, std::memory_order_release);
}

inline S2HookLifecycle S2Hook_Lifecycle() {
    return s2hook_detail::g_lifecycle.load(std::memory_order_acquire);
}

inline bool S2Hook_MayDispatch() {
    return S2Hook_Lifecycle() == S2HookLifecycle::Running;
}

inline bool S2Hook_AcceptingRegistrations() {
    return S2Hook_Lifecycle() == S2HookLifecycle::Running;
}

inline int S2Hook_ActiveCount() {
    return s2hook_detail::g_active_callbacks.load(std::memory_order_acquire);
}

inline bool S2Hook_NoActiveDispatch() {
    if (s2hook_detail::g_callback_depth > 0) {
        return false;
    }
    return S2Hook_ActiveCount() == 0;
}

inline bool S2Hook_EnterDispatch(const S2HookObserve& obs) {
    return static_cast<bool>(obs) && S2Hook_MayDispatch();
}

// Direct core-dispatch entries (ConCommand trampoline, event listener) that
// are not a checked KHook Observe still hold an active-dispatch count.
class [[nodiscard]] S2HookDispatchGuard {
public:
    S2HookDispatchGuard() {
        // Count the entry before the JS decision (Observe order) so Unload
        // cannot finish in the window after MayDispatch and before fetch_add.
        s2hook_detail::g_active_callbacks.fetch_add(1, std::memory_order_acq_rel);
        if (!S2Hook_MayDispatch()) {
            s2hook_detail::g_active_callbacks.fetch_sub(1, std::memory_order_acq_rel);
            return;
        }
        armed_ = true;
    }
    ~S2HookDispatchGuard() { reset(); }

    S2HookDispatchGuard(S2HookDispatchGuard&& other) noexcept : armed_(other.armed_) {
        other.armed_ = false;
    }
    S2HookDispatchGuard& operator=(S2HookDispatchGuard&& other) noexcept {
        if (this != &other) {
            reset();
            armed_ = other.armed_;
            other.armed_ = false;
        }
        return *this;
    }

    S2HookDispatchGuard(const S2HookDispatchGuard&) = delete;
    S2HookDispatchGuard& operator=(const S2HookDispatchGuard&) = delete;

    explicit operator bool() const noexcept { return armed_; }

    void reset() noexcept {
        if (!armed_) {
            return;
        }
        armed_ = false;
        s2hook_detail::g_active_callbacks.fetch_sub(1, std::memory_order_acq_rel);
    }

private:
    bool armed_ = false;
};

// Wrap core inbound-hook dispatch so Retiring skips JS and Unload sees the hold.
// Continue (0) if the guard refuses or `fn` is null (weak core export missing).
inline int S2Hook_GuardedDispatchHook(int (*fn)(int, void*), int hookId, void* argView) {
    S2HookDispatchGuard guard;
    if (!guard || !fn) {
        return 0;
    }
    return fn(hookId, argView);
}

inline int S2Hook_GuardedDispatchHookPost(int (*fn)(int, void*, int), int hookId, void* argView,
                                         int skipped) {
    S2HookDispatchGuard guard;
    if (!guard || !fn) {
        return 0;
    }
    return fn(hookId, argView, skipped);
}

// true = this thread is not on a hook callback stack (safe to walk the
// retirement queue). It does NOT mean the queue is empty: delayed
// completions leave Removing entries. Unload must require
// DrainRetirement() && RetirementPending() == 0 and must not busy-wait
// those completions on the game thread. false = reject/defer Unload.
// TLS depth alone is not a cross-thread drain: also require a zero global
// active-callback count.
inline bool S2Hook_DrainRetirement() {
    if (s2hook_detail::g_callback_depth > 0) {
        return false;
    }
    if (s2hook_detail::g_active_callbacks.load(std::memory_order_acquire) > 0) {
        return false;
    }
    std::lock_guard<std::mutex> lock(s2hook_detail::g_retire_mu);
    auto it = s2hook_detail::g_retire.begin();
    while (it != s2hook_detail::g_retire.end()) {
        bool gone = false;
        {
            std::lock_guard<std::mutex> gs(it->state->mu);
            auto o = it->state->owned.find(it->id);
            gone = o == it->state->owned.end() || o->second.state == S2HookState::Removed;
        }
        if (gone) {
            it = s2hook_detail::g_retire.erase(it);
        } else {
            ++it;
        }
    }
    return true;
}

inline std::size_t S2Hook_RetirementPending() {
    std::lock_guard<std::mutex> lock(s2hook_detail::g_retire_mu);
    std::size_t n = 0;
    for (const auto& e : s2hook_detail::g_retire) {
        std::lock_guard<std::mutex> gs(e.state->mu);
        auto o = e.state->owned.find(e.id);
        if (o == e.state->owned.end() || o->second.state != S2HookState::Removed) {
            ++n;
        }
    }
    return n;
}

class S2CheckedBindingOps {
public:
    S2HookReceipt Snapshot() const {
        std::lock_guard<std::mutex> lock(state_->mu);
        return {state_->last_id, state_->last_state, state_->reason};
    }

    bool RemovalComplete() const {
        std::lock_guard<std::mutex> lock(state_->mu);
        for (const auto& pair : state_->owned) {
            if (pair.second.id != KHook::INVALID_HOOK && pair.second.state != S2HookState::Removed)
                return false;
        }
        return true;
    }

    bool CanBeginRemove(bool async = true, const S2HookTerminalPermit* permit = nullptr) const {
        const bool terminal = !async && permit && permit->IsValid() &&
            s2hook_detail::g_observe_stack.size() == 1 &&
            s2hook_detail::g_active_callbacks.load(std::memory_order_acquire) == 1 &&
            s2hook_detail::g_observe_stack.back().capsule == permit->Capsule();
        if (!async && !terminal && !S2Hook_NoActiveDispatch()) return false;
        std::lock_guard<std::mutex> lock(state_->mu);
        for (const auto& pair : state_->owned) {
            const auto& rec = pair.second;
            if (rec.state == S2HookState::Removed || rec.id == KHook::INVALID_HOOK) continue;
            if (!async && rec.remove_scheduled) return false;
            if (terminal && rec.capsule == permit->Capsule()) return false;
        }
        return true;
    }

    // Synchronous removal is reserved for a proven off-callback terminal boundary. It must be
    // the first removal request for this binding; an async retirement cannot be upgraded later.
    bool BeginRemove(bool async = true, const S2HookTerminalPermit* permit = nullptr) {
        const bool terminal = !async && permit && permit->IsValid() &&
            s2hook_detail::g_observe_stack.size() == 1 && S2Hook_ActiveCount() == 1 &&
            s2hook_detail::g_observe_stack.back().capsule == permit->Capsule();
        if (!async && !terminal && !S2Hook_NoActiveDispatch()) {
            return false;
        }
        std::vector<KHook::HookID_t> to_remove;
        {
            std::lock_guard<std::mutex> lock(state_->mu);
            if (!async) {
                for (const auto& pair : state_->owned) {
                    const auto& rec = pair.second;
                    if (rec.state == S2HookState::Removed || rec.id == KHook::INVALID_HOOK) continue;
                    if (terminal && rec.capsule == permit->Capsule()) return false;
                    if (rec.remove_scheduled && rec.state != S2HookState::Removed) {
                        return false;
                    }
                }
            }
            for (auto& pair : state_->owned) {
                auto& rec = pair.second;
                if (rec.remove_scheduled || rec.id == KHook::INVALID_HOOK) {
                    continue;
                }
                rec.remove_scheduled = true;
                rec.state = S2HookState::Removing;
                if (state_->last_id == rec.id) {
                    state_->last_state = S2HookState::Removing;
                }
                to_remove.push_back(rec.id);
            }
        }
        for (KHook::HookID_t id : to_remove) {
            if (async) {
                std::lock_guard<std::mutex> lock(s2hook_detail::g_retire_mu);
                s2hook_detail::g_retire.push_back({state_, id});
            }
            ::KHook::RemoveHook(id, async, &s2hook_detail::OnRemoved, state_.get());
        }
        return true;
    }

protected:
    std::shared_ptr<S2HookBindingState> state_ = std::make_shared<S2HookBindingState>();

    S2HookReceipt Fail(const char* why) {
        std::lock_guard<std::mutex> lock(state_->mu);
        if (state_->owned.empty()) {
            state_->last_id = KHook::INVALID_HOOK;
            state_->last_state = S2HookState::Failed;
            state_->reason = why;
        }
        return {KHook::INVALID_HOOK, S2HookState::Failed, std::string(why)};
    }

    S2HookReceipt Accept(KHook::HookID_t id, const void* capsule) {
        std::lock_guard<std::mutex> lock(state_->mu);
        auto it = state_->owned.find(id);
        if (it == state_->owned.end()) {
            S2HookBindingState::Owned rec;
            rec.id = id;
            rec.state = S2HookState::Pending;
            rec.capsule = capsule;
            state_->owned.emplace(id, rec);
            state_->last_id = id;
            state_->last_state = S2HookState::Pending;
            state_->reason.clear();
        } else {
            state_->last_id = id;
            state_->last_state = it->second.state;
            state_->reason.clear();
        }
        return {state_->last_id, state_->last_state, state_->reason};
    }

    [[nodiscard]] S2HookObserve ObserveOwned(KHook::HookID_t id) {
        return S2HookObserve{s2hook_detail::NoteObserve(state_, id)};
    }
};

template <std::size_t N>
inline bool S2HookInventoryCanRemoveSync(
    const std::array<S2CheckedBindingOps*, N>& bindings, const S2HookTerminalPermit& permit) {
    for (const auto* binding : bindings)
        if (!binding || !binding->CanBeginRemove(false, &permit)) return false;
    return true;
}

template <std::size_t N>
inline bool S2HookInventoryBeginRemoveSync(
    const std::array<S2CheckedBindingOps*, N>& bindings, const S2HookTerminalPermit& permit) {
    if (!S2HookInventoryCanRemoveSync(bindings, permit)) return false;
    for (auto* binding : bindings)
        if (!binding || !binding->BeginRemove(false, &permit)) return false;
    for (const auto* binding : bindings)
        if (!binding->RemovalComplete()) return false;
    return true;
}

template <std::size_t N>
inline bool S2HookInventoryRemovalComplete(
    const std::array<S2CheckedBindingOps*, N>& bindings) {
    for (const auto* binding : bindings)
        if (!binding || !binding->RemovalComplete()) return false;
    return true;
}

template <typename Ret, typename... Args>
class S2CheckedFunction : public KHook::Function<Ret, Args...>, public S2CheckedBindingOps {
public:
    using Base = KHook::Function<Ret, Args...>;
    using Base::Function;

    S2HookReceipt Configure(const void* address) {
        if (address == nullptr) {
            return this->Fail("null function address");
        }
        if (!S2Hook_AcceptingRegistrations()) {
            return this->Fail("plugin retiring");
        }
        const void* hooked = this->_hooked_addr;
        {
            std::lock_guard<std::mutex> lock(this->state_->mu);
            for (const auto& pair : this->state_->owned) {
                const auto& rec = pair.second;
                if (rec.id == KHook::INVALID_HOOK) {
                    continue;
                }
                if (rec.state == S2HookState::Failed || rec.state == S2HookState::Removed) {
                    continue;
                }
                if (hooked == address) {
                    this->state_->last_id = rec.id;
                    this->state_->last_state = rec.state;
                    this->state_->reason.clear();
                    return {rec.id, rec.state, std::string{}};
                }
                this->state_->reason = "function already bound to a different address";
                return {KHook::INVALID_HOOK, S2HookState::Failed, this->state_->reason};
            }
        }
        Base::Configure(address);
        KHook::HookID_t id = KHook::INVALID_HOOK;
        {
            std::lock_guard<std::mutex> lock(this->_hooks_stored);
            id = this->_associated_hook_id;
        }
        if (id == KHook::INVALID_HOOK) {
            return this->Fail("SetupHook returned INVALID_HOOK");
        }
        return this->Accept(id, address);
    }

    S2HookReceipt Configure(void* address) {
        return Configure(static_cast<const void*>(address));
    }

    S2HookReceipt Configure(Ret (*function)(Args...)) {
        return Configure(reinterpret_cast<const void*>(function));
    }

    // Keep the returned guard alive for the callback body.
    [[nodiscard]] S2HookObserve Observe() {
        KHook::HookID_t id = KHook::INVALID_HOOK;
        {
            std::lock_guard<std::mutex> lock(this->state_->mu);
            id = this->state_->last_id;
        }
        return this->ObserveOwned(id);
    }
};

template <typename Class, typename Ret, typename... Args>
class S2CheckedVirtual : public KHook::Virtual<Class, Ret, Args...>, public S2CheckedBindingOps {
public:
    using Base = KHook::Virtual<Class, Ret, Args...>;
    using Base::Virtual;
    using Base::Remove;
    using Base::RemoveGlobal;
    using Base::ClearHooks;
    using Base::CallOriginal;

    ~S2CheckedVirtual() override {
        // The pinned Virtual helper erases its address->id entry on physical
        // removal but retains id->address for its destructor. Discard only
        // completed IDs so base destruction cannot re-remove historical hooks.
        // Live IDs retain the base helper's normal cleanup. As with BeginRemove,
        // callers must retain this object until all pending removals complete.
        std::vector<KHook::HookID_t> completed;
        {
            std::lock_guard<std::mutex> lock(this->state_->mu);
            for (const auto& pair : this->state_->owned) {
                if (pair.second.state == S2HookState::Removed) completed.push_back(pair.first);
            }
        }
        std::lock_guard<std::mutex> lock(this->_hooks_stored);
        for (const auto id : completed) {
            const auto old = this->_hook_ids_addr.find(id);
            if (old == this->_hook_ids_addr.end()) continue;
            const auto current = this->_addr_hook_ids.find(old->second);
            if (current != this->_addr_hook_ids.end() && current->second == id) {
                this->_addr_hook_ids.erase(current);
            }
            this->_hook_ids_addr.erase(old);
        }
    }

    void Configure(std::int32_t index) {
        if (index < 0) {
            return;
        }
        Base::Configure(index);
    }

    void Configure(Ret (Class::*function)(Args...)) {
        const std::int32_t index = KHook::GetVtableIndex(function);
        if (index < 0) {
            return;
        }
        Base::Configure(function);
    }

    void Configure(Ret (Class::*function)(Args...) const) {
        const std::int32_t index = KHook::GetVtableIndex(function);
        if (index < 0) {
            return;
        }
        Base::Configure(function);
    }

    S2HookReceipt Add(Class* obj) {
        if (!S2Hook_AcceptingRegistrations()) {
            return this->Fail("plugin retiring");
        }
        if (obj == nullptr) {
            return this->Fail("null object");
        }
        if (this->_vtbl_index < 0) {
            return this->Fail("invalid vtable slot");
        }
        bool already = false;
        {
            std::lock_guard<std::mutex> lock(this->_m_hooked_this);
            already = this->_hooked_this.find(obj) != this->_hooked_this.end();
        }
        Base::Add(obj);
        const KHook::HookID_t id = LookupId(obj);
        if (id == KHook::INVALID_HOOK) {
            if (!already) {
                Base::Remove(obj);
            }
            return this->Fail("SetupVirtualHook returned INVALID_HOOK");
        }
        void** vtable = *reinterpret_cast<void***>(obj);
        return this->Accept(id, vtable + this->_vtbl_index);
    }

    S2HookReceipt AddGlobal(Class* holder) {
        if (!S2Hook_AcceptingRegistrations()) {
            return this->Fail("plugin retiring");
        }
        if (holder == nullptr) {
            return this->Fail("null object");
        }
        if (this->_vtbl_index < 0) {
            return this->Fail("invalid vtable slot");
        }
        void** vt = *(void***)holder;
        if (vt == nullptr) {
            return this->Fail("null vtable");
        }
        bool already = false;
        {
            std::lock_guard<std::mutex> lock(this->_m_hooked_this);
            already = this->_hooked_global.find(vt) != this->_hooked_global.end();
        }
        Base::AddGlobal(holder);
        const KHook::HookID_t id = LookupId(holder);
        if (id == KHook::INVALID_HOOK) {
            if (!already) {
                Base::RemoveGlobal(holder);
            }
            return this->Fail("SetupVirtualHook returned INVALID_HOOK");
        }
        return this->Accept(id, vt + this->_vtbl_index);
    }

    bool HasThisFilter(Class* obj) {
        if (obj == nullptr) {
            return false;
        }
        std::lock_guard<std::mutex> lock(this->_m_hooked_this);
        return this->_hooked_this.find(obj) != this->_hooked_this.end();
    }

    bool HasGlobalFilter(Class* holder) {
        if (holder == nullptr) {
            return false;
        }
        void** vt = *(void***)holder;
        std::lock_guard<std::mutex> lock(this->_m_hooked_this);
        return this->_hooked_global.find(vt) != this->_hooked_global.end();
    }

    // Keep the returned guard alive for the callback body. Unmatched this
    // pointers return an empty guard and stay Pending.
    [[nodiscard]] S2HookObserve Observe(Class* self) {
        if (self == nullptr) {
            return S2HookObserve{};
        }
        bool match = false;
        {
            std::lock_guard<std::mutex> lock(this->_m_hooked_this);
            if (this->_hooked_this.find(self) != this->_hooked_this.end()) {
                match = true;
            } else {
                void** vt = *(void***)self;
                match = this->_hooked_global.find(vt) != this->_hooked_global.end();
            }
        }
        if (!match) {
            return S2HookObserve{};
        }
        return this->ObserveOwned(LookupId(self));
    }

private:
    KHook::HookID_t LookupId(Class* obj) {
        if (obj == nullptr) {
            return KHook::INVALID_HOOK;
        }
        void** vtable = *(void***)obj;
        if (vtable == nullptr) {
            return KHook::INVALID_HOOK;
        }
        std::lock_guard<std::mutex> lock(this->_hooks_stored);
        auto key = vtable + this->_vtbl_index;
        auto it = this->_addr_hook_ids.find(key);
        if (it == this->_addr_hook_ids.end()) {
            return KHook::INVALID_HOOK;
        }
        return it->second;
    }
};
