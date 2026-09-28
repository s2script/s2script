#ifdef S2BRIDGE_TARGET_FIXTURE
#include <thread>
#include <cstring>
static const void* escaped_input=nullptr;
static volatile int fixture_calls=0;
extern "C" __attribute__((visibility("default"),noinline)) int s2bridge_fixture_native(int n) {
    ++fixture_calls; return n+1;
}
extern "C" __attribute__((visibility("default"),noinline)) int s2bridge_fixture_other(int n) {
    ++fixture_calls; return n+2;
}
extern "C" __attribute__((visibility("default"),noinline)) void* s2bridge_fixture_pointer(void* p) {
    ++fixture_calls; escaped_input=p; return p;
}
extern "C" __attribute__((visibility("default"),noinline)) int s2bridge_fixture_call_count() { return fixture_calls; }
extern "C" __attribute__((visibility("default"),noinline)) void* s2bridge_fixture_bad_pointer(void*) { ++fixture_calls; return reinterpret_cast<void*>(1); }
extern "C" __attribute__((visibility("default"),noinline)) const void* s2bridge_fixture_last_pointer() { return escaped_input; }
extern "C" __attribute__((visibility("default"),noinline)) int s2bridge_fixture_length(const char* value) { ++fixture_calls; return std::strlen(value); }
extern "C" __attribute__((visibility("default"),noinline)) int s2bridge_fixture_join(int n) {
    int result=0;
    auto volatile target=&s2bridge_fixture_other;
    std::thread worker([&] { result=target(n); });
    worker.join();
    return result+1;
}
#else
#include "engine_function_bridge.h"
#include "sha256.h"
#include "../third_party/json.hpp"
#include <cassert>
#include <cstring>
#include <iostream>
#include <limits>
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
    // Ownership is a semantic contract, not a change to the machine fingerprint.
    a=abi(); a["parameters"][0]["native"]="ptr"; a["parameters"][0]["projection"]["id"]="string";
    a["parameters"][0]["ownership"]="callee-borrowed"; a["fingerprint"]="linux-x86_64-sysv:none:i32(ptr)";
    assert(s2bridge::Parse(target().dump(),a.dump(),a["fingerprint"]));
    a["parameters"][0].erase("ownership"); assert(!s2bridge::Parse(target().dump(),a.dump(),a["fingerprint"]));
    for (const auto owner : {"caller-borrowed", "garbage"}) {
        a["parameters"][0]["ownership"]=owner; assert(!s2bridge::Parse(target().dump(),a.dump(),a["fingerprint"]));
    }
    a["parameters"][0]["ownership"]="native-observed";
    a["parameters"][0]["mutable"]=json::array({"pre"}); assert(!s2bridge::Parse(target().dump(),a.dump(),a["fingerprint"]));
    a["parameters"][0]["ownership"]="callee-borrowed";
    a["returns"]["native"]="ptr"; a["returns"]["projection"]["id"]="entity";
    a["fingerprint"]="linux-x86_64-sysv:none:ptr(ptr)";
    assert(!s2bridge::Parse(target().dump(),a.dump(),a["fingerprint"]));
    a["parameters"][0]["ownership"]="callee-retained"; assert(s2bridge::Parse(target().dump(),a.dump(),a["fingerprint"]));
    std::cout << "PASS bridge normalization and real S1 staged resolution\n";
}

