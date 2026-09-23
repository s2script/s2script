#pragma once
#include "engine_function_abi.h"
#include "engine_resolver.h"

namespace s2bridge {
struct Declaration {
    s2resolve::TargetRecipe recipe;
    std::string target_validation;
    s2fn::AbiSignature abi;
    s2fn::AbiInfo info;
};
using Resolver = std::function<bool(const s2resolve::TargetRecipe&, s2resolve::Resolution&, std::string&)>;
s2fn::Result<Declaration> Parse(const std::string& target, const std::string& abi,
                                const std::string& fingerprint);
s2fn::Result<s2resolve::Resolution> Resolve(const Declaration&, const Resolver& = s2resolve::Resolve,
    s2validate::Ops = {}, std::function<bool(uintptr_t, void*, size_t)> read_live = {});
}
#ifndef S2FN_VALIDATION_ONLY
#include "../include/s2script_core.h"
#include <functional>
namespace s2bridge {
using TargetId = long long;
static_assert(sizeof(S2FunctionValue)==16 && offsetof(S2FunctionValue,bits)==8, "function transport layout");
// ptr transport is a host handle or copy request, NEVER a native address.
// flags select a host projection codec; aux/bits are interpreted only by it.
enum class ValueKind : unsigned char { Void, Bool, I32, U32, I64, U64, F32, F64, Pointer };
enum class PointerProjection : unsigned char { Entity = 1, NullableEntity, Opaque, String, Vector };
struct CallStorage { std::vector<std::shared_ptr<void>> retained; };
class PointerCodec {
public:
    virtual ~PointerCodec() = default;
    // Must validate host liveness and retain/copy every pointee for the call scope.
    virtual s2fn::Result<s2fn::NativeValue> Decode(const S2FunctionValue&, CallStorage&) = 0;
    // request flags/aux select the projection; encode/adopt an opaque host handle.
    virtual s2fn::Result<S2FunctionValue> Encode(const s2fn::NativeValue&, const S2FunctionValue& request) = 0;
};
class DispatchSink {
public:
    virtual ~DispatchSink() = default;
    // Owner zero means engine-originated entry with no suppression.
    virtual void Dispatch(TargetId, unsigned long long suppressed_owner, s2fn::DispatchFrame&) = 0;
    virtual void Error(TargetId, const char*) noexcept = 0;
};
class Service {
public:
    explicit Service(Resolver = s2resolve::Resolve);
    ~Service();
    Service(const Service&) = delete;
    Service& operator=(const Service&) = delete;
    // Host wiring is immutable while records exist. The host owns these objects.
    bool SetDispatchSink(DispatchSink*);
    bool SetPointerCodec(PointerCodec*);
    s2fn::Result<TargetId> Prepare(const std::string& canonical_id, const std::string& target,
                                 const std::string& abi, const std::string& fingerprint);
    s2fn::Result<S2FunctionValue> Call(TargetId, unsigned long long owner,
        const S2FunctionValue*, int argc, S2FunctionValue result_request = {});
    // Positive opaque receipt (provider id + 1); zero is the C boundary failure.
    s2fn::Result<long long> HookAcquire(TargetId);
    bool HookRelease(TargetId);
    bool TargetRelease(TargetId);
    S2HookReceipt Receipt(TargetId) const;
    // Host safe-boundary drain: never waits. False means callbacks/removal still own records.
    bool Collect();
private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
Service& Global();
}
using s2_function_target_id = long long;
extern "C" {
s2_function_target_id S2_FunctionPrepare(const char*, const char*, const char*, const char*, char*, int);
int S2_FunctionCall(s2_function_target_id, unsigned long long, const S2FunctionValue*, int,
                    S2FunctionValue*, char*, int);
long long S2_FunctionHookAcquire(s2_function_target_id, char*, int);
int S2_FunctionHookRelease(s2_function_target_id);
int S2_FunctionTargetRelease(s2_function_target_id);
}
#endif
