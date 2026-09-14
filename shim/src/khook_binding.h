#pragma once

// Checked typed bindings over the pinned KHook Function/Virtual helpers.
// Pin-specific: inherit their constructors and inspect protected associated-id /
// slot-to-id maps under those helpers' existing locks. Not an alternate detour
// engine and not a public plugin API.
//
// Public FFI conventions (unchanged): SDKHooks VP add uses nonzero success;
// declarative S2_HookInstall uses 0 success / -1 failure.

#include <khook.hpp>

#include <cstddef>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

enum class S2HookState { Failed, Pending, Active, Removing, Removed };

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
};

inline thread_local int g_callback_depth = 0;
inline thread_local std::vector<ObserveFrame> g_observe_stack;

struct RetirementEntry {
    std::shared_ptr<S2HookBindingState> state;
    KHook::HookID_t id = KHook::INVALID_HOOK;
};

inline std::mutex g_retire_mu;
inline std::vector<RetirementEntry> g_retire;

inline void NoteObserve(const std::shared_ptr<S2HookBindingState>& state, KHook::HookID_t id) {
    if (!state || id == KHook::INVALID_HOOK) {
        return;
    }
    {
        std::lock_guard<std::mutex> lock(state->mu);
        auto it = state->owned.find(id);
        if (it == state->owned.end()) {
            return;
        }
        if (it->second.state == S2HookState::Failed || it->second.state == S2HookState::Removed) {
            return;
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
    g_observe_stack.push_back({state.get(), id});
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

inline bool S2Hook_DrainRetirement() {
    if (s2hook_detail::g_callback_depth > 0) {
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

    void EndObserve() {
        // Pairs with Observe: drop the invocation hold and TLS callback-stack
        // depth. Do not wait here; DrainRetirement runs outside the callback.
        if (s2hook_detail::g_observe_stack.empty()) {
            return;
        }
        s2hook_detail::ObserveFrame frame = s2hook_detail::g_observe_stack.back();
        s2hook_detail::g_observe_stack.pop_back();
        if (s2hook_detail::g_callback_depth > 0) {
            s2hook_detail::g_callback_depth--;
        }
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

    void BeginRemove() {
        std::vector<KHook::HookID_t> to_remove;
        {
            std::lock_guard<std::mutex> lock(state_->mu);
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
            {
                std::lock_guard<std::mutex> lock(s2hook_detail::g_retire_mu);
                s2hook_detail::g_retire.push_back({state_, id});
            }
            ::KHook::RemoveHook(id, true, &s2hook_detail::OnRemoved, state_.get());
        }
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

    S2HookReceipt Accept(KHook::HookID_t id) {
        std::lock_guard<std::mutex> lock(state_->mu);
        auto it = state_->owned.find(id);
        if (it == state_->owned.end()) {
            S2HookBindingState::Owned rec;
            rec.id = id;
            rec.state = S2HookState::Pending;
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

    void ObserveOwned(KHook::HookID_t id) { s2hook_detail::NoteObserve(state_, id); }
};

template <typename Ret, typename... Args>
class S2CheckedFunction : public KHook::Function<Ret, Args...>, public S2CheckedBindingOps {
public:
    using Base = KHook::Function<Ret, Args...>;
    using Base::Function;

    S2HookReceipt Configure(const void* address) {
        if (address == nullptr) {
            return this->Fail("null function address");
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
        return this->Accept(id);
    }

    S2HookReceipt Configure(void* address) {
        return Configure(static_cast<const void*>(address));
    }

    S2HookReceipt Configure(Ret (*function)(Args...)) {
        return Configure(reinterpret_cast<const void*>(function));
    }

    void Observe() {
        KHook::HookID_t id = KHook::INVALID_HOOK;
        {
            std::lock_guard<std::mutex> lock(this->state_->mu);
            id = this->state_->last_id;
        }
        this->ObserveOwned(id);
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
        return this->Accept(id);
    }

    S2HookReceipt AddGlobal(Class* holder) {
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
        return this->Accept(id);
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

    void Observe(Class* self) {
        if (self == nullptr) {
            return;
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
            return;
        }
        this->ObserveOwned(LookupId(self));
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
