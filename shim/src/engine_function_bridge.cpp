#include "engine_function_bridge.h"
#include "../third_party/json.hpp"
#include "vtable.h"
#include <limits>
#include <stdexcept>
using nlohmann::json;
namespace s2bridge {
namespace {
void require(bool condition, const char* why) { if (!condition) throw std::runtime_error(why); }
void keys(const json& value, std::initializer_list<const char*> allowed) {
    require(value.is_object(), "expected normalized object");
    for (auto i=value.begin(); i!=value.end(); ++i) {
        bool found=false; for (auto key : allowed) if (i.key()==key) found=true;
        require(found, "unknown normalized field");
    }
}
std::string string(const json& j, const char* key) {
    auto s=j.at(key).get<std::string>();
    require(!s.empty() && s.find('\0')==std::string::npos, "empty/NUL normalized string");
    return s;
}
void validator(const json& j) {
    keys(j,{"prologue","string-xref","vtable-member"});
    for (auto key : {"prologue","vtable-member"}) if (j.contains(key)) string(j,key);
    if (j.contains("string-xref")) {
        const auto& x=j.at("string-xref"); keys(x,{"at","dispOff","instrLen","expect"});
        for (auto key : {"at","dispOff","instrLen"})
            require(x.at(key).is_number_integer() && x.at(key).get<int64_t>()>=0 &&
                x.at(key).get<int64_t>()<=INT32_MAX,"invalid string-xref bounds");
        require(x.at("instrLen").get<int64_t>()>0 &&
            x.at("dispOff").get<int64_t>()+4<=x.at("instrLen").get<int64_t>() &&
            string(x,"expect").size()<=256,"invalid string-xref bounds");
    }
}
s2fn::AbiAtom atom(const json& j, bool returns) {
    keys(j,returns ? std::initializer_list<const char*>{"native","projection"} :
                    std::initializer_list<const char*>{"name","native","projection","mutable"});
    s2fn::AbiAtom result{string(j,"native"),string(j.at("projection"),"id")};
    keys(j.at("projection"),{"id","version"});
    require(j.at("projection").at("version")==1,"unsupported projection version");
    bool valid=result.native==result.projection && result.native!="ptr" && result.native!="u8";
    if (result.native=="void") valid=returns && result.projection=="void";
    if (result.native=="u8") valid=result.projection=="bool";
    if (result.native=="ptr") valid=result.projection=="entity" || result.projection=="entity?" ||
        result.projection=="string" || result.projection=="vector";
    require(valid,"unsupported native/projection pair");
    return result;
}
}
s2fn::Result<Declaration> Parse(const std::string& target, const std::string& abi, const std::string& fingerprint) {
    try {
        Declaration d; auto t=json::parse(target); auto a=json::parse(abi);
        keys(t,{"kind","module","pattern","class","index","resolve","derivation","candidateValidate","targetValidate"});
        d.recipe.module=string(t,"module"); d.recipe.strategy=string(t,"resolve");
        const auto& candidate=t.at("candidateValidate"); const auto& final=t.at("targetValidate");
        validator(candidate); validator(final);
        auto kind=string(t,"kind");
        if (kind=="signature") {
            require(!t.contains("class") && !t.contains("index"),"unexpected virtual fields");
            d.recipe.pattern=string(t,"pattern");
            const auto& s=d.recipe.strategy;
            auto derivation=s=="direct" ? "identity" : s=="validated-call" ? "e8-rel32" : s.c_str();
            require((s=="direct" || s=="validated-call" || s=="ctor-body-xref" || s=="lea-disp") &&
                string(t,"derivation")==derivation,"resolver/derivation mismatch");
            if (s=="validated-call") {
                require(candidate.contains("string-xref"),"validated-call requires candidate string-xref");
                d.recipe.validate_json=candidate.dump(); d.target_validation=final.dump();
            } else {
                require(candidate.empty() && !final.empty(),"invalid validator stage"); d.recipe.validate_json=final.dump();
            }
        } else {
            require(kind=="virtual" && !t.contains("pattern"),"unsupported target kind");
            d.recipe.kind=s2resolve::Kind::Virtual; d.recipe.class_name=string(t,"class");
            require(t.at("index").is_number_integer(),"invalid virtual index");
            auto index=t.at("index").get<int64_t>(); require(index>=0 && index<512,"invalid virtual index");
            d.recipe.vtable_index=static_cast<int>(index);
            require(d.recipe.strategy=="direct" && string(t,"derivation")=="virtual-slot" &&
                candidate.empty() && final.contains("prologue"),"invalid virtual target");
            d.recipe.validate_json=final.dump();
        }
        keys(a,{"platform","receiver","fingerprint","stackCopyBytes","parameters","returns"});
        d.abi.platform=string(a,"platform"); d.abi.receiver=string(a,"receiver");
        d.abi.returns=atom(a.at("returns"),true);
        require(a.at("parameters").is_array(),"expected parameters array");
        for (const auto& p : a.at("parameters")) d.abi.parameters.push_back(atom(p,false));
        auto info=s2fn::Validate(d.abi); if (!info) return {{},info.error};
        require(info.value.fingerprint==fingerprint && string(a,"fingerprint")==fingerprint,"ABI fingerprint mismatch");
        require(a.at("stackCopyBytes")==info.value.stack_bytes,"stack-copy mismatch");
        d.info=std::move(info.value); return {std::move(d),{}};
    } catch (const std::exception& e) { return {{},std::string("invalid normalized declaration: ")+e.what()}; }
}
s2fn::Result<s2resolve::Resolution> Resolve(const Declaration& d, const Resolver& resolver,
    s2validate::Ops ops, std::function<bool(uintptr_t,void*,size_t)> read_live) {
    s2resolve::Resolution r; std::string reason;
    if (!resolver || !resolver(d.recipe,r,reason)) return {{},"resolution unavailable: "+reason};
    if (!r.image || !r.address || !r.image->executable(r.address)) return {{},"resolution lacks verified executable image"};
    if (!d.target_validation.empty()) {
        s2validate::ModuleView view;
        view.read_code=[image=r.image](auto at,auto out,auto n){ return image->read(at,out,n); };
        view.executable=[image=r.image](auto at,auto n){ return image->executable(at,n); };
        view.read_live=read_live ? std::move(read_live) : [image=r.image](auto at,auto out,auto n){return image->read_live(at,out,n);};
        if (!ops.vtable_from_image) ops.vtable_from_image=[image=r.image](const char* name){return s2vtable::GetVTableByName(*image,name);};
#ifndef S2FN_VALIDATION_ONLY
        if (!ops.original_virtual) ops.original_virtual=&KHook::FindOriginalVirtual;
#endif
        char why[512]{};
        if (!s2validate::Run(d.target_validation.c_str(),view,d.recipe.module.c_str(),
            reinterpret_cast<void*>(r.address),ops,why,sizeof why)) return {{},std::string("target validation: ")+why};
    }
    return {std::move(r),{}};
}
}
#ifndef S2FN_VALIDATION_ONLY
#include <map>
#include <mutex>
#include <tuple>
#include <cstdio>
namespace s2bridge {
namespace {
using PhysicalKey = std::tuple<uint64_t,uint64_t,uintptr_t,std::string,uintptr_t>;
PhysicalKey key(const s2resolve::Resolution& r) {
    const auto& i=r.image->identity(); return {i.device,i.inode,i.load_bias,i.build_id,r.address};
}
struct Bypass { const void* service; TargetId target; unsigned long long owner; };
thread_local std::vector<Bypass> bypass;
ValueKind kind(const std::string& atom) {
    if (atom=="u8") return ValueKind::Bool;
    if (atom=="i32") return ValueKind::I32;
    if (atom=="u32") return ValueKind::U32;
    if (atom=="i64") return ValueKind::I64;
    if (atom=="u64") return ValueKind::U64;
    if (atom=="f32") return ValueKind::F32;
    if (atom=="f64") return ValueKind::F64;
    if (atom=="ptr") return ValueKind::Pointer;
    return ValueKind::Void;
}
bool pointer_request(const S2FunctionValue& v) {
    return v.kind==static_cast<unsigned char>(ValueKind::Pointer) && v.reserved==0 &&
        v.flags>=static_cast<unsigned char>(PointerProjection::Entity) &&
        v.flags<=static_cast<unsigned char>(PointerProjection::Vector);
}
}
struct Service::Impl {
    struct Record final : s2fn::DispatchSink {
        Impl& host;
        TargetId id;
        std::string canonical_id;
        Declaration declaration;
        s2resolve::Resolution resolution; // private diagnostic/provenance retains its image
        std::unique_ptr<s2fn::RuntimeBinding> binding;
        size_t refs=1, subscriptions=0, active_calls=0;
        Record(Impl& h,TargetId i,std::string name,Declaration d,s2resolve::Resolution r)
            : host(h),id(i),canonical_id(std::move(name)),declaration(std::move(d)),resolution(std::move(r)) {}
        void Dispatch(s2fn::DispatchFrame& frame) override {
            std::shared_ptr<Record> retained;
            s2bridge::DispatchSink* sink=nullptr;
            {
                std::lock_guard<std::recursive_mutex> lock(host.mu);
                retained=host.records.at(id);
                if (subscriptions) sink=host.sink;
            }
            // The record hold prevents collection/interface replacement while
            // the host runs. Never hold service bookkeeping across host code.
            unsigned long long owner=0;
            for (auto i=bypass.rbegin();i!=bypass.rend();++i)
                if (i->service==&host && i->target==id) { owner=i->owner; break; }
            if (sink) sink->Dispatch(id,owner,frame);
        }
        void Error(const char* why) noexcept override {
            std::shared_ptr<Record> retained;
            s2bridge::DispatchSink* sink=nullptr;
            {
                std::lock_guard<std::recursive_mutex> lock(host.mu);
                retained=host.records.at(id);
                sink=host.sink;
            }
            if (sink) sink->Error(id,why);
        }
    };
    mutable std::recursive_mutex mu;
    Resolver resolver;
    DispatchSink* sink=nullptr;
    PointerCodec* codec=nullptr;
    TargetId next=1;
    std::map<TargetId,std::shared_ptr<Record>> records;
    std::map<PhysicalKey,TargetId> physical;
    explicit Impl(Resolver r):resolver(std::move(r)) {}
    std::shared_ptr<Record> find(TargetId id) {
        auto i=records.find(id); return i==records.end() || !i->second->refs ? nullptr : i->second;
    }
    void collect() {
        for (auto i=records.begin();i!=records.end();) {
            auto& r=*i->second;
            if (!r.refs && !r.subscriptions && r.binding->RemovalComplete() && i->second.use_count()==1) {
                physical.erase(key(r.resolution)); i=records.erase(i);
            } else ++i;
        }
        S2Hook_DrainRetirement();
    }
};
Service::Service(Resolver r):impl_(new Impl(std::move(r))) {}
Service::~Service() {
    // A service is a host-lifetime owner. Destroy only at a completed teardown
    // boundary; asynchronous callbacks must never outlive their sink/records.
    if (!Collect()) std::terminate();
}
bool Service::SetDispatchSink(DispatchSink* sink) {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu); impl_->collect();
    if (!impl_->records.empty()) return false;
    impl_->sink=sink; return true;
}
bool Service::SetPointerCodec(PointerCodec* codec) {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu); impl_->collect();
    if (!impl_->records.empty()) return false;
    impl_->codec=codec; return true;
}
s2fn::Result<TargetId> Service::Prepare(const std::string& name,const std::string& target,
                                       const std::string& abi,const std::string& fingerprint) {
    if (name.empty() || name.find('\0')!=std::string::npos) return {0,"invalid canonical id"};
    auto d=Parse(target,abi,fingerprint); if (!d) return {0,name+": "+d.error};
    auto resolution=Resolve(d.value,impl_->resolver); if (!resolution) return {0,name+": "+resolution.error};
    // The immutable resolver may contact the host. Only interning needs the lock.
    std::lock_guard<std::recursive_mutex> lock(impl_->mu); impl_->collect();
    auto physical=key(resolution.value); auto prior=impl_->physical.find(physical);
    if (prior!=impl_->physical.end()) {
        auto& r=*impl_->records.at(prior->second);
        if (r.declaration.info.fingerprint!=fingerprint)
            return {0,"ABI conflict: "+r.canonical_id+" and "+name};
        if (!r.refs) return {0,name+": physical target retirement pending"};
        if (r.refs==std::numeric_limits<size_t>::max()) return {0,"target reference overflow"};
        ++r.refs; return {r.id,{}};
    }
    if (impl_->next==std::numeric_limits<TargetId>::max()) return {0,"target id exhausted"};
    auto r=std::make_shared<Impl::Record>(*impl_,impl_->next,name,std::move(d.value),std::move(resolution.value));
    auto binding=s2fn::RuntimeBinding::Create(r->declaration.abi,*r);
    if (!binding) return {0,name+": "+binding.error};
    auto error=binding.value->BindTarget(reinterpret_cast<void*>(r->resolution.address));
    if (!error.empty()) return {0,name+": "+error};
    r->binding=std::move(binding.value);
    impl_->records.emplace(r->id,r);
    try { impl_->physical.emplace(std::move(physical),r->id); }
    catch (...) { impl_->records.erase(r->id); throw; }
    ++impl_->next; return {r->id,{}};
}
s2fn::Result<S2FunctionValue> Service::Call(TargetId id,unsigned long long owner,
    const S2FunctionValue* args,int argc,S2FunctionValue request) {
    std::shared_ptr<Impl::Record> r;
    PointerCodec* codec=nullptr;
    {
        std::lock_guard<std::recursive_mutex> lock(impl_->mu);
        r=impl_->find(id); if (!r) return {{},"target handle unavailable"};
        if (r->active_calls==std::numeric_limits<size_t>::max()) return {{},"active call overflow"};
        ++r->active_calls;
        codec=impl_->codec;
    }
    struct ReleaseCall {
        Impl& host;
        Impl::Record& record;
        ~ReleaseCall() {
            std::lock_guard<std::recursive_mutex> lock(host.mu);
            --record.active_calls;
        }
    } release{*impl_,*r};
    // r keeps its map entry and immutable host interfaces alive through decode,
    // native execution, encode, and call-storage destruction, without this lock.
    const auto& abi=r->declaration.abi;
    size_t receiver=abi.receiver=="entity" ? 1 : 0;
    if (argc<0 || static_cast<size_t>(argc)!=abi.parameters.size()+receiver || (argc && !args))
        return {{},"argument count mismatch"};
    if (abi.returns.native=="ptr" && (!pointer_request(request) || !codec))
        return {{},"pointer result codec/request unavailable"};
    CallStorage storage; std::vector<s2fn::NativeValue> values; values.reserve(argc);
    for (int i=0;i<argc;++i) {
        const auto& v=args[i]; const std::string atom=receiver && i==0 ? "ptr" : abi.parameters[i-receiver].native;
        if (v.reserved || v.kind!=static_cast<unsigned char>(kind(atom))) return {{},"argument value kind/reserved mismatch"};
        if (atom=="ptr") {
            if (!pointer_request(v) || !codec) return {{},"pointer argument codec/request unavailable"};
            if (receiver && i==0 && v.flags!=static_cast<unsigned char>(PointerProjection::Entity))
                return {{},"receiver requires live entity projection"};
            auto native=codec->Decode(v,storage); if (!native) return {{},native.error};
            if (receiver && i==0 && !native.value.Get<void*>()) return {{},"receiver entity unavailable"};
            values.push_back(native.value);
        } else {
            if (v.flags || v.aux) return {{},"scalar transport has projection metadata"};
            auto native=s2fn::NativeValue::From(v.bits);
            if (atom=="u8") native=s2fn::NativeValue::From<uint8_t>(v.bits ? 1 : 0);
            values.push_back(native);
        }
    }
    bypass.push_back({impl_.get(),id,owner});
    struct Pop { ~Pop(){bypass.pop_back();} } pop;
    auto result=r->binding->Call(values.data(),values.size()); if (!result) return {{},result.error};
    if (abi.returns.native=="ptr") {
        auto encoded=codec->Encode(result.value,request);
        if (encoded && (!pointer_request(encoded.value) || encoded.value.flags!=request.flags))
            return {{},"pointer codec returned invalid projection"};
        return encoded;
    }
    S2FunctionValue out{}; out.kind=static_cast<unsigned char>(kind(abi.returns.native));
    // Copy only the native width; i32 and bool upper bytes stay deterministically zero.
    size_t width=abi.returns.native=="void" ? 0 : abi.returns.native=="u8" ? 1 :
        (abi.returns.native=="i32" || abi.returns.native=="u32" || abi.returns.native=="f32") ? 4 : 8;
    std::memcpy(&out.bits,result.value.bytes.data(),width);
    return {out,{}};
}
s2fn::Result<long long> Service::HookAcquire(TargetId id) {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu);
    auto r=impl_->find(id); if (!r) return {0,"target handle unavailable"};
    if (!impl_->sink) return {0,"function dispatch sink unavailable"};
    if (r->subscriptions==std::numeric_limits<size_t>::max()) return {0,"subscription overflow"};
    if (!r->subscriptions) {
        // A call's host codec may still be running before RuntimeBinding has its
        // own activity lease. Serialize installation against that entire scope.
        if (r->active_calls) return {0,"hook acquisition requires idle target"};
        const auto receipt=r->binding->Configure(reinterpret_cast<void*>(r->resolution.address));
        if (!receipt.Accepted()) return {0,"hook acquisition failed: "+receipt.reason};
    }
    ++r->subscriptions; return {static_cast<long long>(r->binding->Receipt().id)+1,{}};
}
bool Service::HookRelease(TargetId id) {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu);
    auto i=impl_->records.find(id); if (i==impl_->records.end() || !i->second->subscriptions) return false;
    auto& r=*i->second; if (--r.subscriptions==0) r.binding->BeginRemove();
    impl_->collect(); return true;
}
bool Service::TargetRelease(TargetId id) {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu);
    auto r=impl_->find(id); if (!r) return false;
    --r->refs;
    // Hook ledger entries release separately; retained subscriptions keep storage alive.
    r.reset(); impl_->collect(); return true;
}
S2HookReceipt Service::Receipt(TargetId id) const {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu);
    auto i=impl_->records.find(id);
    return i==impl_->records.end() ? S2HookReceipt{KHook::INVALID_HOOK,S2HookState::Failed,"target handle unavailable"} : i->second->binding->Receipt();
}
bool Service::Collect() {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu); impl_->collect(); return impl_->records.empty();
}
Service& Global() {
    // Explicitly drained by the host ledger/safe-boundary integration; static
    // destruction cannot race the provider's asynchronous removal thread.
    static Service* service=new Service;
    return *service;
}
}
namespace {
void reason_out(char* out,int cap,const std::string& reason) {
    if (out && cap>0) std::snprintf(out,static_cast<size_t>(cap),"%s",reason.c_str());
}
}
extern "C" long long S2_FunctionPrepare(const char* name,const char* target,const char* abi,const char* fingerprint,char* reason,int cap) {
    try {
        if (!name || !target || !abi || !fingerprint) { reason_out(reason,cap,"null declaration input"); return 0; }
        auto result=s2bridge::Global().Prepare(name,target,abi,fingerprint); reason_out(reason,cap,result.error);return result ? result.value : 0;
    } catch (...) { reason_out(reason,cap,"native function preparation exception");return 0; }
}
extern "C" int S2_FunctionCall(long long id,unsigned long long owner,const S2FunctionValue* args,int argc,S2FunctionValue* ret,char* reason,int cap) {
    try {
        if (!ret) {reason_out(reason,cap,"missing return transport");return 0;}
        auto result=s2bridge::Global().Call(id,owner,args,argc,*ret);
        reason_out(reason,cap,result.error); if (!result) return 0; *ret=result.value;return 1;
    } catch (...) {reason_out(reason,cap,"native function call exception");return 0;}
}
extern "C" long long S2_FunctionHookAcquire(long long id,char* reason,int cap) {
    try {auto result=s2bridge::Global().HookAcquire(id);reason_out(reason,cap,result.error);return result ? result.value : 0;}
    catch (...) {reason_out(reason,cap,"native function hook exception");return 0;}
}
extern "C" int S2_FunctionHookRelease(long long id) {
    try {return s2bridge::Global().HookRelease(id) ? 1 : 0;} catch (...) {return 0;}
}
extern "C" int S2_FunctionTargetRelease(long long id) {
    try {return s2bridge::Global().TargetRelease(id) ? 1 : 0;} catch (...) {return 0;}
}
#endif
