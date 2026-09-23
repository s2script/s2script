#pragma once
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>
#include <utility>

namespace s2fn {
template<class T> struct Result {
    T value{};
    std::string error;
    explicit operator bool() const { return error.empty(); }
};
struct AbiAtom { std::string native; std::string projection = {}; };
struct AbiSignature {
    std::string platform = "linux-x86_64-sysv";
    std::string receiver = "none";
    AbiAtom returns{"void", ""};
    std::vector<AbiAtom> parameters;
    bool varargs = false;
};
using AbiFingerprint = std::string;
struct AbiInfo { AbiFingerprint fingerprint; std::size_t stack_bytes = 0; };
Result<AbiInfo> Validate(const AbiSignature&);
Result<std::size_t> StackCopyBytes(std::size_t spilled_slots);
// Internal native transport only. Pointer values never cross into JavaScript.
struct alignas(16) NativeValue {
    std::array<std::uint8_t, 16> bytes{};
    template<class T> static NativeValue From(T value) {
        static_assert(sizeof(T) <= 8); NativeValue result;
        std::memcpy(result.bytes.data(), &value, sizeof(T)); return result;
    }
    template<class T> T Get() const {
        static_assert(sizeof(T) <= 8); T value{};
        std::memcpy(&value, bytes.data(), sizeof(T)); return value;
    }
};
}
#ifndef S2FN_VALIDATION_ONLY
#include "khook_binding.h"
#include <ffi.h>
namespace s2fn {
enum class Phase { Pre, Post, MakeReturn, MakeOriginal };
struct DispatchFrame {
    Phase phase;
    NativeValue receiver;
    std::vector<NativeValue> arguments;
    NativeValue result;
    KHook::Action action = KHook::Action::Ignore;
    bool changed = false;
    bool original_skipped = false;
};
class DispatchSink {
public:
    virtual ~DispatchSink() = default;
    virtual void Dispatch(DispatchFrame&) = 0;
    virtual void Error(const char*) noexcept = 0;
};
class RuntimeBinding final : private S2CheckedBindingOps {
public:
    static Result<std::unique_ptr<RuntimeBinding>> Create(AbiSignature, DispatchSink&);
    ~RuntimeBinding();
    RuntimeBinding(const RuntimeBinding&) = delete;
    RuntimeBinding& operator=(const RuntimeBinding&) = delete;
    Result<NativeValue> Call(const NativeValue* args, std::size_t argc);
    S2HookReceipt Configure(const void* address);
    void BeginRemove();
    bool RemovalComplete() const;
    const AbiInfo& Info() const { return info_; }
private:
    struct Closure {
        ffi_closure* allocation = nullptr;
        void* code = nullptr;
        RuntimeBinding* binding = nullptr;
        Phase phase = Phase::Pre;
        ~Closure();
    };
    RuntimeBinding(AbiSignature s, AbiInfo info, DispatchSink& sink);
    static void ClosureEntry(ffi_cif*, void*, void**, void*) noexcept;
    static void OnKHookRemoved(KHook::HookID_t);
    void Enter(Phase, void*, void**);
    Result<NativeValue> Invoke(void*, const NativeValue*, std::size_t);
    void Save(KHook::Action, NativeValue&, bool);
    void WriteResult(void*, const NativeValue&);
    AbiSignature signature_;
    AbiInfo info_;
    DispatchSink& sink_;
    ffi_cif cif_{};
    std::vector<ffi_type*> argument_types_;
    std::vector<std::string> argument_atoms_;
    Closure pre_, post_, make_return_, make_original_;
    KHook::HookID_t hook_id_ = KHook::INVALID_HOOK;
    const void* target_ = nullptr;
    std::atomic<bool> provider_detached_{true};
};
}
#endif