json instance_contract() {
    auto position=[](const char* name,const char* projection,const char* ownership) {
        json p={{"name",name},{"native","ptr"},{"projection",{{"id",projection},{"version",1}}},
            {"ownership",ownership},{"mutable",json::array()},{"nullable",false}};
        if(std::string(projection)=="borrowed-record") p["instance"]=0;
        return p;
    };
    json layout={{"extent",8},{"alignment",4},{"fields",json::array({
        {{"name","small"},{"offsetKey","small"},{"offset",0},{"storage","u16"},{"nullable",false},{"read",json::array({"pre","post"})},{"write",json::array({"pre"})}},
        {{"name","handle"},{"offsetKey","handle"},{"offset",4},{"storage","entity-handle32"},{"nullable",true},{"read",json::array({"pre","post"})},{"write",json::array()}}})}};
    auto ret=abi()["returns"];ret["name"]="";ret["mutable"]=json::array();ret["nullable"]=false;
    return {{"version",1},{"selectedDataHash",std::string(64,'a')},{"signature",{
        {"platform","linux-x86_64-sysv"},{"memberReceiver",true},{"receiver",position("self","borrowed-record","synchronous-record")},
        {"parameters",json::array({position("hidden","native-only","invocation-passthrough")})},{"returns",ret},
        {"fingerprint","linux-x86_64-sysv:entity:i32(ptr)"},{"stackCopyBytes",128},
        {"instances",json::array({{{"codecId","borrowed-record"},{"codecVersion",1},{"kind",nullptr},{"record",layout},{"layoutHash",s2::digest::sha256(layout.dump())}}})}}},
        {"policy",{{"id","generic.v2"},{"version",1},{"surfaces",json::array({"pre","post"})}}}};
}
void instance_normalization() {
    auto seal=[](json& j) {j.erase("contractHash");j["contractHash"]=s2::digest::sha256(j.dump());};
    auto good=instance_contract();seal(good);
    auto parsed=s2bridge::ParseInstance(target().dump(),good.dump());assert(parsed);
    assert(parsed.value.abi.member_receiver && parsed.value.instances.hidden.count(0));
    const auto& layout=parsed.value.instances.records.at(-1).layout;
    assert(layout.extent==8 && layout.fields[0].width==2 && layout.fields[1].width==4);
    // A trusted wire must never be admitted by the unchanged public-v2 parser.
    assert(!s2bridge::Parse(target().dump(),good["signature"].dump(),parsed.value.info.fingerprint));
    for(int mode=0;mode<10;++mode) {
        auto invalid=instance_contract();auto& signature=invalid["signature"];
        auto& instance=signature["instances"][0];auto& record=instance["record"];
        switch(mode) {
        case 0:record["fields"][0]["offset"]=1;break;
        case 1:record["fields"][0]["storage"]="ptr";break;
        case 2:record["fields"][0]["offset"]=4;break;
        case 3:record["extent"]=65537;break;
        case 4:record["fields"][0]["write"]=json::array({"post"});break;
        case 5:signature["memberReceiver"]=false;break;
        case 6:signature["receiver"]["ownership"]="inferred";break;
        case 7:signature["parameters"][0]["instance"]=0;break;
        case 8:instance["kind"]={{"id","unknown"},{"version",1}};break;
        case 9:signature["fingerprint"]="linux-x86_64-sysv:none:i32(ptr)";break;
        }
        instance["layoutHash"]=s2::digest::sha256(record.dump());seal(invalid);
        assert(!s2bridge::ParseInstance(target().dump(),invalid.dump()));
    }
    auto stale=good;stale["signature"]["instances"][0]["record"]["fields"][0]["write"]=json::array();seal(stale);
    assert(!s2bridge::ParseInstance(target().dump(),stale.dump()));
    auto readonly=instance_contract();readonly["signature"]["instances"][0]["record"]["fields"][0]["write"]=json::array();
    readonly["signature"]["instances"][0]["layoutHash"]=s2::digest::sha256(readonly["signature"]["instances"][0]["record"].dump());seal(readonly);
    auto narrow=s2bridge::ParseInstance(target().dump(),readonly.dump());assert(narrow && parsed.value.instances.compatible(narrow.value.instances));
    auto hidden=instance_contract();hidden["signature"]["receiver"]=hidden["signature"]["parameters"][0];seal(hidden);
    assert(s2bridge::ParseInstance(target().dump(),hidden.dump()));
    auto nullable=instance_contract();nullable["signature"]["receiver"]={{"name","self"},{"native","ptr"},{"projection",{{"id","entity?"},{"version",1}}},{"mutable",json::array()},{"nullable",false}};seal(nullable);
    assert(s2bridge::ParseInstance(target().dump(),nullable.dump()).value.instances.nullable_receiver);
    std::cout << "PASS trusted physical receiver/hidden/record normalization, u16 storage, hash/bounds/rights rejection and public mint denial\n";
}
void string_indirect_normalization() {
    auto seal=[](json& j) {j.erase("contractHash");j["contractHash"]=s2::digest::sha256(j.dump());};
    auto indirect=[]() {
        auto c=instance_contract();
        c["signature"]["parameters"].push_back({{"name","label"},{"native","ptr"},{"projection",{{"id","string-indirect"},{"version",1}}},
            {"ownership","native-observed"},{"mutable",json::array()},{"nullable",false}});
        c["signature"]["fingerprint"]="linux-x86_64-sysv:entity:i32(ptr,ptr)";
        return c;
    };
    auto good=indirect();seal(good);
    auto parsed=s2bridge::ParseInstance(target().dump(),good.dump());assert(parsed);
    const auto& p=parsed.value.copies[1];
    assert(p && p.indirect && p.kind==s2fn::copy::Kind::String && p.ownership==s2bridge::CopyOwnership::NativeObserved && !p.mutable_pre);
    assert(parsed.value.HasCopies() && !parsed.value.copies[0]);
    // A plain copied string at the same position is a different native contract.
    auto plain=indirect();auto& row=plain["signature"]["parameters"][1];row["projection"]["id"]="string";seal(plain);
    auto direct=s2bridge::ParseInstance(target().dump(),plain.dump());assert(direct && !direct.value.copies[1].indirect);
    assert(!parsed.value.CompatibleCopies(direct.value) && parsed.value.CompatibleCopies(parsed.value));
    for(int mode=0;mode<5;++mode) {
        auto bad=indirect();auto& q=bad["signature"]["parameters"][1];
        switch(mode) {
        case 0:q["mutable"]=json::array({"pre"});break;
        case 1:q["ownership"]="callee-borrowed";break;
        case 2:q.erase("ownership");break;
        case 3:q["native"]="u64";bad["signature"]["fingerprint"]="linux-x86_64-sysv:entity:i32(ptr,u64)";break;
        case 4:q["nullable"]=true;break;
        }
        seal(bad);assert(!s2bridge::ParseInstance(target().dump(),bad.dump()));
    }
    auto ret=indirect();ret["signature"]["returns"]={{"name",""},{"native","ptr"},{"projection",{{"id","string-indirect"},{"version",1}}},
        {"ownership","native-observed"},{"mutable",json::array()},{"nullable",false}};
    ret["signature"]["fingerprint"]="linux-x86_64-sysv:entity:ptr(ptr,ptr)";seal(ret);
    assert(!s2bridge::ParseInstance(target().dump(),ret.dump()));
    // The public-v2 grammar stays closed to the trusted projection.
    auto a=abi();a["parameters"][0]["native"]="ptr";a["parameters"][0]["projection"]["id"]="string-indirect";
    a["parameters"][0]["ownership"]="native-observed";a["fingerprint"]="linux-x86_64-sysv:none:i32(ptr)";
    assert(!s2bridge::Parse(target().dump(),a.dump(),a["fingerprint"]));
    std::cout << "PASS trusted string-indirect normalization, readonly/direction rejection and public denial\n";
}
// Member fixture: an owner object holding a services sub-object pointer at a
// schema-style offset, compared against a hidden native-only argument.
struct RelationServices { int32_t marker=7; };
struct RelationOwner { uint64_t header=0; int32_t health=100; RelationServices* services=nullptr; };
void hidden_relationship() {
    using s2fn::copy::Reader;
    Reader reader{[](void*,uintptr_t source,void* dest,size_t count)->size_t {std::memcpy(dest,reinterpret_cast<const void*>(source),count);return count;},nullptr,4096,true};
    RelationServices mine, other; RelationOwner owner; owner.services=&mine;
    const auto base=reinterpret_cast<uintptr_t>(&owner);const auto field=static_cast<uint32_t>(offsetof(RelationOwner,services));
    auto hidden=[](const RelationServices& s){return reinterpret_cast<uintptr_t>(&s);};
    auto r=s2bridge::ReferencesHidden(reader,base,field,hidden(mine));assert(r && r.value);
    r=s2bridge::ReferencesHidden(reader,base,field,hidden(other));assert(r && !r.value);
    // Wrong field offset reads some other word: not referenced.
    r=s2bridge::ReferencesHidden(reader,base,static_cast<uint32_t>(offsetof(RelationOwner,health)),hidden(mine));assert(r && !r.value);
    // Null hidden / null owner / null stored pointer are never "referenced".
    owner.services=nullptr;r=s2bridge::ReferencesHidden(reader,base,field,0);assert(r && !r.value);
    owner.services=&mine;r=s2bridge::ReferencesHidden(reader,0,field,hidden(mine));assert(r && !r.value);
    // Overflowing ranges never reach the reader; a hidden value is never dereferenced.
    static int reads=0;
    Reader counting{[](void*,uintptr_t source,void* dest,size_t count)->size_t {++reads;std::memcpy(dest,reinterpret_cast<const void*>(source),count);return count;},nullptr,4096,true};
    r=s2bridge::ReferencesHidden(counting,UINTPTR_MAX-4,0,hidden(mine));assert(r && !r.value && reads==0);
    r=s2bridge::ReferencesHidden(counting,UINTPTR_MAX-64,UINT32_MAX,hidden(mine));assert(r && !r.value && reads==0);
    r=s2bridge::ReferencesHidden(counting,base,field,0x10);assert(r && !r.value && reads==1); // 0x10 compared, not read
    // Unreadable word: denied read is false, not a fault.
    Reader denied{[](void*,uintptr_t,void*,size_t)->size_t {return 0;},nullptr,4096,true};
    r=s2bridge::ReferencesHidden(denied,base,field,hidden(mine));assert(r && !r.value);
    Reader shortread{[](void*,uintptr_t,void*,size_t count)->size_t {return count>1 ? count-1 : 0;},nullptr,4096,true};
    r=s2bridge::ReferencesHidden(shortread,base,field,hidden(mine));assert(r && !r.value);
    // A word straddling a (fake, tiny) page boundary is assembled from page-bounded reads.
    alignas(16) uint8_t storage[32]{};const auto target=hidden(mine);std::memcpy(storage+4,&target,sizeof target);
    const auto at=reinterpret_cast<uintptr_t>(storage);reads=0;
    Reader paged{[](void*,uintptr_t source,void* dest,size_t count)->size_t {++reads;assert(source/8==(source+count-1)/8);std::memcpy(dest,reinterpret_cast<const void*>(source),count);return count;},nullptr,8,true};
    r=s2bridge::ReferencesHidden(paged,at,4,target);assert(r && r.value && reads==2);
    // Missing reader is a named error, not a silent false.
    Reader none{};r=s2bridge::ReferencesHidden(none,base,field,hidden(mine));assert(!r && r.error.find("reader unavailable")!=std::string::npos);
    std::cout << "PASS hidden relationship check via checked reader: match, mismatch, null, overflow, denied, straddle and unavailable\n";
}

