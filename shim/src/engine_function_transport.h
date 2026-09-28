#pragma once
// Private host transport. These versioned spans/identities are never JS inputs
// and confer no callback/adapter authority. Existing public PODs stay frozen.
#include "engine_function_copy.h"
#include "../include/s2script_core.h"
namespace s2bridge {
enum class CopyOwnership { None, CalleeBorrowed, CalleeRetained, CallerBorrowed, NativeObserved };
struct CopyPosition {
    s2fn::copy::Kind kind{};
    CopyOwnership ownership = CopyOwnership::None;
    bool mutable_pre = false;
    // Trusted-only `string-indirect`: the native argument points at an object whose
    // first pointer-sized word is the char*. Always NativeObserved (read-only).
    bool indirect = false;
    explicit operator bool() const { return kind != s2fn::copy::Kind{}; }
};
struct CopyInput {
    uint32_t version = 1, struct_size = sizeof(CopyInput);
    const uint8_t* data = nullptr;
    uint64_t size = 0;
};
struct CopyOutput {
    uint32_t version = 1, struct_size = sizeof(CopyOutput);
    uint8_t* data = nullptr;
    uint64_t capacity = 0, size = 0;
};
struct CopyProducer {
    uint32_t version = 1, struct_size = sizeof(CopyProducer);
    uint32_t domain = 0, reserved = 0;
    std::array<uint8_t,32> digest{};
    uint64_t generation = 0;
};
static_assert(sizeof(CopyInput)==24 && offsetof(CopyInput,size)==16, "copy input v1 layout");
static_assert(sizeof(CopyOutput)==32 && offsetof(CopyOutput,size)==24, "copy output v1 layout");
static_assert(sizeof(CopyProducer)==56 && offsetof(CopyProducer,generation)==48, "copy producer v1 layout");
static_assert(sizeof(S2FunctionCopyInput)==sizeof(CopyInput) && offsetof(S2FunctionCopyInput,size)==16, "public copy input layout");
static_assert(sizeof(S2FunctionCopyOutput)==sizeof(CopyOutput) && offsetof(S2FunctionCopyOutput,size)==24, "public copy output layout");
static_assert(sizeof(S2FunctionCopyProducer)==sizeof(CopyProducer) && offsetof(S2FunctionCopyProducer,generation)==48, "public copy producer layout");
s2fn::Result<s2fn::copy::OwnerGeneration> CheckedProducer(const CopyProducer&);
s2fn::Result<s2fn::copy::Snapshot> DecodeCopy(const s2fn::copy::Operation&, s2fn::copy::Kind,
    const S2FunctionValue&, const CopyInput&);
s2fn::Result<bool> AdmitCopyOutput(s2fn::copy::Kind, const S2FunctionValue&, const CopyOutput&, size_t);
s2fn::Result<S2FunctionValue> EncodeCopy(const s2fn::copy::Snapshot&, const S2FunctionValue&, CopyOutput&);
// One bounded transaction on the actual frame/call. Staging replaces candidates
// without permanent publication. Publish receives only the host-folded winners.
class CopyTransaction final : public s2fn::FrameRetention {
public:
    static constexpr size_t Return = 32, Original = 33;
    static s2fn::Result<s2fn::RetainedFrame> Create(const CopyProducer& engine);
    void Release() noexcept override;
    s2fn::Result<bool> Capture(size_t, s2fn::copy::Kind, uintptr_t, const s2fn::copy::Reader&);
    s2fn::Result<bool> CaptureIndirect(size_t, uintptr_t, const s2fn::copy::Reader&);
    s2fn::Result<bool> Stage(size_t, s2fn::copy::Kind, const S2FunctionValue&, const CopyInput&, const CopyProducer&);
    s2fn::Result<std::array<const void*,33>> Publish(const std::array<CopyPosition,32>&, size_t, bool return_wins);
    const s2fn::copy::Snapshot& Read(size_t) const;
    s2fn::copy::Operation& EngineOperation() { return engine_; }
    void AcceptReturn(const s2fn::copy::Snapshot&) noexcept;
private:
    s2fn::copy::Lease charge_;
    s2fn::copy::Operation engine_;
    std::array<s2fn::copy::Snapshot,34> observed_{};
    std::array<s2fn::copy::Snapshot,33> edits_{};
    std::array<s2fn::copy::StableOwner,33> producers_{};
};
}
