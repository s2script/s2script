#include "engine_function_bridge.h"
#include "../third_party/json.hpp"
#include "vtable.h"
#include "sha256.h"
#include <limits>
#include <stdexcept>
#include <cmath>
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
    keys(j,returns ? std::initializer_list<const char*>{"native","projection","ownership"} :
                    std::initializer_list<const char*>{"name","native","projection","mutable","ownership"});
    s2fn::AbiAtom result{string(j,"native"),string(j.at("projection"),"id")};
    keys(j.at("projection"),{"id","version"});
    require(j.at("projection").at("version")==1,"unsupported projection version");
    bool valid=result.native==result.projection && result.native!="ptr" && result.native!="u8";
    if (result.native=="void") valid=returns && result.projection=="void";
    if (result.native=="u8") valid=result.projection=="bool";
    if (result.native=="ptr") valid=result.projection=="entity" || result.projection=="entity?" ||
        result.projection=="string" || result.projection=="vector";
    require(valid,"unsupported native/projection pair");
    const bool copied=result.projection=="string" || result.projection=="vector";
    if (copied) {
        require(j.contains("ownership"),"FunctionCopyLifetimeUnsupported: copied ownership required; rebuild declaration");
        const auto owner=string(j,"ownership");
        require(owner=="native-observed" || (returns ? owner=="caller-borrowed" : owner=="callee-borrowed" || owner=="callee-retained"),
            "FunctionCopyLifetimeUnsupported: invalid directional ownership");
    } else require(!j.contains("ownership"),"ownership only allowed on copied positions");
    return result;
}
CopyPosition copy_position(const json& j, bool returns) {
    CopyPosition p;
    auto projection=string(j.at("projection"),"id");
    if(projection!="string" && projection!="vector") return p;
    p.kind=projection=="string" ? s2fn::copy::Kind::String : s2fn::copy::Kind::Vector;
    auto owner=string(j,"ownership");
    p.ownership=owner=="callee-borrowed" ? CopyOwnership::CalleeBorrowed :
        owner=="callee-retained" ? CopyOwnership::CalleeRetained :
        owner=="caller-borrowed" ? CopyOwnership::CallerBorrowed : CopyOwnership::NativeObserved;
    if(!returns) {
        require(j.at("mutable").is_array(),"invalid copied mutable phases");
        require(j.at("mutable").empty() || j.at("mutable")==json::array({"pre"}),"invalid copied mutable phases");
        p.mutable_pre=!j.at("mutable").empty();
        require(!p.mutable_pre || p.ownership!=CopyOwnership::NativeObserved,"FunctionCopyLifetimeUnsupported: native-observed is readonly");
    }
    return p;
}
}
bool Declaration::HasCopies() const {
    if(return_copy) return true;
    for(const auto& p:copies) if(p) return true;
    return false;
}
bool Declaration::CompatibleCopies(const Declaration& b) const {
    auto same=[](const CopyPosition& a,const CopyPosition& b) {return a.kind==b.kind && a.ownership==b.ownership && a.indirect==b.indirect;};
    if(!same(return_copy,b.return_copy)) return false;
    for(size_t i=0;i<copies.size();++i) if(!same(copies[i],b.copies[i])) return false;
    return true; // entity and entity? retain their binding-local projection rules.
}
static void parse_target(Declaration& d,const json& t) {
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
}
s2fn::Result<Declaration> Parse(const std::string& target, const std::string& abi, const std::string& fingerprint) {
    try {
        Declaration d; auto t=json::parse(target); auto a=json::parse(abi);
        parse_target(d,t);
        keys(a,{"platform","receiver","fingerprint","stackCopyBytes","parameters","returns"});
        d.abi.platform=string(a,"platform");
        auto receiver=string(a,"receiver"); require(receiver=="none" || receiver=="entity","invalid public receiver");
        d.abi.member_receiver=receiver=="entity";
        d.abi.returns=atom(a.at("returns"),true);
        require(a.at("parameters").is_array(),"expected parameters array");
        for (const auto& p : a.at("parameters")) d.abi.parameters.push_back(atom(p,false));
        require(d.abi.parameters.size()<=32,"unsupported parameter count");
        d.return_copy=copy_position(a.at("returns"),true);
        for(size_t i=0;i<d.abi.parameters.size();++i) {
            d.copies[i]=copy_position(a.at("parameters")[i],false);
            require(!(d.copies[i].mutable_pre && d.copies[i].ownership==CopyOwnership::CalleeBorrowed && d.abi.returns.native=="ptr"),
                "FunctionCopyLifetimeUnsupported: mutable copied input with pointer return requires callee-retained");
        }
        auto info=s2fn::Validate(d.abi); if (!info) return {{},info.error};
        require(info.value.fingerprint==fingerprint && string(a,"fingerprint")==fingerprint,"ABI fingerprint mismatch");
        require(a.at("stackCopyBytes")==info.value.stack_bytes,"stack-copy mismatch");
        d.info=std::move(info.value); return {std::move(d),{}};
    } catch (const std::exception& e) { return {{},std::string("invalid normalized declaration: ")+e.what()}; }
}
namespace {
uint32_t bounded_integer(const json& j,uint32_t max,const char* why) {
    require(j.is_number_unsigned() || (j.is_number_integer() && j.get<int64_t>()>=0),why);
    auto n=j.get<uint64_t>();require(n<=max,why);return static_cast<uint32_t>(n);
}
uint8_t record_phases(const json& j,bool write) {
    require(j.is_array(),"invalid record phases");
    if(j==json::array()) {require(write,"empty record read phases");return 0;}
    if(j==json::array({"pre"})) return 1;
    if(!write && j==json::array({"post"})) return 2;
    if(!write && j==json::array({"pre","post"})) return 3;
    throw std::runtime_error("invalid record phases");
}
RecordLayout record_layout(const json& instance) {
    keys(instance,{"codecId","codecVersion","kind","layoutHash","record"});
    require(instance.at("codecId")=="borrowed-record" && instance.at("codecVersion")==1 && instance.at("kind").is_null(),"unsupported registered instance kind/version");
    const auto& j=instance.at("record");keys(j,{"extent","alignment","fields"});
    RecordLayout out;out.extent=bounded_integer(j.at("extent"),65536,"record extent limit");
    out.alignment=bounded_integer(j.at("alignment"),8,"record alignment limit");
    require(out.extent && out.alignment && !(out.alignment&(out.alignment-1)) && out.extent%out.alignment==0,"invalid record extent/alignment");
    out.hash=string(instance,"layoutHash");
    require(out.hash==s2::digest::sha256(j.dump()),"record layout hash mismatch");
    const auto& fields=j.at("fields");require(fields.is_array() && !fields.empty() && fields.size()<=256,"record field count limit");
    std::set<std::string> names;json storage=json::array();
    for(const auto& row:fields) {
        keys(row,{"name","offsetKey","offset","storage","nullable","read","write"});
        RecordField f;f.name=string(row,"name");
        require(f.name.size()<=128 && names.insert(f.name).second && string(row,"offsetKey").size()<=256,"invalid record field name");
        f.storage=string(row,"storage");
        f.width=f.storage=="bool" ? 1 : f.storage=="u16" ? 2 : (f.storage=="i32" || f.storage=="u32" || f.storage=="f32" || f.storage=="entity-handle32") ? 4 :
            (f.storage=="i64" || f.storage=="u64" || f.storage=="f64") ? 8 : 0;
        require(f.width,"unsupported record storage");
        f.offset=bounded_integer(row.at("offset"),out.extent,"record offset exceeds extent");
        require(f.width<=out.extent-f.offset && out.alignment>=f.width && f.offset%f.width==0,"record field extent/alignment");
        for(const auto& prior:out.fields) require(f.offset>=prior.offset+prior.width || prior.offset>=f.offset+f.width,"record field overlap");
        require(row.at("nullable").is_boolean(),"invalid record nullability");f.nullable=row.at("nullable");
        require(!f.nullable || f.storage=="entity-handle32","invalid record nullability");
        f.read=record_phases(row.at("read"),false);f.write=record_phases(row.at("write"),true);
        require(!f.write || (f.read&1),"record write requires PRE read");
        storage.push_back({f.offset,f.storage});out.fields.push_back(std::move(f));
    }
    out.storage_key=json::array({out.extent,out.alignment,storage}).dump();return out;
}
Declaration parse_instance(const std::string& target,const std::string& text) {
    require(text.size()<=1024*1024,"instance contract byte limit");
    auto j=json::parse(text);keys(j,{"version","signature","policy","selectedDataHash","contractHash"});
    require(j.at("version")==1,"unsupported instance contract version");
    const auto hash=string(j,"contractHash");j.erase("contractHash");
    require(hash==s2::digest::sha256(j.dump()),"instance contract hash mismatch");
    require(string(j,"selectedDataHash").size()==64,"invalid selected-data hash");
    const auto& a=j.at("signature");
    keys(a,{"platform","memberReceiver","receiver","parameters","returns","fingerprint","stackCopyBytes","instances"});
    Declaration d;parse_target(d,json::parse(target));d.abi.platform=string(a,"platform");
    require(a.at("memberReceiver").is_boolean(),"invalid physical receiver");d.abi.member_receiver=a.at("memberReceiver");
    std::vector<RecordLayout> layouts;
    require(a.at("instances").is_array() && a.at("instances").size()<=33,"instance count limit");
    for(const auto& row:a.at("instances")) layouts.push_back(record_layout(row));
    auto position=[&](const json& p,int selector,bool ret) {
        keys(p,{"name","native","projection","ownership","mutable","instance","nullable"});
        const auto native=string(p,"native");const auto projection=string(p.at("projection"),"id");
        keys(p.at("projection"),{"id","version"});require(p.at("projection").at("version")==1,"unsupported position version");
        require(p.at("nullable").is_boolean(),"invalid position nullability");
        if(projection=="borrowed-record" || projection=="native-only") {
            require(!ret && native=="ptr" && p.at("mutable")==json::array(),"record/hidden position direction or mutation");
            require(p.at("ownership")== (projection=="borrowed-record" ? "synchronous-record" : "invocation-passthrough"),"missing native lifetime registration");
            if(projection=="native-only") {require(!p.contains("instance") && !p.at("nullable").get<bool>(),"hidden instance/nullability");d.instances.hidden.insert(selector);}
            else {const auto index=bounded_integer(p.at("instance"),32,"invalid instance index");require(index<layouts.size(),"unknown record instance");d.instances.records.emplace(selector,RecordPosition{layouts[index],p.at("nullable")});}
            return s2fn::AbiAtom{native,projection};
        }
        require(!p.contains("instance") && !p.at("nullable").get<bool>(),"unexpected position instance/nullability");
        if(projection=="string-indirect") {
            // Trusted-only copied projection; the public grammar (atom) never admits it.
            require(!ret && selector>=0 && native=="ptr" && p.at("mutable")==json::array() &&
                p.contains("ownership") && p.at("ownership")=="native-observed","string-indirect is a readonly native-observed parameter");
            d.copies[selector]={s2fn::copy::Kind::String,CopyOwnership::NativeObserved,false,true};
            return s2fn::AbiAtom{native,projection};
        }
        auto simple=p;simple.erase("nullable");
        if(ret) {simple.erase("name");simple.erase("mutable");}
        auto value=atom(simple,ret);
        if(selector==-1) {require(native=="ptr" && (projection=="entity" || projection=="entity?") && p.at("mutable")==json::array(),"invalid receiver projection");d.instances.nullable_receiver=projection=="entity?";}
        else if(ret) d.return_copy=copy_position(simple,true);
        else d.copies[selector]=copy_position(simple,false);
        return value;
    };
    require(d.abi.member_receiver==!a.at("receiver").is_null(),"physical receiver projection mismatch");
    if(d.abi.member_receiver) position(a.at("receiver"),-1,false);
    require(a.at("parameters").is_array() && a.at("parameters").size()<=32,"physical parameter limit");
    for(const auto& p:a.at("parameters")) {auto index=static_cast<int>(d.abi.parameters.size());d.abi.parameters.push_back(position(p,index,false));}
    d.abi.returns=position(a.at("returns"),-2,true);
    auto info=s2fn::Validate(d.abi);require(bool(info),info.error.c_str());
    require(a.at("fingerprint")==info.value.fingerprint && a.at("stackCopyBytes")==info.value.stack_bytes,"physical fingerprint/stack mismatch");
    d.info=std::move(info.value);
    const auto& policy=j.at("policy");
    require(policy.at("id")=="generic.v2" && policy.at("version")==1,"unsupported instance policy");
    const auto& surfaces=policy.at("surfaces");require(surfaces.is_array() && !surfaces.empty(),"invalid instance surfaces");
    for(const auto& s:surfaces) require(s=="pre" || s=="post","record/hidden declarations are hook-only");
    for(const auto& row:d.instances.records) for(const auto& field:row.second.layout.fields)
        require(!field.write || std::find(surfaces.begin(),surfaces.end(),"pre")!=surfaces.end(),"record writes require PRE surface");
    return d;
}
unsigned char copy_flag(s2fn::copy::Kind kind) { return kind==s2fn::copy::Kind::String ? 4 : 5; }
constexpr size_t SpanBudget = s2fn::copy::MaxBatch * s2fn::copy::MaxString;
bool valid_copy_input(const CopyInput& input) {
    return input.version==1 && input.struct_size==sizeof(CopyInput) && input.size<=SpanBudget &&
        (!input.size || input.data) && (!input.data || input.size<=UINTPTR_MAX-reinterpret_cast<uintptr_t>(input.data));
}
}
s2fn::Result<Declaration> ParseInstance(const std::string& target,const std::string& contract) {
    try {return {parse_instance(target,contract),{}};}
    catch(const std::exception& e) {return {{},std::string("invalid trusted instance declaration: ")+e.what()};}
}
s2fn::Result<s2fn::copy::OwnerGeneration> CheckedProducer(const CopyProducer& producer) {
    if(producer.version!=1 || producer.struct_size!=sizeof(CopyProducer) || producer.reserved || producer.domain>2)
        return {{},"FunctionCopyLifetimeUnsupported: invalid trusted producer context"};
    return {{{static_cast<s2fn::copy::Domain>(producer.domain),producer.digest},producer.generation},{}};
}
s2fn::Result<s2fn::copy::Snapshot> DecodeCopy(const s2fn::copy::Operation& operation,s2fn::copy::Kind kind,
    const S2FunctionValue& value,const CopyInput& input) {
    if(!valid_copy_input(input) ||
       value.kind!=8 || value.flags!=copy_flag(kind) || value.reserved || value.bits>input.size || value.aux>input.size-value.bits)
        return {{},"FunctionCopyLifetimeUnsupported: invalid copied span/value"};
    static const uint8_t empty=0;
    const auto* data=input.data ? input.data+value.bits : &empty;
    return s2fn::copy::Snapshot::FromBytes(operation,kind,data,value.aux);
}
s2fn::Result<bool> AdmitCopyOutput(s2fn::copy::Kind kind,const S2FunctionValue& request,const CopyOutput& out,size_t size) {
    if(request.kind!=8 || request.flags!=copy_flag(kind) || request.reserved || request.bits || request.aux ||
       out.version!=1 || out.struct_size!=sizeof(CopyOutput) || out.capacity>SpanBudget || (out.capacity && !out.data) || (out.data && out.capacity>UINTPTR_MAX-reinterpret_cast<uintptr_t>(out.data)))
        return {false,"FunctionCopyLifetimeUnsupported: invalid copied output request"};
    if(size>out.capacity) return {false,"FunctionCopyBudgetExceeded: copied output capacity"};
    return {true,{}};
}
s2fn::Result<S2FunctionValue> EncodeCopy(const s2fn::copy::Snapshot& snapshot,const S2FunctionValue& request,CopyOutput& out) {
    if(!snapshot) return {{},"FunctionCopyLifetimeUnsupported: missing snapshot"};
    auto admitted=AdmitCopyOutput(snapshot.kind(),request,out,snapshot.size());if(!admitted) return {{},admitted.error};
    if(snapshot.size()) std::memcpy(out.data,snapshot.data(),snapshot.size());
    auto result=request;result.aux=static_cast<uint32_t>(snapshot.size());out.size=snapshot.size();
    return {result,{}};
}
s2fn::Result<s2fn::RetainedFrame> CopyTransaction::Create(const CopyProducer& engine) {
    auto producer=CheckedProducer(engine);if(!producer) return {{},producer.error};
    if(producer.value.owner.domain!=s2fn::copy::Domain::Engine) return {{},"FunctionCopyLifetimeUnsupported: capture requires Engine producer"};
    auto operation=s2fn::copy::NativeBudget().Begin(producer.value);if(!operation) return {{},operation.error};
    auto charge=operation.value.Reserve(sizeof(CopyTransaction));if(!charge) return {{},charge.error};
    auto* state=new(std::nothrow) CopyTransaction;
    if(!state) return {{},"FunctionCopyBudgetExceeded: frame storage"};
    state->charge_=std::move(charge.value);state->engine_=std::move(operation.value);
    return {s2fn::RetainedFrame(state),{}};
}
void CopyTransaction::Release() noexcept { auto charge=std::move(charge_);delete this; }
s2fn::Result<bool> CopyTransaction::Capture(size_t index,s2fn::copy::Kind kind,uintptr_t source,const s2fn::copy::Reader& reader) {
    if(index>=observed_.size()) return {false,"invalid copied capture position"};
    auto snapshot=s2fn::copy::Snapshot::Capture(engine_,kind,source,reader);if(!snapshot) return {false,snapshot.error};
    observed_[index]=std::move(snapshot.value);return {true,{}};
}
s2fn::Result<bool> CopyTransaction::CaptureIndirect(size_t index,uintptr_t object,const s2fn::copy::Reader& reader) {
    if(index>=32) return {false,"invalid indirect capture position"};
    auto snapshot=s2fn::copy::Snapshot::CaptureIndirect(engine_,object,reader);if(!snapshot) return {false,snapshot.error};
    observed_[index]=std::move(snapshot.value);return {true,{}};
}
s2fn::Result<bool> CopyTransaction::Stage(size_t index,s2fn::copy::Kind kind,const S2FunctionValue& value,
    const CopyInput& input,const CopyProducer& producer) {
    if(index>=edits_.size()) return {false,"invalid copied edit position"};
    auto checked=CheckedProducer(producer);if(!checked) return {false,checked.error};
    auto operation=s2fn::copy::NativeBudget().Begin(checked.value);if(!operation) return {false,operation.error};
    auto snapshot=DecodeCopy(operation.value,kind,value,input);if(!snapshot) return {false,snapshot.error};
    edits_[index]=std::move(snapshot.value);producers_[index]=checked.value.owner;return {true,{}};
}
const s2fn::copy::Snapshot& CopyTransaction::Read(size_t index) const {
    require(index<observed_.size(),"invalid copied read position");
    return index<edits_.size() && edits_[index] ? edits_[index] : observed_[index];
}
void CopyTransaction::AcceptReturn(const s2fn::copy::Snapshot& value) noexcept {
    observed_[Return]=value;edits_[Return]={};
}
s2fn::Result<std::array<const void*,33>> CopyTransaction::Publish(const std::array<CopyPosition,32>& positions,size_t count,bool return_wins) {
    using namespace s2fn::copy;
    if(count>32) return {{},"invalid copied transaction count"};
    struct Batch {
        std::array<Snapshot,MaxBatch> values;
        std::array<StableOwner,MaxBatch> owners;
        std::array<const void*,MaxBatch> permanent{}, result{};
        std::array<size_t,MaxBatch> indices{};
    };
    auto charge=engine_.Reserve(sizeof(Batch));if(!charge) return {{},charge.error};
    Batch batch{};size_t size=0;
    for(size_t i=0;i<MaxBatch;++i) {
        if(!edits_[i] || (i<count ? false : i!=Return || !return_wins)) continue;
        if(i<count && (!positions[i] || positions[i].ownership==CopyOwnership::NativeObserved || positions[i].kind!=edits_[i].kind()))
            return {{},"FunctionCopyLifetimeUnsupported: invalid copied transaction position"};
        const bool permanent=i==Return || positions[i].ownership==CopyOwnership::CalleeRetained;
        if(permanent) {batch.values[size]=edits_[i];batch.owners[size]=producers_[i];batch.indices[size++]=i;}
        else batch.result[i]=edits_[i].data();
    }
    auto intern=Arena::Resident().Intern(engine_,batch.owners.data(),batch.values.data(),size,batch.permanent.data());
    if(!intern) return {{},intern.error};
    // No fallible work after publication: fixed-array assignments retain leases.
    for(size_t i=0;i<size;++i) batch.result[batch.indices[i]]=batch.permanent[i];
    for(size_t i=0;i<count;++i) if(edits_[i]) observed_[i]=edits_[i];
    if(return_wins && edits_[Return]) observed_[Return]=edits_[Return];
    return {batch.result,{}};
}
s2fn::Result<bool> ReferencesHidden(const s2fn::copy::Reader& reader,uintptr_t owner,uint32_t offset,uintptr_t hidden) {
    if(!reader.available || !reader.read || !reader.page_size) return {false,"relationship check: native reader unavailable"};
    constexpr size_t Word=sizeof(uintptr_t);
    if(!owner || !hidden || offset>UINTPTR_MAX-owner || owner+offset>UINTPTR_MAX-(Word-1)) return {false,{}};
    const uintptr_t at=owner+offset;uint8_t word[Word];size_t done=0;
    while(done<Word) {
        const auto address=at+done;const auto count=std::min(Word-done,reader.page_size-address%reader.page_size);
        if(reader.read(reader.context,address,word+done,count)!=count) return {false,{}};
        done+=count;
    }
    uintptr_t stored=0;std::memcpy(&stored,word,Word);
    return {stored==hidden,{}};
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
#include <climits>
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
        v.flags<=static_cast<unsigned char>(PointerProjection::Opaque);
}
}
s2fn::Result<s2fn::NativeValue> EntityPointerCodec::Decode(const S2FunctionValue& value,CallStorage&) {
    if(std::this_thread::get_id()!=owner_) return {{},"entity codec off host thread"};
    if(value.kind!=8 || (value.flags!=1 && value.flags!=2) || value.reserved || value.bits>UINT32_MAX ||
       (value.aux>INT32_MAX && value.aux!=UINT32_MAX) ||
       (value.aux==UINT32_MAX && (value.flags!=2 || value.bits!=0))) return {{},"invalid entity identity transport"};
    void* pointer=nullptr;
    if(value.aux!=UINT32_MAX && access_.resolve) pointer=access_.resolve(value.aux,static_cast<uint32_t>(value.bits));
    if(!pointer && value.flags==1) return {{},"strict entity projection is null or stale"};
    return {s2fn::NativeValue::From(pointer),{}};
}
s2fn::Result<S2FunctionValue> EntityPointerCodec::Encode(const s2fn::NativeValue& value,const S2FunctionValue& request) {
    if(std::this_thread::get_id()!=owner_) return {{},"entity codec off host thread"};
    if(request.kind!=8 || (request.flags!=1 && request.flags!=2) || request.reserved || request.aux || request.bits)
        return {{},"invalid entity projection request"};
    EntityIdentity identity;const auto pointer=value.Get<void*>();
    bool live=pointer && access_.identify && access_.identify(pointer,identity);
    if(live && (identity.index>INT32_MAX || !access_.resolve || access_.resolve(identity.index,identity.serial)!=pointer)) live=false;
    if(!live && request.flags==1) return {{},"strict entity projection cannot adopt native value"};
    auto out=request;out.aux=live ? identity.index : UINT32_MAX;out.bits=live ? identity.serial : 0;
    return {out,{}};
}
namespace {
static_assert(sizeof(S2FunctionFrameInfo)==48 && alignof(S2FunctionFrameInfo)==8 &&
    offsetof(S2FunctionFrameInfo,invocation_id)==24 && offsetof(S2FunctionFrameInfo,flags)==44,
    "frame metadata v1 layout");
static_assert(sizeof(S2FunctionHookStatus)==16 && offsetof(S2FunctionHookStatus,receipt)==8,
    "hook status layout");
struct FrameAccess {
    TargetId target;
    const Declaration& declaration;
    const std::string& canonical;
    s2fn::DispatchFrame& native;
    S2FunctionFrameInfo info;
    s2fn::RuntimeBinding& binding;
    std::thread::id owner;
    std::vector<s2fn::NativeValue> staged;
    PointerCodec* codec=nullptr;
    std::map<int,S2FunctionValue> entity_edits;
    bool changed=false, committed=false;
    CopyTransaction* copies=nullptr;
    struct InstanceCapability {
        uint64_t binding=0; TargetId target=0;
        S2FunctionInstanceOwner owner{};
        Declaration declaration;
        bool active=false, released=false;
    };
    using Capabilities=std::map<uint64_t,std::shared_ptr<InstanceCapability>>;
    Capabilities* capabilities=nullptr;
    struct FieldEdit { std::shared_ptr<InstanceCapability> capability; uint32_t field; S2FunctionValue value; };
    std::map<std::pair<int,uint32_t>,FieldEdit> field_edits;
    const s2fn::copy::Reader* reader=nullptr; // host checked reader for relationship reads
};
thread_local std::vector<FrameAccess*> frames;
std::atomic<unsigned long long> next_frame{1};
unsigned long long unique_frame() {
    auto value=next_frame.load(std::memory_order_relaxed);
    do { if (value==ULLONG_MAX) throw std::runtime_error("frame epoch exhausted"); }
    while (!next_frame.compare_exchange_weak(value,value+1,std::memory_order_relaxed));
    return value;
}
FrameAccess& frame_access(TargetId target, unsigned long long token, unsigned long long epoch, const char* fingerprint) {
    require(!frames.empty(),"function frame unavailable on this thread");
    auto& f=*frames.back();
    require(std::this_thread::get_id()==f.owner,"function frame unavailable off host thread");
    require(f.target==target && f.info.frame_token==token && f.info.native_epoch==epoch &&
        fingerprint && f.declaration.info.fingerprint==fingerprint,"function frame capability mismatch");
    return f;
}
size_t scalar_width(ValueKind k) {
    if (k==ValueKind::Void) return 0;
    if (k==ValueKind::Bool) return 1;
    if (k==ValueKind::I32 || k==ValueKind::U32 || k==ValueKind::F32) return 4;
    require(k!=ValueKind::Pointer,"unsupported pointer projection in scalar frame");
    return 8;
}
S2FunctionValue scalar_copy(const s2fn::NativeValue& value, ValueKind k) {
    S2FunctionValue result{}; result.kind=static_cast<unsigned char>(k);
    std::memcpy(&result.bits,value.bytes.data(),scalar_width(k));
    require(k!=ValueKind::Bool || result.bits<=1,"noncanonical bool");
    return result;
}
s2fn::NativeValue scalar_decode(const S2FunctionValue& value, ValueKind k) {
    const auto width=scalar_width(k);
    require(value.kind==static_cast<unsigned char>(k) && !value.flags && !value.reserved && !value.aux,
        "scalar value kind/metadata mismatch");
    require((width==8 || (width==4 ? value.bits<=UINT32_MAX : width==1 ? value.bits<=1 : value.bits==0)),
        "noncanonical scalar value");
    s2fn::NativeValue out; std::memcpy(out.bytes.data(),&value.bits,width); return out;
}
using InstanceCapability=FrameAccess::InstanceCapability;
std::pair<FrameAccess*,std::shared_ptr<InstanceCapability>> instance_access(const S2FunctionInstanceAccess& key) {
    require(key.version==1 && key.struct_size==48 && !frames.empty(),"invalid instance frame access");
    auto& f=*frames.back();
    require(std::this_thread::get_id()==f.owner && f.target==key.target && f.info.frame_token==key.frame_token &&
        f.info.native_epoch==key.native_epoch && f.capabilities,"instance frame capability mismatch");
    auto i=f.capabilities->find(key.capability);require(i!=f.capabilities->end(),"instance capability retired");
    const auto& c=i->second;
    require(c->active && !c->released && c->binding==key.binding_id && c->target==key.target && c->owner.generation &&
        c->declaration.info.fingerprint==f.declaration.info.fingerprint,"instance binding authority mismatch");
    return {&f,c};
}
uintptr_t record_pointer(FrameAccess& f,const InstanceCapability& c,int selector) {
    const auto i=c.declaration.instances.records.find(selector);require(i!=c.declaration.instances.records.end(),"not a record position");
    const auto pointer=selector==-1 ? f.native.receiver.Get<uintptr_t>() : f.native.arguments.at(selector).Get<uintptr_t>();
    require(pointer || i->second.nullable,"null strict record");
    if(pointer) require(pointer%i->second.layout.alignment==0 && pointer<=UINTPTR_MAX-i->second.layout.extent,"record pointer alignment/extent");
    return pointer;
}
const RecordField& record_field(FrameAccess& f,const InstanceCapability& c,int selector,uint32_t field,bool write) {
    auto row=c.declaration.instances.records.find(selector);require(row!=c.declaration.instances.records.end(),"not a record position");
    require(field<row->second.layout.fields.size(),"record field index out of bounds");
    const auto& result=row->second.layout.fields[field];
    const auto phase=f.native.phase==s2fn::Phase::Pre ? 1 : f.native.phase==s2fn::Phase::Post ? 2 : 0;
    require(phase && (result.read&phase) && (!write || (phase==1 && !f.committed && (result.write&1))),"record field is readonly/unavailable in this phase");
    return result;
}
uint64_t field_bits(FrameAccess& f,const RecordField& field,const S2FunctionValue& value) {
    if(field.storage=="entity-handle32") {
        require(value.kind==8 && value.flags==(field.nullable ? 2 : 1) && !value.reserved && value.bits<=0x1ffff,"invalid packed entity value");
        if(value.aux==UINT32_MAX) {require(field.nullable && !value.bits,"invalid null entity field");return UINT32_MAX;}
        require(value.aux<0x8000 && f.codec,"packed entity index/codec unavailable");
        auto strict=value;strict.flags=1;CallStorage storage;
        auto resolved=f.codec->Decode(strict,storage);require(bool(resolved) && resolved.value.Get<void*>(),"stale packed entity field");
        return (value.bits<<15)|value.aux;
    }
    const auto k=kind(field.storage=="bool" ? "u8" : field.storage=="u16" ? "u32" : field.storage);
    auto decoded=scalar_decode(value,k);
    require(field.storage!="u16" || value.bits<=UINT16_MAX,"record u16 out of range");
    require(k!=ValueKind::F32 || std::isfinite(decoded.Get<float>()),"nonfinite record f32");
    require(k!=ValueKind::F64 || std::isfinite(decoded.Get<double>()),"nonfinite record f64");
    return value.bits;
}
struct FieldPublication { uintptr_t destination; uint64_t bits; uint32_t width; };
std::vector<FieldPublication> prepare_fields(FrameAccess& f) {
    std::vector<FieldPublication> stores;stores.reserve(f.field_edits.size());
    for(const auto& row:f.field_edits) {
        const auto& edit=row.second;const auto& c=*edit.capability;
        require(c.active && !c.released && c.target==f.target,"record writer retired before commit");
        const auto& field=record_field(f,c,row.first.first,edit.field,true);
        auto pointer=record_pointer(f,c,row.first.first);require(pointer,"cannot write null record");
        const auto bits=field_bits(f,field,edit.value);
        stores.push_back({pointer+field.offset,bits,field.width});
    }
    return stores;
}
void publish_fields(const std::vector<FieldPublication>& fields) noexcept {
    // Registered synchronous native lifetime is a prerequisite. No rollback claim
    // is made for a host contract violation or fault after the first store.
    for(const auto& f:fields) std::memcpy(reinterpret_cast<void*>(f.destination),&f.bits,f.width);
}
}
void CoreDispatchSink::Dispatch(TargetId target, unsigned long long, s2fn::DispatchFrame& frame) {
    if (!dispatch_ || std::this_thread::get_id()!=owner_) throw std::runtime_error("function dispatch unavailable off host thread");
    require(!frames.empty() && frames.back()->target==target,"missing native frame lease");
    if (dispatch_(target,&frames.back()->info,frame.phase==s2fn::Phase::Pre ? 0 : 1)!=1)
        throw std::runtime_error("synchronous core function dispatch failed");
}
void CoreDispatchSink::Error(TargetId target,const char* why) noexcept {
    std::fprintf(stderr,"[s2script] function target %lld: %s\n",target,why);
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
        // The provider hook outlives its last subscription while the target is referenced, so an
        // immediate resubscription (plugin reload) reuses it instead of racing async removal.
        bool installing=false, hooked=false;
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
            if (sink) {
                CopyTransaction* copies=nullptr;
                if(declaration.HasCopies()) {
                    require(std::this_thread::get_id()==host.owner,"copied frame unavailable off host thread");
                    auto storage=CopyTransaction::Create(host.engine);require(bool(storage),storage.error.c_str());
                    frame.copied=std::move(storage.value);copies=static_cast<CopyTransaction*>(frame.copied.get());
                    for(size_t i=0;i<declaration.abi.parameters.size();++i) if(declaration.copies[i]) {
                        auto captured=declaration.copies[i].indirect ?
                            copies->CaptureIndirect(i,frame.arguments[i].Get<uintptr_t>(),host.reader) :
                            copies->Capture(i,declaration.copies[i].kind,frame.arguments[i].Get<uintptr_t>(),host.reader);
                        require(bool(captured),captured.error.c_str());
                    }
                    if(frame.phase==s2fn::Phase::Post && declaration.return_copy) {
                        auto captured=copies->Capture(CopyTransaction::Return,declaration.return_copy.kind,frame.result.Get<uintptr_t>(),host.reader);
                        require(bool(captured),captured.error.c_str());
                        if(!frame.original_skipped) {
                            captured=copies->Capture(CopyTransaction::Original,declaration.return_copy.kind,frame.original_result.Get<uintptr_t>(),host.reader);
                            require(bool(captured),captured.error.c_str());
                        }
                    }
                }
                const auto epoch=unique_frame();
                FrameAccess access{id,declaration,canonical_id,frame,
                    {1,sizeof(S2FunctionFrameInfo),epoch,epoch,frame.invocation_id,owner,
                     static_cast<unsigned int>(frame.arguments.size()),frame.original_skipped ? 1u : 0u},
                    *binding,host.owner,frame.arguments,host.codec,{}};
                access.copies=copies;access.capabilities=&host.capabilities;access.reader=&host.reader;
                frames.push_back(&access);
                struct Pop { ~Pop() { frames.pop_back(); } } pop;
                sink->Dispatch(id,owner,frame);
            }
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
    const std::thread::id owner=std::this_thread::get_id();
    Resolver resolver;
    DispatchSink* sink=nullptr;
    PointerCodec* codec=nullptr;
    s2fn::copy::Reader reader=s2fn::copy::SystemReader();
    CopyProducer engine{};
    TargetId next=1;
    std::map<TargetId,std::shared_ptr<Record>> records;
    std::map<PhysicalKey,TargetId> physical;
    FrameAccess::Capabilities capabilities;
    uint64_t next_capability=1;
    explicit Impl(Resolver r):resolver(std::move(r)) {}
    std::shared_ptr<Record> find(TargetId id) {
        auto i=records.find(id); return i==records.end() || !i->second->refs ? nullptr : i->second;
    }
    void collect() {
        for (auto i=records.begin();i!=records.end();) {
            auto& r=*i->second;
            if (!r.refs && !r.subscriptions && !r.hooked && r.binding->RemovalComplete() && i->second.use_count()==1 && r.binding->PruneCompletedTicket()) {
                physical.erase(key(r.resolution)); i=records.erase(i);
            } else ++i;
        }
        // Completed bridge tickets were pruned individually above. The global
        // drain retains its unrelated-callback quiescence requirement.
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
bool Service::SetCopyContext(s2fn::copy::Reader reader,const CopyProducer& engine) {
    auto checked=CheckedProducer(engine);
    if(!checked || checked.value.owner.domain!=s2fn::copy::Domain::Engine) return false;
    std::lock_guard<std::recursive_mutex> lock(impl_->mu);impl_->collect();
    if(!impl_->records.empty()) return false;
    impl_->reader=reader;impl_->engine=engine;return true;
}
s2fn::Result<TargetId> Service::Prepare(const std::string& name,const std::string& target,
                                       const std::string& abi,const std::string& fingerprint) {
    if (name.empty() || name.find('\0')!=std::string::npos) return {0,"invalid canonical id"};
    auto d=Parse(target,abi,fingerprint); if (!d) return {0,name+": "+d.error};
    return PrepareDeclaration(name,std::move(d.value));
}
s2fn::Result<TargetId> Service::PrepareDeclaration(const std::string& name,Declaration d) {
    if(d.HasCopies() && (!impl_->reader.available || !impl_->reader.read || !impl_->reader.page_size))
        return {0,name+": FunctionCopyLifetimeUnsupported: native reader unavailable"};
    auto resolution=Resolve(d,impl_->resolver); if (!resolution) return {0,name+": "+resolution.error};
    // The immutable resolver may contact the host. Only interning needs the lock.
    std::lock_guard<std::recursive_mutex> lock(impl_->mu); impl_->collect();
    auto physical=key(resolution.value); auto prior=impl_->physical.find(physical);
    if (prior!=impl_->physical.end()) {
        auto& r=*impl_->records.at(prior->second);
        if (r.declaration.info.fingerprint!=d.info.fingerprint || !r.declaration.CompatibleCopies(d) || !r.declaration.instances.compatible(d.instances))
            return {0,"ABI conflict: "+r.canonical_id+" and "+name};
        if (!r.refs) return {0,name+": physical target retirement pending"};
        if (r.refs==std::numeric_limits<size_t>::max()) return {0,"target reference overflow"};
        ++r.refs; return {r.id,{}};
    }
    if (impl_->next==std::numeric_limits<TargetId>::max()) return {0,"target id exhausted"};
    auto r=std::make_shared<Impl::Record>(*impl_,impl_->next,name,std::move(d),std::move(resolution.value));
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
namespace {
void instance_owner(const S2FunctionInstanceOwner& o,bool active) {
    require(o.version==1 && o.struct_size==56 && !o.reserved && (o.kind==1 || o.kind==2) && (!active || o.generation),"invalid instance owner");
    require(std::any_of(std::begin(o.id_digest),std::end(o.id_digest),[](uint8_t b){return b!=0;}),"empty instance owner identity");
}
}
s2fn::Result<S2FunctionInstancePrepared> Service::PrepareInstance(uint64_t binding,
    const S2FunctionInstanceOwner& owner,const std::string& name,const std::string& target,const std::string& contract) {
    TargetId acquired=0;
    try {
        require(std::this_thread::get_id()==impl_->owner,"instance preparation off host thread");
        instance_owner(owner,false);require(binding && !name.empty() && name.find('\0')==std::string::npos,"invalid instance binding/name");
        auto declaration=parse_instance(target,contract);
        auto capability=std::make_shared<FrameAccess::InstanceCapability>();
        capability->binding=binding;capability->owner=owner;capability->declaration=declaration;
        require(impl_->next_capability && impl_->next_capability<UINT64_MAX,"instance capability exhaustion");
        const auto id=impl_->next_capability++;
        auto prepared=PrepareDeclaration(name,std::move(declaration));require(bool(prepared),prepared.error.c_str());
        acquired=prepared.value;capability->target=acquired;
        impl_->capabilities.emplace(id,std::move(capability));
        return {{1,24,acquired,id},{}};
    } catch(const std::exception& e) {if(acquired) TargetRelease(acquired);return {{},e.what()};}
}
s2fn::Result<bool> Service::ActivateInstance(uint64_t id,const S2FunctionInstanceOwner& owner) {
    try {
        require(std::this_thread::get_id()==impl_->owner,"instance activation off host thread");instance_owner(owner,true);
        auto found=impl_->capabilities.find(id);require(found!=impl_->capabilities.end(),"unknown instance capability");
        auto& c=*found->second;
        require(!c.active && !c.released && c.owner.kind==owner.kind &&
            std::equal(std::begin(owner.id_digest),std::end(owner.id_digest),std::begin(c.owner.id_digest)) &&
            (!c.owner.generation || c.owner.generation==owner.generation),"instance activation owner/generation mismatch");
        c.owner=owner;c.active=true;return {true,{}};
    } catch(const std::exception& e) {return {false,e.what()};}
}
bool Service::ReleaseInstance(uint64_t id) {
    if(std::this_thread::get_id()!=impl_->owner) return false;
    auto found=impl_->capabilities.find(id);if(found==impl_->capabilities.end()) return true;
    found->second->active=false;found->second->released=true;impl_->capabilities.erase(found);return true;
}
s2fn::Result<S2FunctionValue> Service::Call(TargetId id,unsigned long long owner,
    const S2FunctionValue* args,int argc,S2FunctionValue request) {
    return CallImpl(id,owner,args,argc,request,nullptr,nullptr,nullptr);
}
s2fn::Result<S2FunctionValue> Service::CallCopy(TargetId id,unsigned long long owner,
    const S2FunctionValue* args,int argc,S2FunctionValue request,const CopyInput& input,CopyOutput& output,const CopyProducer& producer) {
    return CallImpl(id,owner,args,argc,request,&input,&output,&producer);
}
s2fn::Result<S2FunctionValue> Service::CallImpl(TargetId id,unsigned long long owner,
    const S2FunctionValue* args,int argc,S2FunctionValue request,const CopyInput* input,CopyOutput* output,const CopyProducer* producer) {
    std::shared_ptr<Impl::Record> r;
    PointerCodec* codec=nullptr;
    {
        std::lock_guard<std::recursive_mutex> lock(impl_->mu);
        r=impl_->find(id); if (!r) return {{},"target handle unavailable"};
        if (r->installing) return {{},"target registration pending admission"};
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
    if(r->declaration.instances.restricted()) return {{},"record/hidden native input cannot be manufactured"};
    const auto& abi=r->declaration.abi;
    size_t receiver=abi.member_receiver ? 1 : 0;
    if (argc<0 || static_cast<size_t>(argc)!=abi.parameters.size()+receiver || (argc && !args))
        return {{},"argument count mismatch"};
    if(r->declaration.HasCopies() && !input) return {{},"FunctionCopyLifetimeUnsupported: copied sidecar required"};
    s2fn::RetainedFrame copy_storage;
    CopyTransaction* copies=nullptr;
    s2fn::copy::Snapshot capture;
    if(input) {
        if(std::this_thread::get_id()!=impl_->owner) return {{},"copied call unavailable off host thread"};
        if(!valid_copy_input(*input)) return {{},"FunctionCopyLifetimeUnsupported: invalid copied input span"};
        auto checked=CheckedProducer(*producer);if(!checked) return {{},checked.error};
        for(const auto& p:r->declaration.copies) if(p.ownership==CopyOwnership::NativeObserved)
            return {{},"FunctionCopyLifetimeUnsupported: native-observed disallows call"};
        if(r->declaration.return_copy.ownership==CopyOwnership::NativeObserved)
            return {{},"FunctionCopyLifetimeUnsupported: native-observed disallows call"};
        auto state=CopyTransaction::Create(impl_->engine);if(!state) return {{},state.error};
        copy_storage=std::move(state.value);copies=static_cast<CopyTransaction*>(copy_storage.get());
        if(r->declaration.return_copy) {
            const auto kind=r->declaration.return_copy.kind;
            auto admitted=AdmitCopyOutput(kind,request,*output,kind==s2fn::copy::Kind::String ? s2fn::copy::MaxString : 12);
            if(!admitted) return {{},admitted.error};
            auto prepared=s2fn::copy::Snapshot::PrepareCapture(copies->EngineOperation(),kind);if(!prepared) return {{},prepared.error};
            capture=std::move(prepared.value);
        }
    }
    if (abi.returns.native=="ptr" && !r->declaration.return_copy && (!pointer_request(request) || !codec))
        return {{},"pointer result codec/request unavailable"};
    CallStorage storage; std::vector<s2fn::NativeValue> values; values.reserve(argc);
    for (int i=0;i<argc;++i) {
        const auto& v=args[i]; const std::string atom=receiver && i==0 ? "ptr" : abi.parameters[i-receiver].native;
        if (v.reserved || v.kind!=static_cast<unsigned char>(kind(atom))) return {{},"argument value kind/reserved mismatch"};
        if (atom=="ptr") {
            const CopyPosition* position=receiver && i==0 ? nullptr : &r->declaration.copies[i-receiver];
            if(position && *position) {
                auto staged=copies->Stage(i-receiver,position->kind,v,*input,*producer);if(!staged) return {{},r->canonical_id+" parameter["+std::to_string(i-receiver)+"]: "+staged.error};
                values.push_back({});continue;
            }
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
    // Reserve all vector/TLS slots before permanent publication or ffi_call.
    bypass.reserve(bypass.size()+1);
    struct Publication {
        CopyTransaction* copies;
        const Declaration& declaration;
        std::vector<s2fn::NativeValue>& values;
        size_t receiver;
    } publication{copies,r->declaration,values,receiver};
    auto publish=[](void* opaque)->s2fn::Result<bool> {
        auto& p=*static_cast<Publication*>(opaque);
        if(!p.copies) return {true,{}};
        auto result=p.copies->Publish(p.declaration.copies,p.declaration.abi.parameters.size(),false);
        if(!result) return {false,result.error};
        for(size_t i=0;i<p.declaration.abi.parameters.size();++i) if(p.declaration.copies[i])
            p.values[i+p.receiver]=s2fn::NativeValue::From(result.value[i]);
        return {true,{}};
    };
    bypass.push_back({impl_.get(),id,owner});
    struct Pop { ~Pop(){bypass.pop_back();} } pop;
    auto result=r->binding->Call(values.data(),values.size(),publish,&publication);
    if(!result) return {{},copies ? "FunctionCopyInvocationFailure: "+result.error : result.error};
    if(r->declaration.return_copy) {
        auto captured=s2fn::copy::Snapshot::CapturePrepared(std::move(capture),result.value.Get<uintptr_t>(),impl_->reader);
        if(!captured) return {{},r->canonical_id+": FunctionCopyCaptureAfterCall: native invocation completed: "+captured.error};
        return EncodeCopy(captured.value,request,*output); // borrowed inputs still alive, including a returned alias.
    }
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
    std::unique_lock<std::recursive_mutex> lock(impl_->mu);
    auto r=impl_->find(id); if (!r) return {0,"target handle unavailable"};
    if (!impl_->sink) return {0,"function dispatch sink unavailable"};
    if (r->installing) return {0,"target registration pending admission"};
    if (r->subscriptions==std::numeric_limits<size_t>::max()) return {0,"subscription overflow"};
    if (!r->subscriptions && !r->hooked) {
        if (r->active_calls) return {0,"hook acquisition requires idle target"};
        r->installing=true;
        lock.unlock();
        S2HookReceipt receipt;
        try { receipt=r->binding->Configure(reinterpret_cast<void*>(r->resolution.address)); }
        catch (...) { lock.lock();r->installing=false;throw; }
        lock.lock();r->installing=false;
        if (!receipt.Accepted()) return {0,"hook acquisition failed: "+receipt.reason};
        if (!r->refs) { r->binding->BeginRemove();return {0,"target retired during registration"}; }
        r->hooked=true;
    }
    ++r->subscriptions; return {static_cast<long long>(r->binding->Receipt().id)+1,{}};
}
bool Service::HookRelease(TargetId id) {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu);
    auto i=impl_->records.find(id); if (i==impl_->records.end() || !i->second->subscriptions) return false;
    auto& r=*i->second; if (--r.subscriptions==0 && !r.refs && r.hooked) { r.hooked=false; r.binding->BeginRemove(); }
    impl_->collect(); return true;
}
bool Service::TargetRelease(TargetId id) {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu);
    auto r=impl_->find(id); if (!r) return false;
    --r->refs;
    if (!r->refs && !r->subscriptions && r->hooked) { r->hooked=false; r->binding->BeginRemove(); }
    // Hook ledger entries release separately; retained subscriptions keep storage alive.
    r.reset(); impl_->collect(); return true;
}
S2HookReceipt Service::Receipt(TargetId id) const {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu);
    auto i=impl_->records.find(id);
    return i==impl_->records.end() ? S2HookReceipt{KHook::INVALID_HOOK,S2HookState::Failed,"target handle unavailable"} : i->second->binding->Receipt();
}
bool Service::Empty() const { std::lock_guard<std::recursive_mutex> lock(impl_->mu); return impl_->records.empty() && impl_->capabilities.empty(); }
bool Service::Collect() {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu); impl_->collect(); return impl_->records.empty() && impl_->capabilities.empty();
}
s2fn::Result<S2FunctionHookStatus> Service::HookStatus(TargetId id) const {
    std::lock_guard<std::recursive_mutex> lock(impl_->mu);
    if (!impl_->find(id)) return {{},"target handle unavailable"};
    const auto receipt=impl_->records.at(id)->binding->Receipt();
    unsigned state=0;
    switch (receipt.state) {
        case S2HookState::Pending: state=1; break;
        case S2HookState::Active: state=2; break;
        case S2HookState::Removing: state=3; break;
        case S2HookState::Removed: state=4; break;
        default: break;
    }
    return {{state,0,receipt.id==KHook::INVALID_HOOK ? 0 : static_cast<unsigned long long>(receipt.id)+1},{}};
}
namespace {
FrameAccess& copy_frame(CopyFrameKey key) {
    auto& f=frame_access(key.target,key.token,key.epoch,key.fingerprint);
    require(f.copies,"FunctionCopyLifetimeUnsupported: frame has no copied storage");return f;
}
size_t copy_index(FrameAccess& f,int selector) {
    if(selector==-2 || selector==-3) {
        require(f.native.phase==s2fn::Phase::Post,"return read is POST-only");
        require(selector!=-3 || !f.native.original_skipped,"original return unavailable: original skipped");
        return selector==-2 ? CopyTransaction::Return : CopyTransaction::Original;
    }
    require(selector>=0 && static_cast<size_t>(selector)<f.staged.size(),"invalid copied selector");
    return static_cast<size_t>(selector);
}
}
s2fn::Result<S2FunctionValue> FrameReadCopy(CopyFrameKey key,int selector,S2FunctionValue request,CopyOutput& out) {
    try {
        auto& f=copy_frame(key);const auto index=copy_index(f,selector);
        const auto& position=selector<0 ? f.declaration.return_copy : f.declaration.copies[index];
        require(bool(position),"selector is not copied");
        return EncodeCopy(f.copies->Read(index),request,out);
    } catch(const std::exception& e) {return {{},e.what()};}
}
s2fn::Result<bool> FrameWriteCopy(CopyFrameKey key,int selector,const S2FunctionValue& value,const CopyInput& input,const CopyProducer& producer) {
    try {
        auto& f=copy_frame(key);
        require(f.native.phase==s2fn::Phase::Pre && !f.committed,"frame is readonly");
        require(selector>=0 && static_cast<size_t>(selector)<f.staged.size(),"invalid copied write selector");
        const auto& position=f.declaration.copies[selector];
        require(position && position.ownership!=CopyOwnership::NativeObserved,"copied position is readonly");
        auto staged=f.copies->Stage(selector,position.kind,value,input,producer);if(!staged) return {false,f.canonical+" parameter["+std::to_string(selector)+"]: "+staged.error};
        f.changed=true;return {true,{}};
    } catch(const std::exception& e) {return {false,e.what()};}
}
s2fn::Result<bool> FrameCommitCopy(CopyFrameKey key,int action,const S2FunctionValue* value,const CopyInput& input,const CopyProducer& producer) {
    try {
        auto& f=copy_frame(key);
        require(f.native.phase==s2fn::Phase::Pre && !f.committed,"frame commit is PRE-only and single-use");
        require(action>=0 && action<=3,"invalid generic action");
        const auto k=kind(f.declaration.abi.returns.native);
        s2fn::NativeValue result;CallStorage storage;
        bool return_wins=false;
        if(action<2 || k==ValueKind::Void) require(!value,"unexpected suppression return");
        else {
            require(value,"missing typed suppression return");
            if(f.declaration.return_copy) {
                require(f.declaration.return_copy.ownership==CopyOwnership::CallerBorrowed,"FunctionCopyLifetimeUnsupported: native-observed disallows suppression");
                // Stock PRE strength remains owned by the provider; this batch
                // contains the host's final folded suppression candidate only.
                auto staged=f.copies->Stage(CopyTransaction::Return,f.declaration.return_copy.kind,*value,input,producer);
                if(!staged) return staged;
                return_wins=true;
            } else if(k==ValueKind::Pointer) {
                require(f.codec && pointer_request(*value),"pointer return codec/request unavailable");
                auto decoded=f.codec->Decode(*value,storage);require(bool(decoded),decoded.error.c_str());result=decoded.value;
            } else result=scalar_decode(*value,k);
        }
        // Allocate native slots and validate every scalar/entity edit before
        // publishing any permanent value in this mixed-producer transaction.
        auto staged=f.staged;
        for(const auto& edit:f.entity_edits) {
            require(f.codec,"entity codec unavailable");
            auto decoded=f.codec->Decode(edit.second,storage);require(bool(decoded),decoded.error.c_str());
            staged.at(edit.first)=decoded.value;
        }
        auto fields=prepare_fields(f);
        auto published=f.copies->Publish(f.declaration.copies,staged.size(),return_wins);
        if(!published) return {false,published.error};
        for(size_t i=0;i<staged.size();++i) if(published.value[i]) staged[i]=s2fn::NativeValue::From(published.value[i]);
        if(return_wins) result=s2fn::NativeValue::From(published.value[CopyTransaction::Return]);
        publish_fields(fields);
        f.native.arguments.swap(staged);f.native.changed=f.changed;f.native.result=result;
        f.native.action=action>=2 ? KHook::Action::Supersede : KHook::Action::Ignore;f.committed=true;
        return {true,{}};
    } catch(const std::exception& e) {return {false,e.what()};}
}
s2fn::Result<S2FunctionValue> FrameOverrideReturnCopy(CopyFrameKey key,const S2FunctionValue& value,
    const CopyInput& input,const CopyProducer& producer,S2FunctionValue request,CopyOutput& output) {
    bool submitted=false;
    try {
        auto& f=copy_frame(key);require(f.native.phase==s2fn::Phase::Post,"return override is POST-only");
        const auto& position=f.declaration.return_copy;
        require(position && position.ownership==CopyOwnership::CallerBorrowed,"FunctionCopyLifetimeUnsupported: copied return override unavailable");
        auto checked=CheckedProducer(producer);if(!checked) return {{},checked.error};
        auto operation=s2fn::copy::NativeBudget().Begin(checked.value);if(!operation) return {{},operation.error};
        auto candidate=DecodeCopy(operation.value,position.kind,value,input);if(!candidate) return {{},candidate.error};
        // Stock uses strictly-greater strength. A prior typed override (including
        // Supersede) beats this Override. Capture/readmission happens before Save.
        const bool wins=KHook::GetOverrideValuePtr()==nullptr;
        auto effective=wins ? candidate.value : f.copies->Read(CopyTransaction::Return);
        auto admitted=AdmitCopyOutput(position.kind,request,output,effective.size());if(!admitted) return {{},admitted.error};
        const void* pointer=f.native.result.Get<const void*>();
        if(wins) {
            auto intern=s2fn::copy::Arena::Resident().Intern(f.copies->EngineOperation(),&checked.value.owner,&candidate.value,1,&pointer);
            if(!intern) return {{},intern.error};
        }
        // All bytes/capacities are ready; Save can only select candidate/current.
        // This internal entry is reached only after the host's exact adapter
        // permit validation; producer identity is deliberately not that permit.
        submitted=true;f.binding.OverridePostReturn(f.native,s2fn::NativeValue::From(pointer));
        if(f.native.result.Get<const void*>()!=pointer) return {{},"FunctionCopyPostSubmitFailure: unexpected provider arbitration"};
        f.copies->AcceptReturn(effective);
        return EncodeCopy(effective,request,output);
    } catch(const std::exception& e) {return {{},submitted ? std::string("FunctionCopyPostSubmitFailure: ")+e.what() : e.what()};}
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
        auto declaration=s2bridge::Parse(target,abi,fingerprint);
        if(!declaration) {reason_out(reason,cap,declaration.error);return 0;}
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
extern "C" int S2_FunctionGetHookStatus(long long id,S2FunctionHookStatus* out,char* reason,int cap) {
    try {
        if (!out) throw std::runtime_error("missing hook status output");
        auto result=s2bridge::Global().HookStatus(id); reason_out(reason,cap,result && result.value.state==0 ? s2bridge::Global().Receipt(id).reason : result.error);
        if (!result) return 0; *out=result.value; return 1;
    } catch (const std::exception& e) {reason_out(reason,cap,e.what());return 0;}
      catch (...) {reason_out(reason,cap,"hook status exception");return 0;}
}
extern "C" int S2_FunctionFrameRead(long long id,unsigned long long token,unsigned long long epoch,
    const char* fp,int selector,unsigned char projection,S2FunctionValue* out,char* reason,int cap) {
    try {
        using namespace s2bridge;
        auto& f=frame_access(id,token,epoch,fp); require(out,"missing frame output");
        require(!f.declaration.instances.records.count(selector) && !f.declaration.instances.hidden.count(selector),"instance position requires capability operation");
        ValueKind k; s2fn::NativeValue value;
        if (selector==-1) {
            require(f.declaration.abi.member_receiver,"receiver unavailable");
            k=ValueKind::Pointer;value=f.native.receiver;
            require(out->flags==static_cast<unsigned char>(PointerProjection::Entity),"receiver requires strict entity projection");
        } else if (selector==-2 || selector==-3) {
            require(f.native.phase==s2fn::Phase::Post,"return read is POST-only");
            require(selector!=-3 || !f.native.original_skipped,"original return unavailable: original skipped");
            k=kind(f.declaration.abi.returns.native);
            value=selector==-3 ? f.native.original_result : f.native.result;
        } else {
            require(selector>=0 && static_cast<size_t>(selector)<f.staged.size(),"unsupported frame selector");
            k=kind(f.declaration.abi.parameters[selector].native); value=f.staged[selector];
        }
        require(!(selector>=0 ? bool(f.declaration.copies[selector]) : selector<-1 && bool(f.declaration.return_copy)),"copied position requires sidecar operation");
        require(projection==static_cast<unsigned char>(k),"projection kind mismatch");
        S2FunctionValue result{};
        if(k==ValueKind::Pointer) {
            require(f.codec,"entity codec unavailable");
            // Validate the caller's binding-local request before touching any slot.
            require(out->kind==8 && (out->flags==1 || out->flags==2) && !out->reserved && !out->aux && !out->bits,
                "invalid entity projection request");
            auto edit=f.entity_edits.find(selector);
            CallStorage storage;
            if(edit!=f.entity_edits.end()) {
                // Decode using writer's checked identity, encode using reader's request.
                auto identity=edit->second;identity.flags=static_cast<unsigned char>(PointerProjection::NullableEntity);
                auto decoded=f.codec->Decode(identity,storage);require(bool(decoded),decoded.error.c_str());value=decoded.value;
            }
            auto encoded=f.codec->Encode(value,*out);require(bool(encoded),encoded.error.c_str());result=encoded.value;
        } else result=scalar_copy(value,k);
        *out=result; reason_out(reason,cap,""); return 1;
    } catch (const std::exception& e) {reason_out(reason,cap,e.what());return 0;}
      catch (...) {reason_out(reason,cap,"frame read exception");return 0;}
}
extern "C" int S2_FunctionFrameOverrideReturn(long long id,unsigned long long token,unsigned long long epoch,
    const char* fp,const S2FunctionValue* value,S2FunctionValue* out,char* reason,int cap) {
    bool submitted=false;
    try {
        using namespace s2bridge;
        auto& f=frame_access(id,token,epoch,fp);
        require(f.native.phase==s2fn::Phase::Post,"return override is POST-only");
        const auto k=kind(f.declaration.abi.returns.native);
        require(k!=ValueKind::Void,"void return cannot be overridden");
        require(value && out,"missing override input/output");
        require(!f.declaration.return_copy,"copied return requires sidecar operation");
        s2fn::NativeValue native;
        CallStorage storage;
        if(k==ValueKind::Pointer) {
            require(f.codec,"entity codec unavailable");
            require(out->kind==8 && (out->flags==1 || out->flags==2) && !out->reserved && !out->aux && !out->bits,
                "invalid entity projection request");
            require(pointer_request(*value),"copied value requires sidecar operation");
            auto decoded=f.codec->Decode(*value,storage);require(bool(decoded),decoded.error.c_str());native=decoded.value;
        } else native=scalar_decode(*value,k);
        submitted=true;
        f.binding.OverridePostReturn(f.native,native);
        S2FunctionValue effective{};
        if(k==ValueKind::Pointer) {
            auto encoded=f.codec->Encode(f.native.result,*out);require(bool(encoded),encoded.error.c_str());effective=encoded.value;
        } else effective=scalar_copy(f.native.result,k);
        *out=effective;reason_out(reason,cap,"");return 1;
    } catch(const std::exception& e) {
        reason_out(reason,cap,submitted ? std::string("POST override submitted: readback failed: ")+e.what() : e.what());return 0;
    } catch(...) {reason_out(reason,cap,submitted ? "POST override submitted: readback exception" : "POST override exception");return 0;}
}
extern "C" int S2_FunctionFrameWrite(long long id,unsigned long long token,unsigned long long epoch,
    const char* fp,int selector,const S2FunctionValue* value,char* reason,int cap) {
    try {
        using namespace s2bridge;
        auto& f=frame_access(id,token,epoch,fp);
        require(f.native.phase==s2fn::Phase::Pre && !f.committed,"frame is readonly");
        require(value && selector>=0 && static_cast<size_t>(selector)<f.staged.size(),"invalid frame write selector");
        require(!f.declaration.instances.records.count(selector) && !f.declaration.instances.hidden.count(selector),"instance position requires capability operation");
        require(!f.declaration.copies[selector],"copied position requires sidecar operation");
        if(kind(f.declaration.abi.parameters[selector].native)==ValueKind::Pointer) {
            require(f.codec,"entity codec unavailable");CallStorage storage;
            require(pointer_request(*value),"copied value requires sidecar operation");
            auto decoded=f.codec->Decode(*value,storage);require(bool(decoded),decoded.error.c_str());
            f.entity_edits[selector]=*value; // Never retain the resolved address as the staged edit.
        } else f.staged[selector]=scalar_decode(*value,kind(f.declaration.abi.parameters[selector].native));
        f.changed=true; reason_out(reason,cap,""); return 1;
    } catch (const std::exception& e) {reason_out(reason,cap,e.what());return 0;}
      catch (...) {reason_out(reason,cap,"frame write exception");return 0;}
}
extern "C" int S2_FunctionFrameCommit(long long id,unsigned long long token,unsigned long long epoch,
    const char* fp,int action,const S2FunctionValue* value,char* reason,int cap) {
    try {
        using namespace s2bridge;
        auto& f=frame_access(id,token,epoch,fp);
        require(f.native.phase==s2fn::Phase::Pre && !f.committed,"frame commit is PRE-only and single-use");
        require(!f.declaration.HasCopies(),"copied frame requires sidecar commit");
        require(action>=0 && action<=3,"invalid generic action");
        auto k=kind(f.declaration.abi.returns.native);
        s2fn::NativeValue result;
        if (action<2 || k==ValueKind::Void) require(!value,"unexpected suppression return");
        else {
            require(value,"missing typed suppression return");
            if(k==ValueKind::Pointer) {
                require(f.codec,"entity codec unavailable");CallStorage storage;
                require(pointer_request(*value),"copied value requires sidecar operation");
                auto decoded=f.codec->Decode(*value,storage);require(bool(decoded),decoded.error.c_str());result=decoded.value;
            } else result=scalar_decode(*value,k);
        }
        auto staged=f.staged;CallStorage storage;
        for(const auto& edit:f.entity_edits) {
            require(f.codec,"entity codec unavailable");
            auto decoded=f.codec->Decode(edit.second,storage);require(bool(decoded),decoded.error.c_str());
            staged.at(edit.first)=decoded.value;
        }
        auto fields=prepare_fields(f);
        // All validation and allocation precede the final no-fail publication.
        publish_fields(fields);
        f.native.arguments.swap(staged); f.native.changed=f.changed;
        f.native.action=action>=2 ? KHook::Action::Supersede : KHook::Action::Ignore;
        f.native.result=result; f.committed=true; reason_out(reason,cap,""); return 1;
    } catch (const std::exception& e) {reason_out(reason,cap,e.what());return 0;}
      catch (...) {reason_out(reason,cap,"frame commit exception");return 0;}
}
namespace {
s2bridge::CopyInput copy_input(const S2FunctionCopyInput* p) {
    if(!p) throw std::runtime_error("FunctionCopyInvalidTransport: missing input span");
    return {p->version,p->struct_size,p->data,p->size};
}
s2bridge::CopyOutput copy_output(S2FunctionCopyOutput* p) {
    if(!p) throw std::runtime_error("FunctionCopyInvalidTransport: missing output span");
    return {p->version,p->struct_size,p->data,p->capacity,p->size};
}
s2bridge::CopyProducer copy_producer(const S2FunctionCopyProducer* p) {
    if(!p) throw std::runtime_error("FunctionCopyInvalidTransport: missing producer");
    s2bridge::CopyProducer out{p->version,p->struct_size,p->domain,p->reserved,{},p->generation};
    std::copy_n(p->digest,32,out.digest.begin());return out;
}
}
extern "C" int S2_FunctionCallCopy(long long target, unsigned long long owner, const S2FunctionValue* args, int argc, S2FunctionValue* ret, const S2FunctionCopyInput* input, S2FunctionCopyOutput* output, const S2FunctionCopyProducer* producer, char* reason, int reason_cap) {
    try {auto in=copy_input(input);auto out=copy_output(output);auto who=copy_producer(producer);if(!ret)throw std::runtime_error("missing return request");auto result=s2bridge::Global().CallCopy(target,owner,args,argc,*ret,in,out,who);reason_out(reason,reason_cap,result.error);if(!result)return 0;*ret=result.value;output->size=out.size;return 1;}
    catch(const std::exception& e){reason_out(reason,reason_cap,e.what());return 0;}
    catch(...){reason_out(reason,reason_cap,"FunctionCopyTransportFailure: native exception");return 0;}
}
extern "C" int S2_FunctionFrameReadCopy(long long target, unsigned long long token, unsigned long long epoch, const char* fingerprint, int selector, S2FunctionValue* value, S2FunctionCopyOutput* output, char* reason, int reason_cap) {
    try {auto out=copy_output(output);if(!value)throw std::runtime_error("missing read request");auto result=s2bridge::FrameReadCopy({target,token,epoch,fingerprint},selector,*value,out);reason_out(reason,reason_cap,result.error);if(!result)return 0;*value=result.value;output->size=out.size;return 1;}
    catch(const std::exception& e){reason_out(reason,reason_cap,e.what());return 0;}
    catch(...){reason_out(reason,reason_cap,"FunctionCopyTransportFailure: native exception");return 0;}
}
extern "C" int S2_FunctionFrameWriteCopy(long long target, unsigned long long token, unsigned long long epoch, const char* fingerprint, int selector, const S2FunctionValue* value, const S2FunctionCopyInput* input, const S2FunctionCopyProducer* producer, char* reason, int reason_cap) {
    try {auto in=copy_input(input);auto who=copy_producer(producer);if(!value)throw std::runtime_error("missing write value");auto result=s2bridge::FrameWriteCopy({target,token,epoch,fingerprint},selector,*value,in,who);reason_out(reason,reason_cap,result.error);if(!result)return 0;return 1;}
    catch(const std::exception& e){reason_out(reason,reason_cap,e.what());return 0;}
    catch(...){reason_out(reason,reason_cap,"FunctionCopyTransportFailure: native exception");return 0;}
}
extern "C" int S2_FunctionFrameCommitCopy(long long target, unsigned long long token, unsigned long long epoch, const char* fingerprint, int action, const S2FunctionValue* value, const S2FunctionCopyInput* input, const S2FunctionCopyProducer* producer, char* reason, int reason_cap) {
    try {auto in=copy_input(input);auto who=copy_producer(producer);auto result=s2bridge::FrameCommitCopy({target,token,epoch,fingerprint},action,value,in,who);reason_out(reason,reason_cap,result.error);if(!result)return 0;return 1;}
    catch(const std::exception& e){reason_out(reason,reason_cap,e.what());return 0;}
    catch(...){reason_out(reason,reason_cap,"FunctionCopyTransportFailure: native exception");return 0;}
}
extern "C" int S2_FunctionFrameOverrideReturnCopy(long long target, unsigned long long token, unsigned long long epoch, const char* fingerprint, const S2FunctionValue* value, const S2FunctionCopyInput* input, const S2FunctionCopyProducer* producer, S2FunctionValue* effective, S2FunctionCopyOutput* output, char* reason, int reason_cap) {
    try {auto in=copy_input(input);auto out=copy_output(output);auto who=copy_producer(producer);if(!value || !effective)throw std::runtime_error("missing override request");auto result=s2bridge::FrameOverrideReturnCopy({target,token,epoch,fingerprint},*value,in,who,*effective,out);reason_out(reason,reason_cap,result.error);if(!result)return 0;*effective=result.value;output->size=out.size;return 1;}
    catch(const std::exception& e){reason_out(reason,reason_cap,e.what());return 0;}
    catch(...){reason_out(reason,reason_cap,"FunctionCopyTransportFailure: native exception");return 0;}
}

namespace {
template<class F> int instance_boundary(char* reason,int cap,F&& f) {
    try { f();reason_out(reason,cap,"");return 1; }
    catch(const std::exception& e) {reason_out(reason,cap,e.what());return 0;}
    catch(...) {reason_out(reason,cap,"native instance exception");return 0;}
}
}
extern "C" int S2_FunctionPrepareInstance(unsigned long long binding,const S2FunctionInstanceOwner* owner,
    const char* name,const char* target,const char* contract,S2FunctionInstancePrepared* out,char* reason,int cap) {
    return instance_boundary(reason,cap,[&] {
        s2bridge::require(owner && name && target && contract && out,"missing instance preparation input");
        auto result=s2bridge::Global().PrepareInstance(binding,*owner,name,target,contract);
        s2bridge::require(bool(result),result.error.c_str());*out=result.value;
    });
}
extern "C" int S2_FunctionInstanceActivate(unsigned long long capability,const S2FunctionInstanceOwner* owner,char* reason,int cap) {
    return instance_boundary(reason,cap,[&] {
        s2bridge::require(owner,"missing instance owner");auto result=s2bridge::Global().ActivateInstance(capability,*owner);
        s2bridge::require(bool(result),result.error.c_str());
    });
}
extern "C" int S2_FunctionInstanceRelease(unsigned long long capability) {
    try {return s2bridge::Global().ReleaseInstance(capability) ? 1 : 0;} catch(...) {return 0;}
}
extern "C" int S2_FunctionFrameReadInstance(const S2FunctionInstanceAccess* access,int selector,S2FunctionValue* out,char* reason,int cap) {
    return instance_boundary(reason,cap,[&] {
        using namespace s2bridge;require(access && out,"missing instance access/output");
        auto [frame,c]=instance_access(*access);auto& f=*frame;
        require(!c->declaration.instances.hidden.count(selector),"hidden native position has no projection");
        if(c->declaration.instances.records.count(selector)) {
            const auto pointer=record_pointer(f,*c,selector);S2FunctionValue v{};v.kind=1;v.bits=pointer ? 1 : 0;*out=v;return;
        }
        if(selector==-1) {
            require(c->declaration.abi.member_receiver && f.codec,"receiver unavailable");
            S2FunctionValue request{};request.kind=8;request.flags=c->declaration.instances.nullable_receiver ? 2 : 1;
            auto value=f.codec->Encode(f.native.receiver,request);require(bool(value),value.error.c_str());*out=value.value;return;
        }
        char why[512]{};
        require(S2_FunctionFrameRead(f.target,f.info.frame_token,f.info.native_epoch,c->declaration.info.fingerprint.c_str(),selector,out->kind,out,why,512)==1,why);
    });
}
extern "C" int S2_FunctionFrameFieldRead(const S2FunctionInstanceAccess* access,int selector,unsigned int field,S2FunctionValue* out,char* reason,int cap) {
    return instance_boundary(reason,cap,[&] {
        using namespace s2bridge;require(access && out,"missing record access/output");
        auto [frame,c]=instance_access(*access);auto& f=*frame;const auto& row=record_field(f,*c,selector,field,false);
        auto pointer=record_pointer(f,*c,selector);require(pointer,"null record has no fields");
        S2FunctionValue value{};uint64_t bits=0;
        auto staged=f.field_edits.find({selector,row.offset});
        if(staged!=f.field_edits.end()) bits=field_bits(f,row,staged->second.value);
        else std::memcpy(&bits,reinterpret_cast<const void*>(pointer+row.offset),row.width);
        if(row.storage=="entity-handle32") {
            value.kind=8;value.flags=row.nullable ? 2 : 1;
            if(bits==UINT32_MAX) {require(row.nullable,"null strict entity field");value.aux=UINT32_MAX;}
            else {value.aux=bits&0x7fff;value.bits=bits>>15;}
            require(f.codec,"record entity codec unavailable");CallStorage storage;
            auto decoded=f.codec->Decode(value,storage);require(bool(decoded),decoded.error.c_str());
            auto request=value;request.aux=0;request.bits=0;
            auto encoded=f.codec->Encode(decoded.value,request);require(bool(encoded),encoded.error.c_str());value=encoded.value;
        } else {
            value.kind=static_cast<uint8_t>(kind(row.storage=="bool" ? "u8" : row.storage=="u16" ? "u32" : row.storage));value.bits=bits;
            field_bits(f,row,value);
        }
        *out=value;
    });
}
extern "C" int S2_FunctionFrameFieldWrite(const S2FunctionInstanceAccess* access,int selector,unsigned int field,const S2FunctionValue* value,char* reason,int cap) {
    return instance_boundary(reason,cap,[&] {
        using namespace s2bridge;require(access && value,"missing record access/value");
        auto [frame,c]=instance_access(*access);auto& f=*frame;const auto& row=record_field(f,*c,selector,field,true);
        require(record_pointer(f,*c,selector),"null record has no fields");field_bits(f,row,*value);
        f.field_edits[{selector,row.offset}]={c,field,*value};f.changed=true;
    });
}
// Authority/usage failures (expired frame, foreign binding, non-hidden position,
// malformed identity) are named errors. A stale entity or an unreadable/unequal
// field is `false`: the answer is only ever "provably referenced" or not.
extern "C" int S2_FunctionFrameHiddenReferencedBy(const S2FunctionInstanceAccess* access,int selector,
    const S2FunctionValue* entity,unsigned int offset,int* out,char* reason,int cap) {
    if(out) *out=0;
    return instance_boundary(reason,cap,[&] {
        using namespace s2bridge;require(access && entity && out,"missing relationship access/entity/output");
        auto [frame,c]=instance_access(*access);auto& f=*frame;
        require(c->declaration.instances.hidden.count(selector),"relationship check requires a hidden native position");
        require(f.native.phase==s2fn::Phase::Pre || f.native.phase==s2fn::Phase::Post,"relationship check unavailable in this phase");
        require(offset<=INT32_MAX,"relationship field offset out of range");
        require(entity->kind==8 && entity->flags==static_cast<unsigned char>(PointerProjection::Entity) && !entity->reserved &&
            entity->aux<=INT32_MAX && entity->bits<=UINT32_MAX,"invalid entity identity transport");
        require(f.codec && f.reader,"relationship codec/reader unavailable");
        // Serial-gated host books resolve the owner; a stale identity is simply unrelated.
        CallStorage storage;auto owner=f.codec->Decode(*entity,storage);
        if(!owner || !owner.value.Get<void*>()) return;
        const auto hidden=selector==-1 ? f.native.receiver.Get<uintptr_t>() : f.native.arguments.at(selector).Get<uintptr_t>();
        auto related=ReferencesHidden(*f.reader,owner.value.Get<uintptr_t>(),offset,hidden);
        require(bool(related),related.error.c_str());*out=related.value ? 1 : 0;
    });
}
static_assert(sizeof(S2FunctionInstanceOwner)==56 && offsetof(S2FunctionInstanceOwner,generation)==48,"instance owner v1");
static_assert(sizeof(S2FunctionInstancePrepared)==24 && offsetof(S2FunctionInstancePrepared,capability)==16,"instance prepared v1");
static_assert(sizeof(S2FunctionInstanceAccess)==48 && offsetof(S2FunctionInstanceAccess,binding_id)==40,"instance access v1");
#endif
