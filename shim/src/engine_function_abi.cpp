#include "engine_function_abi.h"
#include <algorithm>
#include <stdexcept>
namespace s2fn {
#include "engine_function_abi.generated.inc"
static std::size_t Width(const std::string& atom) {
    for (auto item : capabilities::atoms) if (atom == item.name) return item.width;
    return 0;
}
Result<std::size_t> StackCopyBytes(std::size_t slots) {
    if (slots > capabilities::maxStackBytes / 8) return {0, "stack-copy byte overflow"};
    return {std::max(capabilities::stackSafetyBuffer, (slots * 8 + 15) & ~std::size_t(15)), {}};
}
Result<AbiInfo> Validate(const AbiSignature& s) {
    auto fail = [](std::string error) -> Result<AbiInfo> { return {{}, std::move(error)}; };
    if (s.platform != capabilities::Platform) return fail("unsupported platform: " + s.platform);
    if (s.varargs) return fail("unsupported varargs");
    auto check = [](const AbiAtom& a, const std::string& position, bool ret) {
        if (!Width(a.native) && !(ret && a.native == "void")) return "unsupported " + position + ": " + a.native;
        if (a.native == "u8" && a.projection != "bool") return "unsupported u8 projection: " + a.projection;
        return std::string{};
    };
    auto err = check(s.returns, "return", true); if (!err.empty()) return fail(err);
    if (s.parameters.size() > capabilities::maxParameters) return fail("unsupported parameter count");
    std::size_t gp = s.member_receiver ? 1 : 0, sse = 0, spills = 0;
    std::string fingerprint = s.platform + ":" + (s.member_receiver ? "entity" : "none") + ":" + s.returns.native + "(";
    for (std::size_t i = 0; i < s.parameters.size(); ++i) {
        const auto& a = s.parameters[i];
        err = check(a, "parameter[" + std::to_string(i) + "]", false); if (!err.empty()) return fail(err);
        bool floating = a.native == "f32" || a.native == "f64";
        if (floating ? sse++ >= capabilities::sseRegisters : gp++ >= capabilities::gpRegisters) ++spills;
        if (i) fingerprint += ',';
        fingerprint += a.native;
    }
    auto bytes = StackCopyBytes(spills); if (!bytes) return fail(bytes.error);
    return {{fingerprint + ")", bytes.value}, {}};
}
#ifndef S2FN_VALIDATION_ONLY
// Call-local error propagation, including synchronous nested target entry. The
// sink also receives errors for engine-originated calls with no Call frame.
static thread_local std::vector<std::pair<RuntimeBinding*, std::string*>> call_errors;
static void NoteCallError(RuntimeBinding* binding, const char* error) {
    for (auto i = call_errors.rbegin(); i != call_errors.rend(); ++i) {
        if (i->first == binding) { if (i->second->empty()) *i->second = error; break; }
    }
}
// Every physical PRE, including neutral/bypassed entries, owns one pairing row.
// Stock KHook walks PRE forward and POST backward exactly once; recall resumes
// after the current callback and must not mint another row for its continuation.
struct InvocationRow { RuntimeBinding* binding=nullptr; std::uint64_t id=0; };
// Allocation-free bookkeeping precedes even checked Observe allocation. A
// neutral sentinel protects the enclosing invocation when PRE fails before it
// can mint an id. Excess nesting is refused without ever consuming an outer row.
static thread_local std::array<InvocationRow,1024> invocation_rows;
static thread_local std::size_t invocation_depth=0, invocation_overflow=0;
struct InvocationPhase {
    RuntimeBinding* binding;
    Phase phase;
    bool paired=false, overflow=false;
    std::size_t index=0;
    InvocationPhase(RuntimeBinding* b,Phase p) noexcept:binding(b),phase(p) {
        if(p==Phase::Pre){
            if(invocation_depth==invocation_rows.size() || invocation_overflow){++invocation_overflow;overflow=true;}
            else{index=invocation_depth++;invocation_rows[index]={b,0};paired=true;}
        }else if(p==Phase::Post){
            if(invocation_overflow){overflow=true;}
            else if(invocation_depth && invocation_rows[invocation_depth-1].binding==b){index=invocation_depth-1;paired=true;}
        }
    }
    ~InvocationPhase(){if(phase==Phase::Post){if(overflow)--invocation_overflow;else if(paired)--invocation_depth;}}
    std::uint64_t id()const{return paired ? invocation_rows[index].id : 0;}
};
static std::atomic<std::uint64_t> next_invocation{1};
static std::uint64_t NewInvocation() {
    auto id = next_invocation.load(std::memory_order_relaxed);
    do { if (id == UINT64_MAX) throw std::runtime_error("invocation id exhausted"); }
    while (!next_invocation.compare_exchange_weak(id, id + 1, std::memory_order_relaxed));
    return id;
}
static ffi_type* Type(const std::string& atom) {
    if (atom == "void") return &ffi_type_void;
    if (atom == "u8") return &ffi_type_uint8;
    if (atom == "i32") return &ffi_type_sint32;
    if (atom == "u32") return &ffi_type_uint32;
    if (atom == "i64") return &ffi_type_sint64;
    if (atom == "u64") return &ffi_type_uint64;
    if (atom == "f32") return &ffi_type_float;
    if (atom == "f64") return &ffi_type_double;
    if (atom == "ptr") return &ffi_type_pointer;
    return nullptr;
}
static bool Canonical(const std::string& atom, const NativeValue& v) {
    return atom != "u8" || v.Get<std::uint8_t>() <= 1;
}
static void Copy1(void* a, void* b) { std::memcpy(a, b, 1); }
static void Copy4(void* a, void* b) { std::memcpy(a, b, 4); }
static void Copy8(void* a, void* b) { std::memcpy(a, b, 8); }
static void DestroyScalar(void*) {}
static void* CopyOp(std::size_t width) {
    return width == 1 ? reinterpret_cast<void*>(&Copy1) : width == 4 ? reinterpret_cast<void*>(&Copy4) : reinterpret_cast<void*>(&Copy8);
}
RuntimeBinding::Closure::~Closure() { if (allocation) ffi_closure_free(allocation); }
RuntimeBinding::RuntimeBinding(AbiSignature s, AbiInfo info, DispatchSink& sink)
    : signature_(std::move(s)), info_(std::move(info)), sink_(sink) {}
RuntimeBinding::~RuntimeBinding() {
    // A caller destroying before both acknowledgements is a lifetime violation.
    // Refuse before member destructors can free executable closure storage.
    if (!RemovalComplete()) std::terminate();
}
Result<std::unique_ptr<RuntimeBinding>> RuntimeBinding::Create(AbiSignature s, DispatchSink& sink) {
    auto info = Validate(s); if (!info) return {{}, info.error};
#if !defined(__linux__) || !defined(__x86_64__)
    return {{}, "unsupported runtime platform: requires linux-x86_64-sysv"};
#else
    auto b = std::unique_ptr<RuntimeBinding>(new RuntimeBinding(std::move(s), std::move(info.value), sink));
    if (b->signature_.member_receiver) b->argument_atoms_.push_back("ptr");
    for (const auto& a : b->signature_.parameters) b->argument_atoms_.push_back(a.native);
    for (const auto& a : b->argument_atoms_) b->argument_types_.push_back(Type(a));
    if (b->argument_types_.size() > capabilities::maxCifArguments) return {{}, "CIF argument overflow"};
    if (ffi_prep_cif(&b->cif_, FFI_UNIX64, b->argument_types_.size(), Type(b->signature_.returns.native), b->argument_types_.data()) != FFI_OK)
        return {{}, "ffi_prep_cif failed"};
    std::array<Closure*, 4> closures{&b->pre_, &b->post_, &b->make_return_, &b->make_original_};
    for (std::size_t i = 0; i < closures.size(); ++i) {
        auto& c = *closures[i]; c.binding = b.get(); c.phase = static_cast<Phase>(i);
        c.allocation = static_cast<ffi_closure*>(ffi_closure_alloc(sizeof(ffi_closure), &c.code));
        if (!c.allocation) return {{}, "ffi_closure_alloc phase " + std::to_string(i)};
        if (ffi_prep_closure_loc(c.allocation, &b->cif_, &ClosureEntry, &c, c.code) != FFI_OK)
            return {{}, "ffi_prep_closure_loc phase " + std::to_string(i)};
    }
    return {std::move(b), {}};
#endif
}
std::string RuntimeBinding::BindTarget(const void* address) {
    if (!address) return "null function address";
    if (target_ && target_ != address) return "immutable target cannot be retargeted";
    target_ = address;
    return {};
}
S2HookReceipt RuntimeBinding::Configure(const void* address) {
    std::lock_guard<std::mutex> admission(admission_mu_);
    if (active_entries_.load(std::memory_order_acquire)) return Fail("Configure requires idle binding");
    const auto error = BindTarget(address);
    if (!error.empty()) return Fail(error.c_str());
    if (!S2Hook_AcceptingRegistrations()) return Fail("plugin retiring");
    // An unrelated checked outer GameFrame is normal host execution. A new
    // capsule still patches synchronously: callers must establish target-page
    // quiescence; async insertion only covers an already existing capsule.
    // This binding's own runtime entries were rejected above. An unrelated
    // subscriber on an existing capsule may be active: stock queues insertion
    // without waiting for that capsule's dispatch lock.
    if (hook_id_ != KHook::INVALID_HOOK) {
        if (!RemovalComplete()) return Fail("binding already configured or removal incomplete");
        // A fresh checked receipt cannot inherit stale id/observation state. The
        // retirement queue retains the old shared state until its next drain.
        if (!PruneCompletedTicket()) return Fail("completed bridge ticket not collectable");
        state_ = std::make_shared<S2HookBindingState>();
        hook_id_ = KHook::INVALID_HOOK;
    }
    detached_callable_ = false;
    provider_detached_.store(false, std::memory_order_release);
    hook_id_ = KHook::SetupHook(const_cast<void*>(address), this, reinterpret_cast<void*>(&OnKHookRemoved),
        pre_.code, post_.code, make_return_.code, make_original_.code, info_.stack_bytes, !S2Hook_NoActiveDispatch());
    if (hook_id_ == KHook::INVALID_HOOK) {
        provider_detached_.store(true, std::memory_order_release); return Fail("SetupHook returned INVALID_HOOK");
    }
    target_ = address;
    // Receipt remains Pending until Observe. Synchronous installation does not
    // mislabel an unobserved native callback as Active.
    return Accept(hook_id_, address);
}
void RuntimeBinding::BeginRemove() { S2CheckedBindingOps::BeginRemove(true); }
bool RuntimeBinding::RemovalComplete() const {
    return provider_detached_.load(std::memory_order_acquire) && S2CheckedBindingOps::RemovalComplete() &&
        active_entries_.load(std::memory_order_acquire) == 0;
}
bool RuntimeBinding::PruneCompletedTicket() {
    return RemovalComplete() && S2Hook_PruneCompletedBridgeTicket(state_, hook_id_);
}
void RuntimeBinding::OnKHookRemoved(KHook::HookID_t) {
    KHook::GetContext<RuntimeBinding>()->provider_detached_.store(true, std::memory_order_release);
}
Result<NativeValue> RuntimeBinding::Invoke(void* address, const NativeValue* args, std::size_t argc) {
    if (argc != argument_atoms_.size() || (argc && !args)) return {{}, "argument count mismatch"};
    std::array<void*,capabilities::maxCifArguments> pointers{};
    for (std::size_t i = 0; i < argc; ++i) {
        if (!Canonical(argument_atoms_[i], args[i])) return {{}, "noncanonical u8 argument[" + std::to_string(i) + "]"};
        pointers[i]=const_cast<std::uint8_t*>(args[i].bytes.data());
    }
    // ffi_call widens narrow integral returns to ffi_arg. Never hand it a byte.
    alignas(16) std::array<std::uint8_t, 16> storage{};
    static_assert(sizeof(ffi_arg) <= 16);
    ffi_call(&cif_, FFI_FN(address), storage.data(), pointers.data());
#ifdef S2FN_TESTING
    extern void TestInvokeReturned(RuntimeBinding*);
    TestInvokeReturned(this);
#endif
    NativeValue result;
    if (signature_.returns.native == "u8") {
        ffi_arg wide{}; std::memcpy(&wide, storage.data(), sizeof(wide));
        if (wide > 1) return {{}, "noncanonical u8 return"};
        result = NativeValue::From(static_cast<std::uint8_t>(wide));
    } else std::memcpy(result.bytes.data(), storage.data(), Width(signature_.returns.native));
    return {result, {}};
}
Result<NativeValue> RuntimeBinding::Call(const NativeValue* args, std::size_t argc, PrepareCall prepare, void* context) {
    std::unique_lock<std::mutex> admission(admission_mu_);
    Activity activity(*this); // retained through ffi_call and result/error handling
    // On refusal or an exception, unlock before releasing the activity hold:
    // the final activity release may allow another thread to reclaim this owner.
    struct UnlockBeforeActivity {
        std::unique_lock<std::mutex>& lock;
        ~UnlockBeforeActivity() { if (lock.owns_lock()) lock.unlock(); }
    } unlock_before_activity{admission};
    const auto state = Snapshot().state;
    if (!target_ || state == S2HookState::Removing) return {{}, "binding not callable"};
    if (state == S2HookState::Removed && !detached_callable_) {
        // Before opening detached admission, every retirement-era entry must be
        // gone. The mutex prevents competing new callers adding their Activity
        // before this decision; this Call owns the single remaining entry.
        if (!provider_detached_.load(std::memory_order_acquire) ||
            active_entries_.load(std::memory_order_acquire) != 1)
            return {{}, "binding not callable"};
        detached_callable_ = true;
    }
    // Completion stays latched for this registration. Subsequent nested or
    // concurrent unhooked calls must not be mistaken for retirement-era work.
    admission.unlock();
    std::string callback_error;
    if(argc!=argument_atoms_.size() || (argc && !args)) return {{},"argument count mismatch"};
    for(size_t i=0;i<argc;++i) if(!Canonical(argument_atoms_[i],args[i])) return {{},"noncanonical call argument"};
    call_errors.emplace_back(this, &callback_error);
    struct Pop { ~Pop() { call_errors.pop_back(); } } pop;
    if(prepare) {auto admitted=prepare(context);if(!admitted) return {{},admitted.error};}
    auto result = Invoke(const_cast<void*>(target_), args, argc);
    if (!callback_error.empty()) return {{}, callback_error};
    return result;
}
void RuntimeBinding::Save(KHook::Action action, NativeValue& value, bool original) {
    const auto width = Width(signature_.returns.native);
    const bool typed = width && (original || action != KHook::Action::Ignore);
    KHook::SaveReturnValue(action, typed ? value.bytes.data() : nullptr, typed ? width : 0,
        typed ? CopyOp(width) : nullptr, typed ? reinterpret_cast<void*>(&DestroyScalar) : nullptr, original);
}
void RuntimeBinding::OverridePostReturn(DispatchFrame& frame, const NativeValue& value) {
    const auto width = Width(signature_.returns.native);
    if (frame.phase != Phase::Post || !width) throw std::runtime_error("nonvoid POST return required");
    if (!Canonical(signature_.returns.native, value)) throw std::runtime_error("noncanonical POST override");
    auto submitted = value;
    Save(KHook::Action::Override, submitted, false);
    // Submission is irreversible. Refresh even if subsequent validation fails.
    const auto current = KHook::GetCurrentValuePtr(false);
    if (!current) throw std::runtime_error("POST override submitted: current return unavailable");
    std::memcpy(frame.result.bytes.data(), current, width);
    if (!Canonical(signature_.returns.native, frame.result))
        throw std::runtime_error("POST override submitted: noncanonical current return");
}
void RuntimeBinding::WriteResult(void* result, const NativeValue& value) {
    if (!Width(signature_.returns.native)) return;
    if (signature_.returns.native == "u8") {
        ffi_arg wide = value.Get<std::uint8_t>(); std::memcpy(result, &wide, sizeof(wide));
    } else if (signature_.returns.native == "i32") {
        ffi_sarg wide = value.Get<std::int32_t>(); std::memcpy(result, &wide, sizeof(wide));
    } else if (signature_.returns.native == "u32") {
        ffi_arg wide = value.Get<std::uint32_t>(); std::memcpy(result, &wide, sizeof(wide));
    } else std::memcpy(result, value.bytes.data(), Width(signature_.returns.native));
}
void RuntimeBinding::ClosureEntry(ffi_cif*, void* result, void** args, void* phase) noexcept {
    auto& c = *static_cast<Closure*>(phase);
    auto* binding = c.binding;
    Activity activity(*binding); // last destructor: no binding access after its release
    InvocationPhase pairing(binding,c.phase);
    try {
        if(c.phase==Phase::Pre){
            if(!pairing.paired)throw std::runtime_error("function invocation nesting exhausted");
#ifdef S2FN_TESTING
            extern void TestBeforeInvocationId(RuntimeBinding*);
            TestBeforeInvocationId(binding);
#endif
            invocation_rows[pairing.index].id=NewInvocation();
        }
        if(c.phase==Phase::Post && !pairing.paired && !pairing.overflow)
            throw std::runtime_error("unmatched function POST invocation");
        auto observe = binding->ObserveOwned(binding->hook_id_);
        binding->WriteResult(result, {});
        binding->Enter(c.phase, result, args, observe, pairing.id());
    }
    catch (const std::exception& e) { NoteCallError(binding, e.what()); binding->sink_.Error(e.what()); }
    catch (...) { NoteCallError(binding, "unknown native closure exception"); binding->sink_.Error("unknown native closure exception"); }
    // Pin-specific reclamation boundary (libffi 5c1c4309, UNIX64 only):
    // ffi64.c ffi_closure_unix64_inner reads all CIF/type data and caches flags
    // BEFORE invoking us; after us it returns only that local flags value.
    // unix64.S then reads the caller-stack result via shared static return code.
    // Both allocated and static trampolines JUMP into that shared entry; neither
    // the trampoline, phase, nor CIF is read/executed again after this callback.
    // Thus the final activity release may retire per-binding storage, while the
    // resident libffi/adapter DSO code and caller-owned result stack remain live.
    // A different libffi pin/platform requires re-auditing this exact boundary.
}
void RuntimeBinding::Enter(Phase phase, void* result, void** args, const S2HookObserve& observe, std::uint64_t invocation) {
    // MakeReturn executes after the provider popped its context stack. Its phase
    // tag is retained with the closure, so it must not use GetContext here.
    if (phase == Phase::MakeReturn) {
        NativeValue effective;
        const auto width = Width(signature_.returns.native);
        if (width) {
            const auto ptr = KHook::GetCurrentValuePtr(true);
            if (ptr) std::memcpy(effective.bytes.data(), ptr, width);
        }
        KHook::DestroyReturnValue(); // exactly once, including void and invalid bool
#ifdef S2FN_TESTING
        extern void TestReturnUnlocked(RuntimeBinding*);
        TestReturnUnlocked(this);
#endif
        if (!Canonical(signature_.returns.native, effective)) throw std::runtime_error("noncanonical u8 effective return");
        WriteResult(result, effective); return;
    }
    std::vector<NativeValue> values(argument_atoms_.size());
    for (std::size_t i = 0; i < values.size(); ++i) {
        std::memcpy(values[i].bytes.data(), args[i], Width(argument_atoms_[i]));
        if (!Canonical(argument_atoms_[i], values[i])) throw std::runtime_error("noncanonical u8 closure argument");
    }
    if (phase == Phase::MakeOriginal) {
        auto native = Invoke(KHook::GetOriginalFunction(), values.data(), values.size());
        if (!native) throw std::runtime_error(native.error);
        Save(KHook::Action::Ignore, native.value, true); WriteResult(result, native.value); return;
    }
    DispatchFrame frame{}; frame.phase = phase; frame.invocation_id = invocation;
    const auto offset = signature_.member_receiver ? 1 : 0;
    if (offset) frame.receiver = values[0];
    frame.arguments.assign(values.begin() + offset, values.end());
    if (phase == Phase::Post) {
        frame.original_skipped = KHook::WasOriginalFunctionSkipped();
        if (!frame.original_skipped && Width(signature_.returns.native)) {
            const auto original = KHook::GetOriginalValuePtr();
            if (!original) throw std::runtime_error("original return unavailable");
            std::memcpy(frame.original_result.bytes.data(), original, Width(signature_.returns.native));
        }
        auto ptr = KHook::GetCurrentValuePtr();
        if (ptr) std::memcpy(frame.result.bytes.data(), ptr, Width(signature_.returns.native));
        if (!Canonical(signature_.returns.native, frame.result)) throw std::runtime_error("noncanonical u8 POST result");
    }
    if (invocation && S2Hook_EnterDispatch(observe)) sink_.Dispatch(frame);
    if (phase == Phase::Post) { Save(KHook::Action::Ignore, frame.result, false); return; }
    if (!Canonical(signature_.returns.native, frame.result)) throw std::runtime_error("noncanonical u8 decision");
    if (frame.changed) {
        if (frame.arguments.size() != signature_.parameters.size()) throw std::runtime_error("edited argument count mismatch");
        std::copy(frame.arguments.begin(), frame.arguments.end(), values.begin() + offset);
        for (std::size_t i = 0; i < values.size(); ++i)
            if (!Canonical(argument_atoms_[i], values[i])) throw std::runtime_error("noncanonical u8 edited argument");
        const auto width = Width(signature_.returns.native);
        const bool typed = width && frame.action != KHook::Action::Ignore;
        auto continuation = KHook::DoRecall(frame.action, typed ? frame.result.bytes.data() : nullptr,
            typed ? width : 0, typed ? CopyOp(width) : nullptr, typed ? reinterpret_cast<void*>(&DestroyScalar) : nullptr);
        auto recalled = Invoke(continuation, values.data(), values.size());
        if (!recalled) throw std::runtime_error(recalled.error);
        WriteResult(result, recalled.value); return; // guard covers this ONE continuation, no second Save
    }
    Save(frame.action, frame.result, false);
}
#endif
} // namespace s2fn
