#ifdef S2BRIDGE_TARGET_FIXTURE
#include <thread>
static volatile int fixture_calls=0;
extern "C" __attribute__((visibility("default"),noinline)) int s2bridge_fixture_native(int n) {
    ++fixture_calls; return n+1;
}
extern "C" __attribute__((visibility("default"),noinline)) int s2bridge_fixture_other(int n) {
    ++fixture_calls; return n+2;
}
extern "C" __attribute__((visibility("default"),noinline)) void* s2bridge_fixture_pointer(void* p) {
    ++fixture_calls; return p;
}
extern "C" __attribute__((visibility("default"),noinline)) int s2bridge_fixture_join(int n) {
    int result=0;
    auto volatile target=&s2bridge_fixture_other;
    std::thread worker([&] { result=target(n); });
    worker.join();
    return result+1;
}
#else
#include "engine_function_bridge.h"
#include "../third_party/json.hpp"
#include <cassert>
#include <cstring>
#include <iostream>
using nlohmann::json;
#define CHECK(c, label) assert((c) && label)
namespace {
void put(std::vector<uint8_t>& b, size_t at, uint64_t value, size_t n) {
    for (size_t i = 0; i < n; ++i) b.at(at+i) = uint8_t(value >> (8*i));
}
struct Fixture {
    uintptr_t base = 0x100000;
    uint64_t device=7;
    std::vector<uint8_t> file = std::vector<uint8_t>(0x2200);
    std::vector<uint8_t> live = std::vector<uint8_t>(0x5000);
    std::shared_ptr<const s2original::Image> image;
    std::string reason;
    explicit Fixture(uintptr_t address_base=0x100000) : base(address_base) {
        std::memcpy(file.data(), "\177ELF\2\1\1", 7);
        put(file,16,3,2); put(file,18,62,2); put(file,20,1,4);
        put(file,32,64,8); put(file,52,64,2); put(file,54,56,2); put(file,56,4,2);
        auto segment = [&](int i, int type, int flags, int off, int addr, int size, int mem) {
            size_t p = 64+i*56;
            put(file,p,type,4); put(file,p+4,flags,4); put(file,p+8,off,8);
            put(file,p+16,addr,8); put(file,p+32,size,8); put(file,p+40,mem,8);
            put(file,p+48,1,8);
        };
        segment(0,1,4,0,0,0x800,0x800);
        segment(1,1,5,0x1000,0x1000,0x400,0x400);
        segment(2,1,6,0x2000,0x3000,0x200,0x1000); // writable data + BSS
        segment(3,4,4,0x300,0x300,20,20);
        put(file,0x300,4,4); put(file,0x304,4,4); put(file,0x308,3,4);
        std::memcpy(file.data()+0x30c,"GNU",4); put(file,0x310,0x3412cdab,4);
        std::fill(file.begin()+0x1000,file.begin()+0x1400,0x90);
        std::memcpy(live.data()+0x500,"ScopeGood",10);
        std::memcpy(live.data()+0x520,"ScopeWrong",11);
        bytes(0x1200,{0x55,0x48,0x89,0xe5,0xc3});
        bytes(0x1300,{0x55,0x48,0x89,0xe5,0x41,0x57,0xc3});
    }
    void bytes(size_t at, std::initializer_list<uint8_t> b) {
        std::copy(b.begin(),b.end(),file.begin()+at);
    }
    void lea(size_t at, size_t target) {
        bytes(at,{0x4c,0x8d,0x35}); put(file,at+3,int32_t(target-(at+7)),4);
    }
    void call(size_t at, size_t target, size_t string) {
        file[at]=0xe8; put(file,at+1,int32_t(target-(at+5)),4); lea(at+5,string);
    }
    bool mapped(uintptr_t at, size_t n) const {
        if (!n || at<base) return false;
        auto off=at-base;
        for (auto r : {std::pair<size_t,size_t>{0,0x800},{0x1000,0x1400},{0x3000,0x4000}})
            if (off>=r.first && off<r.second && n<=r.second-off) return true;
        return false;
    }
    void freeze() {
        s2original::Identity id{device,9,base,"abcd1234"};
        image=s2original::FromElf(file,id,id,{{base,base+0x800,0,device,9},
            {base+0x1000,base+0x1400,0x1000,device,9},
            {base+0x3000,base+0x4000,0x2000,device,9}},reason);
        CHECK(image, "fixture has a verified original image");
        std::copy(file.begin()+0x1000,file.begin()+0x1400,live.begin()+0x1000);
        std::fill(live.begin()+0x1000,live.begin()+0x1010,0xcc); // peer-patched bytes
    }
    s2resolve::Sources sources() {
        s2resolve::Sources s;
        s.image=image;
        s.mapped=[this](uintptr_t at,size_t n) { return mapped(at,n); };
        s.read_live=[this](uintptr_t at,void* out,size_t n) {
            if (!mapped(at,n)) return false;
            std::memcpy(out,live.data()+at-base,n); return true;
        };
        return s;
    }
};

json target() { return {{"kind","signature"},{"module","fixture"},{"pattern","E8 ?? ?? ?? ?? 4C 8D 35"},{"resolve","validated-call"},{"derivation","e8-rel32"},{"candidateValidate",{{"string-xref",{{"at",5},{"dispOff",3},{"instrLen",7},{"expect","ScopeGood"}}}}},{"targetValidate",{{"prologue","55 48 89 E5 41 57"}}}}; }
json abi() { return {{"platform","linux-x86_64-sysv"},{"receiver","none"},{"fingerprint","linux-x86_64-sysv:none:i32(i32)"},{"stackCopyBytes",128},{"parameters",json::array({{{"name","value"},{"native","i32"},{"projection",{{"id","i32"},{"version",1}}},{"mutable",json::array()}}})},{"returns",{{"native","i32"},{"projection",{{"id","i32"},{"version",1}}}}}}; }
void stages() {
    Fixture f; f.call(0x1000,0x1200,0x520); f.call(0x1100,0x1300,0x500); f.freeze();
    int resolves=0;
    s2bridge::Resolver resolver=[&](const auto& recipe,auto& out,auto& reason) { ++resolves; return s2resolve::Evaluate(recipe,f.sources(),out,reason); };
    auto parsed=s2bridge::Parse(target().dump(),abi().dump(),abi()["fingerprint"]);
    assert(parsed);
    auto resolved=s2bridge::Resolve(parsed.value,resolver,f.sources().ops,f.sources().read_live);
    assert(resolved && resolved.value.address==f.base+0x1300 && resolves==1);
    auto t=target(); t["targetValidate"]["prologue"]="E8";
    parsed=s2bridge::Parse(t.dump(),abi().dump(),abi()["fingerprint"]);
    assert(parsed && !s2bridge::Resolve(parsed.value,resolver,f.sources().ops,f.sources().read_live));
    // Two semantically valid callers remain ambiguous even if a target validator
    // would select one of the callees: target validation cannot select candidates.
    Fixture ambiguous; ambiguous.call(0x1000,0x1200,0x500); ambiguous.call(0x1100,0x1300,0x500); ambiguous.freeze();
    resolver=[&](const auto& recipe,auto& out,auto& reason) { return s2resolve::Evaluate(recipe,ambiguous.sources(),out,reason); };
    parsed=s2bridge::Parse(target().dump(),abi().dump(),abi()["fingerprint"]);
    auto rejected=s2bridge::Resolve(parsed.value,resolver,ambiguous.sources().ops,ambiguous.sources().read_live);
    assert(!rejected && rejected.error.find("ambiguous")!=std::string::npos);
    auto a=abi(); a["parameters"][0]["native"]="i16";
    assert(!s2bridge::Parse(target().dump(),a.dump(),abi()["fingerprint"]));
    a=abi(); a["varargs"]=true; assert(!s2bridge::Parse(target().dump(),a.dump(),abi()["fingerprint"]));
    assert(!s2bridge::Parse(target().dump(),abi().dump(),"wrong"));
    t=target(); t["derivation"]="identity"; assert(!s2bridge::Parse(t.dump(),abi().dump(),abi()["fingerprint"]));
    t=target(); t["candidateValidate"]["oops"]=true; assert(!s2bridge::Parse(t.dump(),abi().dump(),abi()["fingerprint"]));
    std::cout << "PASS bridge normalization and real S1 staged resolution\n";
}
}
#ifndef S2FN_VALIDATION_ONLY
#include <chrono>
#include <thread>
#include <atomic>
#include <condition_variable>
#include <cstdlib>
static int allocations=0, frees=0;
extern "C" void* __real_ffi_closure_alloc(size_t,void**);
extern "C" void __real_ffi_closure_free(void*);
extern "C" void* __wrap_ffi_closure_alloc(size_t n,void** p) { auto v=__real_ffi_closure_alloc(n,p); if(v) ++allocations;return v; }
extern "C" void __wrap_ffi_closure_free(void* p) { if(p) ++frees;__real_ffi_closure_free(p); }
namespace s2fn { void TestBeforeInvocationId(RuntimeBinding*) {} void TestInvokeReturned(RuntimeBinding*) {} void TestReturnUnlocked(RuntimeBinding*) {} }
namespace {
extern "C" int s2bridge_fixture_native(int);
extern "C" int s2bridge_fixture_other(int);
extern "C" int s2bridge_fixture_join(int);
extern "C" void* s2bridge_fixture_pointer(void*);
#define native s2bridge_fixture_native
struct Sink : s2bridge::DispatchSink {
    s2bridge::Service* service=nullptr;
    s2bridge::TargetId id=0;
    std::vector<unsigned long long> seen, post;
    bool nested=false, release=false;
    void Dispatch(s2bridge::TargetId target_id,unsigned long long owner,s2fn::DispatchFrame& frame) override {
        assert(target_id==id);
        if (frame.phase!=s2fn::Phase::Pre) { post.push_back(owner); return; }
        seen.push_back(owner);
        if (nested && owner==11) {
            S2FunctionValue v{}; v.kind=static_cast<unsigned char>(s2bridge::ValueKind::I32); v.bits=7;
            auto result=service->Call(id,22,&v,1); assert(result && result.value.bits==8);
            nested=false;
        }
        if (release) { release=false; assert(service->HookRelease(id)); assert(service->TargetRelease(id)); assert(!service->Collect() && frees==0); }
    }
    void Error(s2bridge::TargetId,const char*) noexcept override { std::abort(); }
};
struct Codec : s2bridge::PointerCodec {
    bool live=true;
    std::shared_ptr<int> pointee=std::make_shared<int>(17);
    s2fn::Result<s2fn::NativeValue> Decode(const S2FunctionValue& value,s2bridge::CallStorage& storage) override {
        if (!live || value.bits!=73) return {{},"stale host handle"};
        // This fixture owns its native pointee; the transported token is unrelated
        // to its address. Production codecs use host entity/opaque liveness books.
        storage.retained.push_back(pointee);
        return {s2fn::NativeValue::From<void*>(pointee.get()),{}};
    }
    s2fn::Result<S2FunctionValue> Encode(const s2fn::NativeValue& value,const S2FunctionValue& request) override {
        if (value.Get<void*>()!=pointee.get()) return {{},"unregistered host result"};
        auto result=request;result.bits=73;return {result,{}};
    }
};
// A stalled worker/join fails the process after five seconds instead of hanging
// CI or destructing a still-joinable worker. This is an actual stock closure path.
void worker_join_regression() {
    struct Deadline {
        std::mutex mu;
        std::condition_variable cv;
        bool done=false;
        std::thread watchdog;
        Deadline() : watchdog([this] {
            std::unique_lock<std::mutex> lock(mu);
            if (!cv.wait_for(lock,std::chrono::seconds(5),[this] {return done;})) {
                std::cerr << "FAIL bridge worker/join exceeded five seconds\n";
                std::_Exit(87);
            }
        }) {}
        ~Deadline() {
            {std::lock_guard<std::mutex> lock(mu);done=true;}
            cv.notify_all();watchdog.join();
        }
    } deadline;
    struct WorkerSink : s2bridge::DispatchSink {
        s2bridge::Service* service=nullptr;
        s2bridge::TargetId joining=0, worker=0;
        std::atomic<int> worker_pre{0};
        void Dispatch(s2bridge::TargetId id,unsigned long long,s2fn::DispatchFrame& frame) override {
            if (frame.phase!=s2fn::Phase::Pre) return;
            if (id==worker) {++worker_pre;return;}
            assert(id==joining);
            // Also catch holding the bookkeeping mutex across the host sink.
            std::thread nested([&] {
                assert(!service->SetDispatchSink(nullptr));
                auto volatile target=&s2bridge_fixture_other;
                assert(target(8)==10);
            });
            nested.join();
        }
        void Error(s2bridge::TargetId,const char*) noexcept override {std::abort();}
    } sink;
    uintptr_t address=reinterpret_cast<uintptr_t>(&s2bridge_fixture_other);
    s2bridge::Service service([&](const auto&,auto& out,auto&) {
        Fixture f(address-0x1200);f.freeze();out.address=address;out.image=f.image;return true;
    });
    sink.service=&service;assert(service.SetDispatchSink(&sink));
    auto t=target();t["resolve"]="direct";t["derivation"]="identity";t["candidateValidate"]=json::object();
    auto a=abi();
    auto worker=service.Prepare("worker",t.dump(),a.dump(),a["fingerprint"]);assert(worker);sink.worker=worker.value;
    assert(service.HookAcquire(worker.value));
    address=reinterpret_cast<uintptr_t>(&s2bridge_fixture_join);
    auto joining=service.Prepare("joining",t.dump(),a.dump(),a["fingerprint"]);assert(joining);sink.joining=joining.value;
    S2FunctionValue input{};input.kind=static_cast<unsigned char>(s2bridge::ValueKind::I32);input.bits=7;
    auto result=service.Call(joining.value,11,&input,1);
    assert(result && result.value.bits==10 && sink.worker_pre==1);
    // Second entry also performs a worker/join inside the injected host sink.
    assert(service.HookAcquire(joining.value));
    result=service.Call(joining.value,11,&input,1);
    assert(result && result.value.bits==10 && sink.worker_pre==3);
    for (auto id : {joining.value,worker.value}) {
        assert(service.HookRelease(id));assert(service.TargetRelease(id));
    }
    while (!service.Collect()) std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(allocations==frees);
    std::cout << "PASS bounded native and sink worker/join without service-lock deadlock\n";
}
static S2FunctionFrameInfo last_frame{};
static std::vector<unsigned long long> transport_invocations;
static int scalar_dispatch(long long target_id,const S2FunctionFrameInfo* info,int phase) {
    assert(info && info->version==1 && info->struct_size==48 && info->invocation_id);
    const char* fp="linux-x86_64-sysv:none:i32(i32)";char why[256]{};S2FunctionValue value{};
    assert(S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,0,2,&value,why,sizeof why));
    const auto old=value;
    assert(!S2_FunctionFrameRead(target_id,info->frame_token+1,info->native_epoch,fp,0,2,&value,why,sizeof why));
    assert(value.kind==old.kind && value.bits==old.bits);
    assert(!S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,"wrong",0,2,&value,why,sizeof why));
    if(phase==0){
        transport_invocations.push_back(info->invocation_id);
        assert(!S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,-2,2,&value,why,sizeof why));
        auto invalid=value;invalid.flags=1;
        assert(!S2_FunctionFrameWrite(target_id,info->frame_token,info->native_epoch,fp,0,&invalid,why,sizeof why));
        value.bits=19;
        assert(S2_FunctionFrameWrite(target_id,info->frame_token,info->native_epoch,fp,0,&value,why,sizeof why));
        assert(!S2_FunctionFrameCommit(target_id,info->frame_token,info->native_epoch,fp,2,nullptr,why,sizeof why));
        assert(S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,0,2,&value,why,sizeof why) && value.bits==19);
        value.bits=73;
        assert(S2_FunctionFrameCommit(target_id,info->frame_token,info->native_epoch,fp,2,&value,why,sizeof why));
        assert(!S2_FunctionFrameCommit(target_id,info->frame_token,info->native_epoch,fp,2,&value,why,sizeof why));
    }else{
        assert(transport_invocations.back()==info->invocation_id);transport_invocations.pop_back();
        assert(info->flags==1);
        assert(S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,-2,2,&value,why,sizeof why) && value.bits==73);
        assert(!S2_FunctionFrameWrite(target_id,info->frame_token,info->native_epoch,fp,0,&value,why,sizeof why));
        assert(!S2_FunctionFrameCommit(target_id,info->frame_token,info->native_epoch,fp,2,&value,why,sizeof why));
    }
    last_frame=*info;return 1;
}
static void busy_service_insertion() {
    Fixture fixture(reinterpret_cast<uintptr_t>(&native)-0x1200);fixture.freeze();
    s2bridge::Service service([&](const auto&,auto& out,auto&){out.address=reinterpret_cast<uintptr_t>(&native);out.image=fixture.image;return true;});
    s2bridge::CoreDispatchSink sink(scalar_dispatch);assert(service.SetDispatchSink(&sink));
    auto t=target();t["resolve"]="direct";t["derivation"]="identity";t["candidateValidate"]=json::object();auto a=abi();
    struct Outer : s2fn::DispatchSink {
        std::function<void(s2fn::DispatchFrame&)> callback;
        void Dispatch(s2fn::DispatchFrame& f)override{callback(f);}
        void Error(const char*)noexcept override{std::abort();}
    } outer_sink;
    s2fn::AbiSignature signature;signature.parameters={{"i32"}};signature.returns={"i32"};
    auto outer_result=s2fn::RuntimeBinding::Create(signature,outer_sink);assert(outer_result);auto outer=std::move(outer_result.value);
    assert(outer->Configure(reinterpret_cast<void*>(&native)).Accepted());
    auto binding=service.Prepare("busy-service",t.dump(),a.dump(),a["fingerprint"]);assert(binding);
    bool acquired=false;
    outer_sink.callback=[&](auto& f){
        assert(s2hook_detail::g_callback_depth>0);
        if(f.phase==s2fn::Phase::Pre && !acquired){acquired=true;assert(service.HookAcquire(binding.value));assert(service.Receipt(binding.value).state==S2HookState::Pending);}
        service.Collect();
    };
    auto input=s2fn::NativeValue::From<std::int32_t>(7);assert(outer->Call(&input,1));
    auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while(service.Receipt(binding.value).state==S2HookState::Pending && std::chrono::steady_clock::now()<deadline){std::this_thread::sleep_for(std::chrono::milliseconds(1));assert(outer->Call(&input,1));}
    assert(service.Receipt(binding.value).state==S2HookState::Active);
    assert(service.HookRelease(binding.value));assert(service.TargetRelease(binding.value));
    // Collection occurs inside a real outer observation, without global drain.
    deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while(!service.Empty() && std::chrono::steady_clock::now()<deadline){std::this_thread::sleep_for(std::chrono::milliseconds(1));assert(outer->Call(&input,1));}
    assert(service.Empty());
    binding=service.Prepare("pending-cancel-service",t.dump(),a.dump(),a["fingerprint"]);assert(binding);bool cancelled=false;
    outer_sink.callback=[&](auto& f){
        if(f.phase==s2fn::Phase::Pre && !cancelled){cancelled=true;assert(service.HookAcquire(binding.value));assert(service.Receipt(binding.value).state==S2HookState::Pending);assert(service.HookRelease(binding.value));assert(service.TargetRelease(binding.value));}
        service.Collect();
    };
    assert(outer->Call(&input,1));deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while(!service.Empty() && std::chrono::steady_clock::now()<deadline){std::this_thread::sleep_for(std::chrono::milliseconds(1));assert(outer->Call(&input,1));}assert(service.Empty());
    {std::lock_guard<std::mutex> lock(s2hook_detail::g_retire_mu);assert(s2hook_detail::g_retire.empty());}
    outer->BeginRemove();deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);while(!outer->RemovalComplete() && std::chrono::steady_clock::now()<deadline)std::this_thread::sleep_for(std::chrono::milliseconds(1));assert(outer->RemovalComplete());assert(outer->PruneCompletedTicket());outer.reset();
    std::cout<<"PASS Service busy-capsule Pending/Observe/cancel and target-local ticket collection under real observation\n";
}
static void scalar_transport() {
    Fixture fixture(reinterpret_cast<uintptr_t>(&native)-0x1200);fixture.freeze();
    s2bridge::Service service([&](const auto&,auto& out,auto&){out.address=reinterpret_cast<uintptr_t>(&native);out.image=fixture.image;return true;});
    s2bridge::CoreDispatchSink sink(scalar_dispatch);assert(service.SetDispatchSink(&sink));
    auto t=target();t["resolve"]="direct";t["derivation"]="identity";t["candidateValidate"]=json::object();auto a=abi();
    auto binding=service.Prepare("transport",t.dump(),a.dump(),a["fingerprint"]);assert(binding);assert(service.HookAcquire(binding.value));
    S2FunctionValue input{};input.kind=2;input.bits=7;auto result=service.Call(binding.value,0,&input,1);assert(result && result.value.bits==73 && transport_invocations.empty());
    char why[256]{};S2FunctionValue out{};
    assert(!S2_FunctionFrameRead(binding.value,last_frame.frame_token,last_frame.native_epoch,"linux-x86_64-sysv:none:i32(i32)",0,2,&out,why,sizeof why));
    assert(service.HookRelease(binding.value));assert(service.TargetRelease(binding.value));
    auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while(!service.Collect() && std::chrono::steady_clock::now()<deadline)std::this_thread::sleep_for(std::chrono::milliseconds(1));assert(service.Empty());
    std::cout<<"PASS scalar frame capability, staged commit, exact invocation and readonly POST\n";
}
void runtime() {
    std::cout << std::unitbuf;
    Fixture f(reinterpret_cast<uintptr_t>(&native)-0x1200); f.freeze();
    int resolves=0; bool fail=false;
    uintptr_t resolved_address=reinterpret_cast<uintptr_t>(&native);
    s2bridge::Service service([&](const auto&,auto& out,auto& why) {
        ++resolves; if (fail) { why="fixture resolve failure"; return false; }
        out.address=resolved_address; out.image=f.image;
        out.recipe="private recipe"; out.validation_receipt="private address receipt"; return true;
    });
    auto t=target(); t["resolve"]="direct";t["derivation"]="identity";t["candidateValidate"]=json::object();
    auto a=abi(); auto prepare=[&](const char* id){return service.Prepare(id,t.dump(),a.dump(),a["fingerprint"]);};
    auto bad=a; bad["parameters"][0]["native"]="aggregate";
    assert(!service.Prepare("bad",t.dump(),bad.dump(),a["fingerprint"]) && resolves==0 && allocations==0);
    fail=true;assert(!prepare("failed"));fail=false;assert(service.Collect());
    Sink sink; sink.service=&service; assert(service.SetDispatchSink(&sink));
    auto first=prepare("owner-a::call"); assert(first); sink.id=first.value;
    auto second=prepare("owner-b::call"); assert(second && second.value==first.value && resolves==3 && allocations==4);
    auto conflict=a; conflict["returns"]={{"native","f64"},{"projection",{{"id","f64"},{"version",1}}}};
    conflict["fingerprint"]="linux-x86_64-sysv:none:f64(i32)";
    const auto allocated_before_conflict=allocations;
    auto rejected=service.Prepare("owner-c::call",t.dump(),conflict.dump(),conflict["fingerprint"]);
    assert(allocations==allocated_before_conflict);
    assert(!rejected && rejected.error.find("owner-a::call")!=std::string::npos && rejected.error.find("owner-c::call")!=std::string::npos);
    S2FunctionValue v{}; v.kind=static_cast<unsigned char>(s2bridge::ValueKind::I32);v.bits=41;
    auto result=service.Call(first.value,11,&v,1); assert(result && result.value.bits==42 && sink.seen.empty());
    S2Hook_SetLifecycle(S2HookLifecycle::Retiring);assert(!service.HookAcquire(first.value));
    assert(service.Call(first.value,11,&v,1));S2Hook_SetLifecycle(S2HookLifecycle::Running);
    auto hook=service.HookAcquire(first.value); assert(hook && service.Receipt(first.value).state==S2HookState::Pending);
    auto shared=service.HookAcquire(second.value); assert(shared && hook.value==shared.value);
    sink.nested=true; result=service.Call(first.value,11,&v,1);
    assert(result && result.value.bits==42 && sink.seen==std::vector<unsigned long long>({11,22}));
    assert(sink.post==std::vector<unsigned long long>({22,11}));
    assert(service.Receipt(first.value).state==S2HookState::Active);
    assert(service.HookRelease(first.value)); assert(service.TargetRelease(first.value));
    assert(service.Call(second.value,33,&v,1) && sink.seen.back()==33);
    sink.release=true; assert(service.Call(second.value,44,&v,1));
    assert(!service.Call(second.value,1,&v,1) && !service.HookAcquire(second.value));
    const auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
    while (!service.Collect() && std::chrono::steady_clock::now()<deadline) std::this_thread::sleep_for(std::chrono::milliseconds(1));
    assert(service.Collect());
    assert(allocations==frees);
    assert(service.SetDispatchSink(nullptr));
    auto next=prepare("new-owner::call"); assert(next && next.value>first.value);
    auto absent=service.HookAcquire(next.value); assert(!absent && absent.error.find("dispatch sink unavailable")!=std::string::npos);
    assert(service.Call(next.value,1,&v,1));assert(service.TargetRelease(next.value));assert(service.Collect());
    // Physical identity includes every verified module field, not just address.
    auto identity_a=prepare("identity-a"); assert(identity_a);
    f.device=8;f.freeze(); auto identity_b=prepare("identity-b");assert(identity_b && identity_b.value!=identity_a.value);
    resolved_address=reinterpret_cast<uintptr_t>(&s2bridge_fixture_other);
    auto address_b=prepare("address-b");assert(address_b && address_b.value!=identity_b.value);
    assert(service.Call(address_b.value,0,&v,1).value.bits==43);
    for (auto id : {identity_a.value,identity_b.value,address_b.value}) assert(service.TargetRelease(id));
    assert(service.Collect() && allocations==frees);
    resolved_address=reinterpret_cast<uintptr_t>(&s2bridge_fixture_pointer);
    Fixture pointer_image(resolved_address-0x1200); pointer_image.freeze();f.image=pointer_image.image;
    a["parameters"][0]["native"]="ptr";a["parameters"][0]["projection"]["id"]="string";
    a["returns"]["native"]="ptr";a["returns"]["projection"]["id"]="string";
    a["fingerprint"]="linux-x86_64-sysv:none:ptr(ptr)";
    S2FunctionValue pointer{};pointer.kind=static_cast<unsigned char>(s2bridge::ValueKind::Pointer);
    pointer.flags=static_cast<unsigned char>(s2bridge::PointerProjection::String);pointer.bits=73;
    auto unavailable=prepare("pointer-unavailable");assert(unavailable);
    assert(!service.Call(unavailable.value,0,&pointer,1,pointer));
    assert(service.TargetRelease(unavailable.value));assert(service.Collect());
    Codec codec;assert(service.SetPointerCodec(&codec));
    auto copied=prepare("pointer-call");assert(copied);
    auto projected=service.Call(copied.value,0,&pointer,1,pointer);assert(projected && projected.value.bits==73);
    codec.live=false;auto stale=service.Call(copied.value,0,&pointer,1,pointer);
    assert(!stale && stale.error=="stale host handle");codec.live=true;
    assert(service.TargetRelease(copied.value));assert(service.Collect());
    a["receiver"]="entity";a["parameters"]=json::array();a["fingerprint"]="linux-x86_64-sysv:entity:ptr()";
    auto member=prepare("entity-receiver");assert(member);
    assert(!service.Call(member.value,0,&pointer,1,pointer)); // string is not a receiver
    auto entity=pointer;entity.flags=static_cast<unsigned char>(s2bridge::PointerProjection::Entity);
    assert(service.Call(member.value,0,&entity,1,pointer));
    codec.live=false;assert(!service.Call(member.value,0,&entity,1,pointer));
    assert(service.TargetRelease(member.value));assert(service.Collect() && allocations==frees);
    std::cout << "PASS real CIF/shared physical stock hook/lazy calls/nested owner bypass/refcount/retirement\n";
    worker_join_regression();
    scalar_transport();
    busy_service_insertion();
    KHook::Shutdown();
}
}
#endif
int main() { stages();
#ifndef S2FN_VALIDATION_ONLY
    runtime();
#endif
}


#endif