void copied_transport_primitives() {
    using namespace s2bridge; using namespace s2fn::copy;
    CopyProducer engine, plugin; plugin.domain=1;plugin.digest[0]=23;plugin.generation=7;
    auto context=CopyTransaction::Create(engine); assert(context);
    auto& tx=static_cast<CopyTransaction&>(*context.value);
    S2FunctionValue value{};value.kind=8;value.flags=4;value.aux=3;
    const uint8_t bytes[]={'a','b','c'};CopyInput input{1,sizeof(CopyInput),bytes,3};
    assert(tx.Stage(0,Kind::String,value,input,plugin));
    assert(tx.Read(0).size()==3 && std::memcmp(tx.Read(0).data(),"abc",3)==0);
    auto saved=tx.Read(0);
    auto bad=value;bad.bits=UINT64_MAX;assert(!tx.Stage(0,Kind::String,bad,input,plugin));
    bad=value;bad.bits=2;assert(!tx.Stage(0,Kind::String,bad,input,plugin));
    bad=value;bad.reserved=1;assert(!tx.Stage(0,Kind::String,bad,input,plugin));
    bad=value;bad.flags=5;assert(!tx.Stage(0,Kind::String,bad,input,plugin));
    CopyInput malformed=input;malformed.struct_size=0;assert(!tx.Stage(0,Kind::String,value,malformed,plugin));
    malformed=input;malformed.size=UINT64_MAX;assert(!tx.Stage(0,Kind::String,value,malformed,plugin));
    const uint8_t nul[]={0,'x'};malformed={1,sizeof(CopyInput),nul,2};bad=value;bad.aux=2;
    assert(!tx.Stage(0,Kind::String,bad,malformed,plugin));
    const uint8_t utf8[]={0xc0,0x80};malformed.data=utf8;assert(!tx.Stage(0,Kind::String,bad,malformed,plugin));
    auto invalid_owner=plugin;invalid_owner.domain=17;assert(!tx.Stage(0,Kind::String,value,input,invalid_owner));
    invalid_owner=plugin;invalid_owner.reserved=1;assert(!tx.Stage(0,Kind::String,value,input,invalid_owner));
    invalid_owner=plugin;invalid_owner.version=2;assert(!tx.Stage(0,Kind::String,value,input,invalid_owner));
    invalid_owner=plugin;invalid_owner.struct_size=0;assert(!tx.Stage(0,Kind::String,value,input,invalid_owner));
    assert(saved.data()==tx.Read(0).data());
    value.aux=0;assert(tx.Stage(0,Kind::String,value,{},plugin));
    assert(tx.Read(0).size()==0 && tx.Read(0).data()[0]==0 && saved.size()==3);
    uint8_t output[16];std::memset(output,0x7f,sizeof output);CopyOutput out{1,sizeof(CopyOutput),output,2,9};
    S2FunctionValue request{};request.kind=8;request.flags=4;
    assert(!EncodeCopy(saved,request,out) && out.size==9 && output[0]==0x7f);
    out.capacity=16;auto encoded=EncodeCopy(saved,request,out);assert(encoded && encoded.value.bits==0 && encoded.value.aux==3 && out.size==3 && output[3]==0x7f);
    float vector[3]={-0.0f,2,3};value.flags=5;value.aux=12;input={1,sizeof(CopyInput),reinterpret_cast<const uint8_t*>(vector),12};
    assert(tx.Stage(1,Kind::Vector,value,input,plugin));
    assert(std::memcmp(tx.Read(1).data(),vector,12)==0);
    value.aux=11;assert(!tx.Stage(1,Kind::Vector,value,input,plugin));
    vector[2]=std::numeric_limits<float>::infinity();value.aux=12;assert(!tx.Stage(1,Kind::Vector,value,input,plugin));
    // Native call capture capacity must be reserved before the invocation.
    auto capture=s2fn::copy::Snapshot::PrepareCapture(tx.EngineOperation(),Kind::String);assert(capture);
    auto reserved=s2fn::copy::NativeBudget().Read();
    Reader reader{[](void*,uintptr_t source,void* dest,size_t count)->size_t { std::memcpy(dest,reinterpret_cast<const void*>(source),count);return count; },nullptr,4096,true};
    std::array<uint8_t,65536> native{};native[0]='z';
    auto captured=s2fn::copy::Snapshot::CapturePrepared(std::move(capture.value),reinterpret_cast<uintptr_t>(native.data()),reader);assert(captured && captured.value.size()==1);
    assert(s2fn::copy::NativeBudget().Read().bytes==reserved.bytes);
    assert(tx.Capture(2,Kind::String,reinterpret_cast<uintptr_t>(native.data()),reader));
    auto initial=tx.Read(2);native[0]='q';
    assert(initial.data()[0]=='z' && tx.Read(2).data()[0]=='z');
    auto immutable=captured.value;
    assert(!s2fn::copy::Snapshot::CapturePrepared(std::move(captured.value),reinterpret_cast<uintptr_t>(native.data()),reader));
    std::array<CopyPosition,32> positions{};positions[0]={Kind::String,CopyOwnership::CalleeRetained,true};positions[1]={Kind::Vector,CopyOwnership::CalleeBorrowed,true};
    auto before=Arena::Resident().Read();auto published=tx.Publish(positions,2,false);assert(published);
    assert(static_cast<const char*>(published.value[0])[0]==0);
    assert(std::memcmp(published.value[1],tx.Read(1).data(),12)==0);
    const auto* escaped=published.value[0];context.value.reset();
    std::vector<std::string> churn(500,std::string(4096,'x'));assert(static_cast<const char*>(escaped)[0]==0);
    assert(Arena::Resident().Read().values==before.values+1);
    std::cout << "PASS copied span validation, immutable staging, transactional output, borrowed/permanent publication\n";
}
void copied_quota_transaction() {
    using namespace s2bridge;using namespace s2fn::copy;
    CopyProducer engine,full,fresh,loser;full.domain=1;full.digest[0]=31;fresh.domain=2;fresh.digest[0]=32;loser.domain=1;loser.digest[0]=33;
    std::array<CopyPosition,32> positions{};positions[0]={Kind::String,CopyOwnership::CalleeRetained,true};positions[1]=positions[0];
    std::string content(65535,'x');S2FunctionValue value{};value.kind=8;value.flags=4;value.aux=65535;
    CopyInput input{1,sizeof(CopyInput),reinterpret_cast<const uint8_t*>(content.data()),content.size()};
    for(int i=0;i<64;++i) {
        content[0]='A'+i/26;content[1]='a'+i%26;
        auto frame=CopyTransaction::Create(engine);assert(frame);auto& tx=static_cast<CopyTransaction&>(*frame.value);
        assert(tx.Stage(0,Kind::String,value,input,full));assert(tx.Publish(positions,1,false));
    }
    const auto before=Arena::Resident().Read();assert(before.bytes==4*MiB && before.values==64 && before.owners==1);
    auto frame=CopyTransaction::Create(engine);assert(frame);auto& tx=static_cast<CopyTransaction&>(*frame.value);
    S2FunctionValue tiny=value;tiny.aux=3;const uint8_t small[]="new";CopyInput small_input{1,sizeof(CopyInput),small,3};
    assert(tx.Stage(0,Kind::String,tiny,small_input,loser));
    assert(tx.Stage(0,Kind::String,tiny,small_input,fresh)); // Losing owner never reaches permanent admission.
    content[0]='Z';assert(tx.Stage(1,Kind::String,value,input,full));
    auto failed=tx.Publish(positions,2,false);assert(!failed && failed.error.find("FunctionCopyPermanentBudgetExceeded")!=std::string::npos);
    auto after=Arena::Resident().Read();assert(after.bytes==before.bytes && after.values==before.values && after.owners==before.owners);
    content[0]='A';content[1]='a';++full.generation;
    assert(tx.Stage(1,Kind::String,value,input,full));auto committed=tx.Publish(positions,2,false);assert(committed);
    assert(std::memcmp(committed.value[0],"new",3)==0 && std::memcmp(committed.value[1],"Aa",2)==0);
    after=Arena::Resident().Read();assert(after.bytes==before.bytes+16 && after.values==65 && after.owners==2);
    const char* escaped=static_cast<const char*>(committed.value[1]);frame.value.reset();
    auto next=CopyTransaction::Create(engine);assert(next);std::vector<std::string> churn(500,std::string(4096,'p'));
    assert(std::memcmp(escaped,"Aa",2)==0 && Arena::Resident().Read().bytes==after.bytes);
    std::cout<<"PASS production-quota mixed-producer transaction rollback, losing candidate no-charge, generation reuse and escaped lifetime\n";
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
extern "C" const void* s2bridge_fixture_last_pointer();
extern "C" int s2bridge_fixture_call_count();
extern "C" void* s2bridge_fixture_bad_pointer(void*);
extern "C" int s2bridge_fixture_length(const char*);
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
static bool scalar_suppressed=true;
static int scalar_dispatch(long long target_id,const S2FunctionFrameInfo* info,int phase) {
    assert(info && info->version==1 && info->struct_size==48 && info->invocation_id);
    const char* fp="linux-x86_64-sysv:none:i32(i32)";char why[256]{};S2FunctionValue value{};
    assert(S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,0,2,&value,why,sizeof why));
    const auto old=value;
    assert(!S2_FunctionFrameRead(target_id,info->frame_token+1,info->native_epoch,fp,0,2,&value,why,sizeof why));
    assert(value.kind==old.kind && value.bits==old.bits);
    assert(!S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,"wrong",0,2,&value,why,sizeof why));
    S2FunctionValue effect{}; effect.kind=2; effect.bits=91;
    S2FunctionValue effective{};
    auto override_return=[&](const S2FunctionValue& v) {
        return S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,fp,&v,&effective,why,sizeof why);
    };
    if(phase==0){
        assert(!override_return(effect));
        assert(!S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,-3,2,&value,why,sizeof why));
        transport_invocations.push_back(info->invocation_id);
        assert(!S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,-2,2,&value,why,sizeof why));
        if(!scalar_suppressed) {
            assert(S2_FunctionFrameCommit(target_id,info->frame_token,info->native_epoch,fp,0,nullptr,why,sizeof why));
            last_frame=*info;return 1;
        }
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
        assert(info->flags==unsigned(scalar_suppressed));
        if(scalar_suppressed) {
            assert(!S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,-3,2,&value,why,sizeof why));
            assert(std::string(why).find("original return unavailable")!=std::string::npos);
        } else assert(S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,-3,2,&value,why,sizeof why) && value.bits==8);
        auto bad=effect;bad.flags=1;assert(!override_return(bad));
        bad=effect;bad.reserved=1;assert(!override_return(bad));
        bad=effect;bad.aux=1;assert(!override_return(bad));
        bad=effect;bad.kind=8;assert(!override_return(bad));
        bad=effect;bad.bits=UINT64_MAX;assert(!override_return(bad));
        assert(!S2_FunctionFrameOverrideReturn(target_id+1,info->frame_token,info->native_epoch,fp,&effect,&effective,why,sizeof why));
        assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token+1,info->native_epoch,fp,&effect,&effective,why,sizeof why));
        assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch+1,fp,&effect,&effective,why,sizeof why));
        assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,"wrong",&effect,&effective,why,sizeof why));
        std::thread worker([&]{assert(!override_return(effect));});worker.join();
        assert(effective.kind==0 && effective.bits==0); // Every failed effect leaves output untouched.
        assert(S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,-2,2,&value,why,sizeof why) && value.bits==(scalar_suppressed ? 73u : 8u));
        const auto expected=scalar_suppressed ? 73u : 91u;
        assert(override_return(effect) && effective.bits==expected); // Earlier Supersede wins.
        effect.bits=92;assert(override_return(effect) && effective.bits==expected);
        assert(S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,-2,2,&value,why,sizeof why) && value.bits==expected);
        if(!scalar_suppressed)assert(S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,-3,2,&value,why,sizeof why) && value.bits==8);
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
static void entity_codec() {
    int object=7,contacts=0;uint32_t serial=71;bool live=true;
    s2bridge::EntityPointerCodec codec({
        [&](uint32_t index,uint32_t wanted)->void* {++contacts;return live && index==901 && wanted==serial ? &object : nullptr;},
        [&](const void* candidate,s2bridge::EntityIdentity& out) {++contacts;if(!live || candidate!=&object)return false;out={901,serial};return true;}
    });
    s2bridge::CallStorage storage;S2FunctionValue strict{};strict.kind=8;strict.flags=1;
    auto nullable=strict;nullable.flags=2;
    auto identity=strict;identity.aux=901;identity.bits=71;
    auto decoded=codec.Decode(identity,storage);assert(decoded && decoded.value.Get<void*>()==&object);
    auto encoded=codec.Encode(decoded.value,strict);assert(encoded && encoded.value.aux==901 && encoded.value.bits==71);
    auto null=nullable;null.aux=UINT32_MAX;
    assert(codec.Decode(null,storage) && codec.Decode(null,storage).value.Get<void*>()==nullptr);
    // Even an inaccessible candidate is compared, never dereferenced.
    auto unknown=s2fn::NativeValue::From(reinterpret_cast<void*>(uintptr_t(1)));
    assert(!codec.Encode(unknown,strict));auto optional=codec.Encode(unknown,nullable);assert(optional && optional.value.aux==UINT32_MAX && !optional.value.bits);
    const int before=contacts;
    for(int field=0;field<6;++field) {
        auto bad=identity;
        if(field==0)bad.kind=2;if(field==1)bad.flags=3;if(field==2)bad.reserved=1;
        if(field==3)bad.bits=UINT64_MAX;if(field==4)bad.aux=UINT32_MAX-1;if(field==5)bad.aux=UINT32_MAX;
        assert(!codec.Decode(bad,storage));
    }
    for(int field=0;field<5;++field) {
        auto bad=strict;
        if(field==0)bad.kind=2;if(field==1)bad.flags=0;if(field==2)bad.reserved=1;if(field==3)bad.aux=901;if(field==4)bad.bits=71;
        assert(!codec.Encode(decoded.value,bad));
    }
    assert(contacts==before);
    std::thread worker([&]{s2bridge::CallStorage worker_storage;assert(!codec.Decode(identity,worker_storage));assert(!codec.Encode(decoded.value,strict));});worker.join();assert(contacts==before);
    live=false;assert(!codec.Decode(identity,storage));identity.flags=2;assert(codec.Decode(identity,storage).value.Get<void*>()==nullptr);
    live=true;serial=72;identity.flags=1;assert(!codec.Decode(identity,storage));identity.bits=72;assert(codec.Decode(identity,storage));
    assert(codec.Encode(decoded.value,strict).value.bits==72);
    std::cout<<"PASS entity codec canonical identity, unknown-address non-dereference, null/stale/reuse and pre-contact owner-thread refusal\n";
}
static bool entity_second_live=true;
static int entity_mode=0,entity_contacts=0;
static S2FunctionFrameInfo entity_last_frame{};
static S2FunctionValue entity_request(unsigned char flags) {S2FunctionValue v{};v.kind=8;v.flags=flags;return v;}
static int entity_dispatch(long long target_id,const S2FunctionFrameInfo* info,int phase) {
    const char* fp="linux-x86_64-sysv:none:ptr(ptr)";char why[256]{};
    auto read=[&](int selector,S2FunctionValue& out){return S2_FunctionFrameRead(target_id,info->frame_token,info->native_epoch,fp,selector,8,&out,why,sizeof why);};
    if(phase==0) {
        auto strict=entity_request(1),nullable=entity_request(2);
        assert(read(0,strict) && strict.aux==901);assert(read(0,nullable) && nullable.aux==901);
        const int before=entity_contacts;
        auto bad=entity_request(1);bad.bits=1;assert(!read(0,bad));assert(entity_contacts==before);
        std::thread worker([&]{auto request=entity_request(1);assert(!read(0,request));});worker.join();assert(entity_contacts==before);
        auto edit=entity_request(entity_mode==0 ? 2 : 1);edit.aux=entity_mode==0 ? UINT32_MAX : 902;edit.bits=entity_mode==0 ? 0 : 72;
        assert(S2_FunctionFrameWrite(target_id,info->frame_token,info->native_epoch,fp,0,&edit,why,sizeof why));
        if(entity_mode==1)entity_second_live=false;
        strict=entity_request(1);nullable=entity_request(2);
        assert(!read(0,strict));assert(std::strstr(why,"strict entity"));
        assert(read(0,nullable) && nullable.aux==UINT32_MAX);
        const auto committed=S2_FunctionFrameCommit(target_id,info->frame_token,info->native_epoch,fp,0,nullptr,why,sizeof why);
        if(entity_mode==0)assert(committed);else {assert(!committed);assert(std::strstr(why,"strict entity"));}
    } else {
        auto value=entity_request(2);assert(read(-2,value));assert(value.aux==(entity_mode==0 ? UINT32_MAX : 901));
        assert(!S2_FunctionFrameWrite(target_id,info->frame_token,info->native_epoch,fp,0,&value,why,sizeof why));
        const auto original=value;auto snapshot=entity_request(2);assert(read(-3,snapshot) && snapshot.aux==original.aux);
        auto effect=entity_request(2);effect.aux=UINT32_MAX;
        auto out=entity_request(1);
        const int contacts=entity_contacts;
        auto bad=out;bad.flags=3;assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,fp,&effect,&bad,why,sizeof why));assert(entity_contacts==contacts);
        bad=out;bad.reserved=1;assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,fp,&effect,&bad,why,sizeof why));assert(entity_contacts==contacts);
        bad=out;bad.bits=1;assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,fp,&effect,&bad,why,sizeof why));assert(entity_contacts==contacts);
        bad=effect;bad.flags=0;assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,fp,&bad,&out,why,sizeof why));
        bad=effect;bad.reserved=1;assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,fp,&bad,&out,why,sizeof why));
        bad=effect;bad.kind=2;assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,fp,&bad,&out,why,sizeof why));
        if(entity_mode==0) {
            // Null was valid input but cannot satisfy this strict output request.
            assert(!S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,fp,&effect,&out,why,sizeof why));
            assert(std::strstr(why,"POST override submitted: readback failed"));
            assert(out.kind==8 && out.flags==1 && out.aux==0 && out.bits==0);
            auto current=entity_request(2);assert(read(-2,current) && current.aux==UINT32_MAX);
            effect=entity_request(1);effect.aux=901;effect.bits=71;out=entity_request(2);
            assert(S2_FunctionFrameOverrideReturn(target_id,info->frame_token,info->native_epoch,fp,&effect,&out,why,sizeof why) && out.aux==UINT32_MAX);
        }
        snapshot=entity_request(2);assert(read(-3,snapshot) && snapshot.aux==original.aux);
    }
    entity_last_frame=*info;return 1;
}
static void entity_transport() {
    for(bool nullable_first:{false,true}) {
        int objects[2]={};entity_second_live=true;
        s2bridge::EntityPointerCodec codec({
            [&](uint32_t index,uint32_t serial)->void* {++entity_contacts;if(index==901 && serial==71)return &objects[0];if(index==902 && serial==72 && entity_second_live)return &objects[1];return nullptr;},
            [&](const void* pointer,s2bridge::EntityIdentity& out) {++entity_contacts;for(unsigned i=0;i<2;++i)if(pointer==&objects[i] && (i==0 || entity_second_live)){out={901+i,71+i};return true;}return false;}
        });
        auto address=reinterpret_cast<uintptr_t>(&s2bridge_fixture_pointer);Fixture fixture(address-0x1200);fixture.freeze();
        s2bridge::Service service([&](const auto&,auto& out,auto&){out.address=address;out.image=fixture.image;return true;});
        s2bridge::CoreDispatchSink sink(entity_dispatch);assert(service.SetDispatchSink(&sink));assert(service.SetPointerCodec(&codec));
        auto t=target();t["resolve"]="direct";t["derivation"]="identity";t["candidateValidate"]=json::object();auto a=abi();
        a["parameters"][0]["native"]="ptr";a["returns"]["native"]="ptr";a["fingerprint"]="linux-x86_64-sysv:none:ptr(ptr)";
        auto prepare=[&](bool nullable){a["parameters"][0]["projection"]["id"]=nullable ? "entity?" : "entity";a["returns"]["projection"]["id"]=nullable ? "entity?" : "entity";return service.Prepare(nullable ? "optional" : "required",t.dump(),a.dump(),a["fingerprint"]);};
        auto first=prepare(nullable_first),second=prepare(!nullable_first);assert(first && second && first.value==second.value);
        auto input=entity_request(1);input.aux=901;input.bits=71;
        for(unsigned char flags:{1,2}) {auto arg=input;arg.flags=flags;auto result=service.Call(first.value,0,&arg,1,entity_request(flags));assert(result && result.value.flags==flags && result.value.aux==901);}
        auto null=entity_request(2);null.aux=UINT32_MAX;assert(service.Call(first.value,0,&null,1,entity_request(2)));
        assert(!service.Call(first.value,0,&null,1,entity_request(1)));
        assert(service.HookAcquire(first.value));
        for(entity_mode=0;entity_mode<2;++entity_mode) {
            auto result=service.Call(first.value,0,&input,1,entity_request(2));assert(result && result.value.aux==(entity_mode==0 ? UINT32_MAX : 901));
        }
        const int before=entity_contacts;auto request=entity_request(1);char why[256]{};
        assert(!S2_FunctionFrameRead(first.value,entity_last_frame.frame_token,entity_last_frame.native_epoch,a["fingerprint"].get<std::string>().c_str(),0,8,&request,why,sizeof why));assert(entity_contacts==before);
        assert(service.HookRelease(first.value));assert(service.TargetRelease(first.value));assert(service.TargetRelease(second.value));
        auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(3);
        while(!service.Collect() && std::chrono::steady_clock::now()<deadline)std::this_thread::sleep_for(std::chrono::milliseconds(1));assert(service.Empty());
    }
    std::cout<<"PASS entity binding-local requests both physical orders, staged null/strict reads, atomic stale commit, phase/lease/thread guards\n";
}
static s2bridge::CopyProducer copy_plugin() {s2bridge::CopyProducer p;p.domain=1;p.digest[0]=91;p.generation=4;return p;}
static int copy_mode=0, copy_pre=0, copy_post=0;
static int copied_dispatch(long long id,const S2FunctionFrameInfo* info,int phase) {
    using namespace s2bridge;
    CopyFrameKey key{id,info->frame_token,info->native_epoch,"linux-x86_64-sysv:none:ptr(ptr)"};
    auto request=entity_request(4);std::array<uint8_t,65535> bytes{};CopyOutput out{1,sizeof(CopyOutput),bytes.data(),bytes.size(),0};
    auto read=FrameReadCopy(key,phase ? -2 : 0,request,out);assert(read && out.size>0);
    auto saved=std::string(reinterpret_cast<char*>(bytes.data()),out.size);
    if(!phase) {
        ++copy_pre;assert(saved=="input");
        auto bad=key;++bad.token;assert(!FrameReadCopy(bad,0,request,out));
        S2FunctionValue edit=request;edit.aux=6;const uint8_t replacement[]="edited";CopyInput in{1,sizeof(CopyInput),replacement,6};
        auto before=s2fn::copy::Arena::Resident().Read();
        assert(FrameWriteCopy(key,0,edit,in,copy_plugin()));
        assert(FrameReadCopy(key,0,request,out) && out.size==6 && std::memcmp(bytes.data(),"edited",6)==0);
        assert(!FrameCommitCopy(key,2,nullptr,{},copy_plugin()));
        assert(s2fn::copy::Arena::Resident().Read().values==before.values);
        const uint8_t result[]="suppressed";CopyInput result_in{1,sizeof(CopyInput),result,10};auto returned=request;returned.aux=10;
        assert(FrameCommitCopy(key,copy_mode==2 ? 2 : 0,copy_mode==2 ? &returned : nullptr,result_in,copy_plugin()));
        assert(saved=="input");
    } else {
        ++copy_post;assert(saved==(copy_mode==2 ? "suppressed" : "edited"));
        auto original=FrameReadCopy(key,-3,request,out);assert(bool(original)==(copy_mode!=2));
        assert(!FrameWriteCopy(key,0,request,{},copy_plugin()));
        const uint8_t replacement[]="post";CopyInput in{1,sizeof(CopyInput),replacement,4};auto edit=request;edit.aux=4;
        out.capacity=3;out.size=71;bytes[0]=99;auto before=s2fn::copy::Arena::Resident().Read();
        assert(!FrameOverrideReturnCopy(key,edit,in,copy_plugin(),request,out));
        assert(out.size==71 && bytes[0]==99 && s2fn::copy::Arena::Resident().Read().values==before.values);
        out.capacity=bytes.size();assert(FrameOverrideReturnCopy(key,edit,in,copy_plugin(),request,out));
        assert(std::string(reinterpret_cast<char*>(bytes.data()),out.size)==(copy_mode==2 ? "suppressed" : "post"));
        if(copy_mode==2) assert(s2fn::copy::Arena::Resident().Read().values==before.values);
        if(copy_mode!=2) {assert(FrameReadCopy(key,-3,request,out));assert(std::string(reinterpret_cast<char*>(bytes.data()),out.size)=="edited");}
    }
    return 1;
}
static int observed_copy_dispatch(long long id,const S2FunctionFrameInfo* info,int phase) {
    using namespace s2bridge;CopyFrameKey key{id,info->frame_token,info->native_epoch,"linux-x86_64-sysv:none:ptr(ptr)"};
    auto request=entity_request(4);uint8_t bytes[16]{};CopyOutput out{1,sizeof(CopyOutput),bytes,16,0};
    assert(FrameReadCopy(key,phase ? -2 : 0,request,out) && out.size==5 && std::memcmp(bytes,"input",5)==0);
    auto value=request;value.aux=1;const uint8_t replacement[]="x";CopyInput in{1,sizeof(CopyInput),replacement,1};
    char reason[256]{};auto entity=entity_request(1);
    // Both copied flags and a misleading entity request are rejected by the old
    // transport before codec contact on a physically copied position.
    assert(!S2_FunctionFrameRead(id,key.token,key.epoch,key.fingerprint,0,8,&entity,reason,sizeof reason));
    assert(!S2_FunctionFrameWrite(id,key.token,key.epoch,key.fingerprint,0,&entity,reason,sizeof reason));
    assert(!S2_FunctionFrameCommit(id,key.token,key.epoch,key.fingerprint,0,nullptr,reason,sizeof reason));
    assert(!FrameWriteCopy(key,0,value,in,copy_plugin()));
    if(!phase) {
        assert(!FrameCommitCopy(key,2,&value,in,copy_plugin()));
        assert(FrameCommitCopy(key,0,nullptr,{},copy_plugin()));
    } else {
        assert(!FrameOverrideReturnCopy(key,value,in,copy_plugin(),request,out));
        assert(!S2_FunctionFrameOverrideReturn(id,key.token,key.epoch,key.fingerprint,&entity,&entity,reason,sizeof reason));
    }
    return 1;
}
static void copied_execution() {
    using namespace s2bridge;
    auto address=reinterpret_cast<uintptr_t>(&s2bridge_fixture_pointer);Fixture fixture(address-0x1200);fixture.freeze();
    Service service([&](const auto&,auto& out,auto&){out.address=address;out.image=fixture.image;return true;});
    CoreDispatchSink sink(copied_dispatch);assert(service.SetDispatchSink(&sink));
    auto t=target();t["resolve"]="direct";t["derivation"]="identity";t["candidateValidate"]=json::object();auto a=abi();
    a["parameters"][0]["native"]="ptr";a["parameters"][0]["projection"]["id"]="string";a["parameters"][0]["ownership"]="callee-borrowed";
    a["returns"]["native"]="ptr";a["returns"]["projection"]["id"]="string";a["returns"]["ownership"]="caller-borrowed";
    a["fingerprint"]="linux-x86_64-sysv:none:ptr(ptr)";
    auto prepared=service.Prepare("copy-alias",t.dump(),a.dump(),a["fingerprint"]);assert(prepared);
    const uint8_t input[]="input";CopyInput in{1,sizeof(CopyInput),input,5};std::array<uint8_t,65535> bytes{};
    CopyOutput out{1,sizeof(CopyOutput),bytes.data(),bytes.size(),0};auto arg=entity_request(4);arg.aux=5;auto request=entity_request(4);
    assert(!service.Call(prepared.value,0,&arg,1,request));
    char reason[256]{};assert(!S2_FunctionPrepare("copied-public",t.dump().c_str(),a.dump().c_str(),"linux-x86_64-sysv:none:ptr(ptr)",reason,sizeof reason));
    // The real public export reaches ordinary service/resolver admission now.
    assert(reason[0] && !std::strstr(reason,"FunctionCopyExecutionUnavailable"));
    const auto calls_before=s2bridge_fixture_call_count();auto metrics=s2fn::copy::Arena::Resident().Read();
    out.capacity=65534;out.size=99;bytes[0]=71;
    assert(!service.CallCopy(prepared.value,0,&arg,1,request,in,out,copy_plugin()));
    assert(s2bridge_fixture_call_count()==calls_before && out.size==99 && bytes[0]==71 && s2fn::copy::Arena::Resident().Read().values==metrics.values);
    out.capacity=bytes.size();auto bad=arg;bad.bits=UINT64_MAX;
    assert(!service.CallCopy(prepared.value,0,&bad,1,request,in,out,copy_plugin()) && s2bridge_fixture_call_count()==calls_before);
    auto result=service.CallCopy(prepared.value,0,&arg,1,request,in,out,copy_plugin());
    assert(result && out.size==5 && std::memcmp(bytes.data(),"input",5)==0);
    auto empty=arg;empty.aux=0;assert(service.CallCopy(prepared.value,0,&empty,1,request,{},out,copy_plugin()) && out.size==0);
    auto conflict=a;conflict["parameters"][0]["ownership"]="callee-retained";
    assert(!service.Prepare("copy-conflict",t.dump(),conflict.dump(),a["fingerprint"]));
    assert(service.TargetRelease(prepared.value));assert(service.Collect());
    {
        auto vector_abi=a;vector_abi["parameters"][0]["projection"]["id"]="vector";vector_abi["returns"]["projection"]["id"]="vector";
        auto vector=service.Prepare("vector-alias",t.dump(),vector_abi.dump(),vector_abi["fingerprint"]);assert(vector);
        float native_vector[3]={-0.0f,1.25f,-2};CopyInput vector_input{1,sizeof(CopyInput),reinterpret_cast<const uint8_t*>(native_vector),12};
        auto vector_arg=entity_request(5);vector_arg.aux=12;auto vector_request=entity_request(5);out.capacity=12;
        auto result=service.CallCopy(vector.value,0,&vector_arg,1,vector_request,vector_input,out,copy_plugin());assert(result && out.size==12 && std::memcmp(bytes.data(),native_vector,12)==0);
        auto before=s2bridge_fixture_call_count();vector_arg.aux=11;assert(!service.CallCopy(vector.value,0,&vector_arg,1,vector_request,vector_input,out,copy_plugin()) && s2bridge_fixture_call_count()==before);
        vector_arg.aux=12;native_vector[2]=std::numeric_limits<float>::infinity();assert(!service.CallCopy(vector.value,0,&vector_arg,1,vector_request,vector_input,out,copy_plugin()) && s2bridge_fixture_call_count()==before);
        assert(service.TargetRelease(vector.value));assert(service.Collect());out.capacity=bytes.size();
    }
    {
        const auto bad_address=reinterpret_cast<uintptr_t>(&s2bridge_fixture_bad_pointer);Fixture image(bad_address-0x1200);image.freeze();
        Service bad_service([&](const auto&,auto& out,auto&){out.address=bad_address;out.image=image.image;return true;});
        auto bad=bad_service.Prepare("bad-native-copy",t.dump(),a.dump(),a["fingerprint"]);assert(bad);
        const auto count=s2bridge_fixture_call_count();out.size=91;bytes[0]=72;
        auto failure=bad_service.CallCopy(bad.value,0,&arg,1,request,in,out,copy_plugin());
        assert(!failure && failure.error.find("FunctionCopyCaptureAfterCall")!=std::string::npos && s2bridge_fixture_call_count()==count+1);
        assert(out.size==91 && bytes[0]==72);assert(bad_service.TargetRelease(bad.value));assert(bad_service.Collect());
    }
    {
        CoreDispatchSink observer(observed_copy_dispatch);assert(service.SetDispatchSink(&observer));
        auto observed=a;observed["parameters"][0]["ownership"]="native-observed";observed["returns"]["ownership"]="native-observed";
        auto binding=service.Prepare("observed-copy",t.dump(),observed.dump(),observed["fingerprint"]);assert(binding);
        const auto count=s2bridge_fixture_call_count();assert(!service.CallCopy(binding.value,0,&arg,1,request,in,out,copy_plugin()) && s2bridge_fixture_call_count()==count);
        assert(service.HookAcquire(binding.value));auto volatile target=&s2bridge_fixture_pointer;
        assert(target(const_cast<uint8_t*>(input))==input);
        assert(service.HookRelease(binding.value));assert(service.TargetRelease(binding.value));while(!service.Collect()) std::this_thread::sleep_for(std::chrono::milliseconds(1));
        assert(service.SetDispatchSink(&sink));
    }
    a=conflict;a["parameters"][0]["mutable"]=json::array({"pre"});
    for(bool mutable_first:{false,true}) {
        auto one=a;one["parameters"][0]["mutable"]=mutable_first ? json::array({"pre"}) : json::array();
        auto two=a;two["parameters"][0]["mutable"]=mutable_first ? json::array() : json::array({"pre"});
        auto first=service.Prepare("first-copy",t.dump(),one.dump(),one["fingerprint"]);
        auto second=service.Prepare("second-copy",t.dump(),two.dump(),two["fingerprint"]);
        assert(first && second && first.value==second.value);
        assert(service.TargetRelease(first.value));assert(service.TargetRelease(second.value));assert(service.Collect());
    }
    prepared=service.Prepare("copy-mutation",t.dump(),a.dump(),a["fingerprint"]);assert(prepared);assert(service.HookAcquire(prepared.value));
    for(copy_mode=1;copy_mode<=2;++copy_mode) {
        auto result=service.CallCopy(prepared.value,0,&arg,1,request,in,out,copy_plugin());assert(result);
        assert(std::string(reinterpret_cast<char*>(bytes.data()),out.size)==(copy_mode==2 ? "suppressed" : "post"));
        auto volatile native_target=&s2bridge_fixture_pointer;
        auto escaped=static_cast<const char*>(native_target(const_cast<uint8_t*>(input)));
        std::vector<std::string> churn(500,std::string(4096,'x'));
        assert(std::string(escaped)==(copy_mode==2 ? "suppressed" : "post"));
    }
    assert(copy_pre==4 && copy_post==4);
    const auto* retained=static_cast<const char*>(s2bridge_fixture_last_pointer());assert(std::string(retained)=="edited");
    assert(service.HookRelease(prepared.value));assert(service.TargetRelease(prepared.value));
    while(!service.Collect()) std::this_thread::sleep_for(std::chrono::milliseconds(1));
    std::vector<std::string> churn(500,std::string(4096,'q'));assert(std::string(retained)=="edited");
    std::cout<<"PASS stock copied alias, immutable frame edits, suppression escape, POST override/arbitration and capacity rollback\n";
}
static void check_callback_trace(const std::vector<std::string>& actual,const std::vector<std::string>& expected) {
    if(actual!=expected) {
        std::cerr<<"unexpected callback order:";
        for(const auto& event:actual) std::cerr<<" "<<event;
        std::cerr<<"\n";
    }
    assert(actual==expected);
}
static std::string arbitration_submission,arbitration_expected;
static std::vector<std::string> arbitration_trace;
static int arbitration_dispatch(long long id,const S2FunctionFrameInfo* info,int phase) {
    using namespace s2bridge;CopyFrameKey key{id,info->frame_token,info->native_epoch,"linux-x86_64-sysv:none:ptr(ptr)"};
    auto request=entity_request(4);
    arbitration_trace.push_back(phase ? "bridge:post" : "bridge:pre");
    if(!phase) {
        auto value=request;value.aux=arbitration_submission.size();CopyInput in{1,sizeof(CopyInput),reinterpret_cast<const uint8_t*>(arbitration_submission.data()),arbitration_submission.size()};
        assert(FrameCommitCopy(key,2,&value,in,copy_plugin()));
    } else {
        std::array<uint8_t,65535> bytes{};CopyOutput out{1,sizeof(CopyOutput),bytes.data(),bytes.size(),0};
        assert(FrameReadCopy(key,-2,request,out));assert(std::string(reinterpret_cast<char*>(bytes.data()),out.size)==arbitration_expected);
        const uint8_t losing[]="post-loser-never-published";CopyInput in{1,sizeof(CopyInput),losing,sizeof(losing)-1};auto value=request;value.aux=in.size;
        const auto before=s2fn::copy::Arena::Resident().Read();assert(FrameOverrideReturnCopy(key,value,in,copy_plugin(),request,out));
        assert(std::string(reinterpret_cast<char*>(bytes.data()),out.size)==arbitration_expected && s2fn::copy::Arena::Resident().Read().values==before.values);
    }
    return 1;
}
static void copied_peer_arbitration() {
    using namespace s2bridge;
    for(bool peer_pre_first:{true,false}) for(auto strength:{KHook::Action::Override,KHook::Action::Supersede}) {
        auto address=reinterpret_cast<uintptr_t>(&s2bridge_fixture_pointer);Fixture fixture(address-0x1200);fixture.freeze();
        Service service([&](const auto&,auto& out,auto&){out.address=address;out.image=fixture.image;return true;});
        CoreDispatchSink sink(arbitration_dispatch);assert(service.SetDispatchSink(&sink));
        auto t=target();t["resolve"]="direct";t["derivation"]="identity";t["candidateValidate"]=json::object();auto a=abi();
        a["parameters"][0]["native"]="ptr";a["parameters"][0]["projection"]["id"]="string";a["parameters"][0]["ownership"]="callee-borrowed";
        a["returns"]["native"]="ptr";a["returns"]["projection"]["id"]="string";a["returns"]["ownership"]="caller-borrowed";a["fingerprint"]="linux-x86_64-sysv:none:ptr(ptr)";
        auto prepared=service.Prepare("peer-copy",t.dump(),a.dump(),a["fingerprint"]);assert(prepared);
        struct Peer : s2fn::DispatchSink {
            KHook::Action strength;
            void Dispatch(s2fn::DispatchFrame& f) override {
                arbitration_trace.push_back(f.phase==s2fn::Phase::Pre ? "peer:pre" : "peer:post");
                if(f.phase==s2fn::Phase::Pre){f.action=strength;f.result=s2fn::NativeValue::From<const char*>("peer");}
            }
            void Error(const char*) noexcept override {std::abort();}
        } peer;peer.strength=strength;
        s2fn::AbiSignature sig;sig.parameters={{"ptr","string"}};sig.returns={"ptr","string"};auto binding=s2fn::RuntimeBinding::Create(sig,peer);assert(binding);
        // Stock inserts each new PRE+POST callback before existing PRE+POST
        // callbacks. Registration order is the reverse of PRE execution order.
        if(!peer_pre_first) assert(binding.value->Configure(reinterpret_cast<void*>(address)).Accepted());
        assert(service.HookAcquire(prepared.value));
        if(peer_pre_first) assert(binding.value->Configure(reinterpret_cast<void*>(address)).Accepted());
        arbitration_submission=std::string("submitted-")+(peer_pre_first ? "prior-" : "later-")+std::to_string(static_cast<int>(strength));
        arbitration_expected=peer_pre_first && strength==KHook::Action::Supersede ? "peer" : arbitration_submission;
        arbitration_trace.clear();
        auto before=s2fn::copy::Arena::Resident().Read();auto volatile target=&s2bridge_fixture_pointer;
        const char* escaped=static_cast<const char*>(target(const_cast<char*>("input")));assert(std::string(escaped)==arbitration_expected);
        const std::vector<std::string> expected_trace=peer_pre_first
            ? std::vector<std::string>{"peer:pre","bridge:pre","bridge:post","peer:post"}
            : std::vector<std::string>{"bridge:pre","peer:pre","peer:post","bridge:post"};
        check_callback_trace(arbitration_trace,expected_trace);
        // A host-fold winner submitted at PRE stays charged even if an earlier
        // peer Supersede wins. Public provider APIs cannot distinguish its PRE
        // strength; no hidden layout read or altered comparison is permitted.
        assert(s2fn::copy::Arena::Resident().Read().values==before.values+1);
        binding.value->BeginRemove();while(!binding.value->RemovalComplete()) std::this_thread::sleep_for(std::chrono::milliseconds(1));assert(binding.value->PruneCompletedTicket());binding.value.reset();
        assert(service.HookRelease(prepared.value));assert(service.TargetRelease(prepared.value));while(!service.Collect()) std::this_thread::sleep_for(std::chrono::milliseconds(1));
        std::vector<std::string> churn(500,std::string(4096,'z'));assert(std::string(escaped)==arbitration_expected);
    }
    std::cout<<"PASS stock copied PRE submission with prior/later Override/Supersede and conclusive POST loser no-charge\n";
}
static s2bridge::Service* borrowed_service=nullptr;
static bool borrowed_nested=false;
static int borrowed_reads=0, borrowed_peers=0;
static std::vector<std::string> borrowed_trace;
static int borrowed_dispatch(long long id,const S2FunctionFrameInfo* info,int phase) {
    using namespace s2bridge;
    CopyFrameKey key{id,info->frame_token,info->native_epoch,"linux-x86_64-sysv:none:i32(ptr)"};
    auto request=entity_request(4);uint8_t bytes[32]{};CopyOutput out{1,sizeof(CopyOutput),bytes,32,0};
    assert(FrameReadCopy(key,0,request,out));
    if(phase) {
        assert(std::string(reinterpret_cast<char*>(bytes),out.size)=="continued");
        borrowed_trace.push_back(borrowed_nested ? "bridge:post:nested" : "bridge:post:outer");
        return 1;
    }
    ++borrowed_reads;
    const auto saved=std::string(reinterpret_cast<char*>(bytes),out.size);
    borrowed_trace.push_back("bridge:pre:"+saved);
    if(!borrowed_nested) {
        borrowed_nested=true;
        auto arg=request;arg.aux=6;const uint8_t inner[]="nested";CopyInput in{1,sizeof(CopyInput),inner,6};CopyOutput unused{};
        auto result=borrowed_service->CallCopy(id,0,&arg,1,{},in,unused,copy_plugin());assert(result && result.value.bits==9);
        borrowed_nested=false;
        assert(FrameReadCopy(key,0,request,out) && std::string(reinterpret_cast<char*>(bytes),out.size)==saved);
    }
    const uint8_t text[]="continued";CopyInput in{1,sizeof(CopyInput),text,9};auto edit=request;edit.aux=9;
    assert(FrameWriteCopy(key,0,edit,in,copy_plugin()));
    assert(FrameCommitCopy(key,0,nullptr,{},copy_plugin()));
    borrowed_trace.push_back("bridge:commit:"+saved);
    return 1;
}
static void borrowed_recall_execution() {
    using namespace s2bridge;
    auto address=reinterpret_cast<uintptr_t>(&s2bridge_fixture_length);Fixture fixture(address-0x1200);fixture.freeze();
    Service service([&](const auto&,auto& out,auto&){out.address=address;out.image=fixture.image;return true;});borrowed_service=&service;
    CoreDispatchSink sink(borrowed_dispatch);assert(service.SetDispatchSink(&sink));
    auto t=target();t["resolve"]="direct";t["derivation"]="identity";t["candidateValidate"]=json::object();auto a=abi();
    a["parameters"][0]["native"]="ptr";a["parameters"][0]["projection"]["id"]="string";a["parameters"][0]["ownership"]="callee-borrowed";
    a["parameters"][0]["mutable"]=json::array({"pre"});a["fingerprint"]="linux-x86_64-sysv:none:i32(ptr)";
    auto prepared=service.Prepare("borrowed",t.dump(),a.dump(),a["fingerprint"]);assert(prepared);
    struct Peer : s2fn::DispatchSink {
        void Dispatch(s2fn::DispatchFrame& f) override {
            if(f.phase==s2fn::Phase::Post) {
                assert(std::string(f.arguments[0].Get<const char*>())=="continued");
                borrowed_trace.push_back(borrowed_nested ? "peer:post:nested" : "peer:post:outer");
                return;
            }
            ++borrowed_peers;std::vector<std::string> churn(500,std::string(4096,'x'));
            const auto observed=std::string(f.arguments[0].Get<const char*>());
            if(observed!="continued") std::cerr<<"borrowed peer PRE argument: "<<observed<<"\n";
            assert(observed=="continued");
            borrowed_trace.push_back(borrowed_nested ? "peer:pre:nested" : "peer:pre:outer");
        }
        void Error(const char* error) noexcept override {std::cerr<<error<<"\n";std::abort();}
    } peer;
    s2fn::AbiSignature sig;sig.parameters={{"ptr","string"}};sig.returns={"i32","i32"};
    auto binding=s2fn::RuntimeBinding::Create(sig,peer);assert(binding);assert(binding.value->Configure(reinterpret_cast<void*>(address)).Accepted());
    // Register the intended later PRE peer first: stock prepends PRE+POST
    // callbacks, so the bridge must be registered last to recall into this peer.
    assert(service.HookAcquire(prepared.value));
    borrowed_trace.clear();
    const auto before=s2fn::copy::NativeBudget().Read();auto volatile target=&s2bridge_fixture_length;assert(target("outer")==9);
    const std::vector<std::string> expected_trace={
        "bridge:pre:outer","bridge:pre:nested","bridge:commit:nested",
        "peer:pre:nested","peer:post:nested","bridge:post:nested","bridge:commit:outer",
        "peer:pre:outer","peer:post:outer","bridge:post:outer"};
    check_callback_trace(borrowed_trace,expected_trace);
    assert(borrowed_reads==2 && borrowed_peers==2 && s2fn::copy::NativeBudget().Read().bytes==before.bytes);
    binding.value->BeginRemove();while(!binding.value->RemovalComplete()) std::this_thread::sleep_for(std::chrono::milliseconds(1));assert(binding.value->PruneCompletedTicket());binding.value.reset();
    assert(service.HookRelease(prepared.value));assert(service.TargetRelease(prepared.value));while(!service.Collect()) std::this_thread::sleep_for(std::chrono::milliseconds(1));
    borrowed_service=nullptr;std::cout<<"PASS stock borrowed PRE storage across recall/later peer heap churn and nested frame leases\n";
}
static void scalar_transport() {
    Fixture fixture(reinterpret_cast<uintptr_t>(&native)-0x1200);fixture.freeze();
    s2bridge::Service service([&](const auto&,auto& out,auto&){out.address=reinterpret_cast<uintptr_t>(&native);out.image=fixture.image;return true;});
    s2bridge::CoreDispatchSink sink(scalar_dispatch);assert(service.SetDispatchSink(&sink));
    auto t=target();t["resolve"]="direct";t["derivation"]="identity";t["candidateValidate"]=json::object();auto a=abi();
    auto binding=service.Prepare("transport",t.dump(),a.dump(),a["fingerprint"]);assert(binding);assert(service.HookAcquire(binding.value));
    S2FunctionValue input{};input.kind=2;input.bits=7;
    for(bool suppressed:{false,true}) {scalar_suppressed=suppressed;auto result=service.Call(binding.value,0,&input,1);assert(result && result.value.bits==(suppressed ? 73u : 91u) && transport_invocations.empty());}
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
    a["parameters"][0]["native"]="ptr";a["parameters"][0]["projection"]["id"]="entity";
    a["returns"]["native"]="ptr";a["returns"]["projection"]["id"]="entity";
    a["fingerprint"]="linux-x86_64-sysv:none:ptr(ptr)";
    S2FunctionValue pointer{};pointer.kind=static_cast<unsigned char>(s2bridge::ValueKind::Pointer);
    pointer.flags=static_cast<unsigned char>(s2bridge::PointerProjection::Opaque);pointer.bits=73;
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
    assert(!service.Call(member.value,0,&pointer,1,pointer)); // opaque is not a receiver
    auto entity=pointer;entity.flags=static_cast<unsigned char>(s2bridge::PointerProjection::Entity);
    assert(service.Call(member.value,0,&entity,1,pointer));
    codec.live=false;assert(!service.Call(member.value,0,&entity,1,pointer));
    assert(service.TargetRelease(member.value));assert(service.Collect() && allocations==frees);
    std::cout << "PASS real CIF/shared physical stock hook/lazy calls/nested owner bypass/refcount/retirement\n";
    worker_join_regression();
    copied_execution();
    borrowed_recall_execution();
    copied_peer_arbitration();
    scalar_transport();
    entity_codec();
    entity_transport();
    busy_service_insertion();
    KHook::Shutdown();
}
}
#endif
int main(int argc,char** argv) {
    if(argc==2 && std::string(argv[1])=="--copy-quota") {copied_quota_transaction();return 0;}
    stages(); instance_normalization(); string_indirect_normalization(); hidden_relationship(); copied_transport_primitives();
#ifndef S2FN_VALIDATION_ONLY
    runtime();
#endif
}


#endif
