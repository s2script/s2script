// Exercises the shipped evaluator over a verified ELF image and separate live data.
#include "engine_resolver.h"
#include "vtable.h"
#include <cstring>
#include <iostream>
#include <vector>

static int failures = 0;
#define CHECK(c, label) do { if (!(c)) { std::cerr << "FAIL: " << label << "\n"; ++failures; } } while (0)
namespace {
void put(std::vector<uint8_t>& b, size_t at, uint64_t value, size_t n) {
    for (size_t i = 0; i < n; ++i) b.at(at+i) = uint8_t(value >> (8*i));
}
struct Fixture {
    static constexpr uintptr_t base = 0x100000;
    std::vector<uint8_t> file = std::vector<uint8_t>(0x2200);
    std::vector<uint8_t> live = std::vector<uint8_t>(0x5000);
    std::shared_ptr<const s2original::Image> image;
    std::string reason;
    Fixture() {
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
        s2original::Identity id{7,9,base,"abcd1234"};
        image=s2original::FromElf(file,id,id,{{base,base+0x800,0,7,9},
            {base+0x1000,base+0x1400,0x1000,7,9},
            {base+0x3000,base+0x4000,0x2000,7,9}},reason);
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
s2resolve::TargetRecipe recipe(const char* pattern, const char* strategy="direct") {
    s2resolve::TargetRecipe r; r.module="fixture"; r.pattern=pattern; r.strategy=strategy; return r;
}
const char* caller_validation=R"({"string-xref":{"at":5,"dispOff":3,"instrLen":7,"expect":"ScopeGood"}})";
void direct_and_closed_recipes() {
    Fixture f; f.bytes(0x1000,{0x41,0x56,0x55,0xc3}); f.freeze();
    auto r=recipe("41 56 55 C3"); r.validate_json=R"({"prologue":"41 56 55"})";
    s2resolve::Resolution out; std::string why;
    CHECK(s2resolve::Evaluate(r,f.sources(),out,why) && out.address==0x101000,
          "peer-patched direct prologue resolves and validates original bytes");
    CHECK(out.image==f.image && !out.validation_receipt.empty(), "resolution retains verified image and receipt");
    r.strategy="unknown";
    CHECK(!s2resolve::Evaluate(r,f.sources(),out,why) && why.find("strategy")!=std::string::npos,
          "unknown resolver strategy fails closed");
    CHECK(out.address==0 && !out.image, "failed resolution clears stale success");
    r.strategy="direct"; r.validate_json=R"({"unknown":true})";
    CHECK(!s2resolve::Evaluate(r,f.sources(),out,why), "unknown validator fails closed");
    r.validate_json="{}"; r.pattern="55 48 89 E5";
    CHECK(!s2resolve::Evaluate(r,f.sources(),out,why) && why.find("ambiguous")!=std::string::npos,
          "two direct matches fail ambiguous");
}
void validated_call_sites() {
    Fixture f; f.call(0x1000,0x1200,0x520); f.call(0x1100,0x1300,0x500); f.freeze();
    auto r=recipe("E8 ? ? ? ? 4C 8D 35 ? ? ? ?","validated-call"); r.validate_json=caller_validation;
    s2resolve::Resolution out; std::string why;
    CHECK(s2resolve::Evaluate(r,f.sources(),out,why) && out.address==0x101300,
          "two raw matches with one validated call site select second callee");
    f.call(0x1000,0x1200,0x500); f.freeze();
    CHECK(!s2resolve::Evaluate(r,f.sources(),out,why) && why.find("ambiguous")!=std::string::npos,
          "two validated call sites fail ambiguous");
    f.call(0x1000,0x1200,0x520); f.call(0x1100,0x3000,0x500); f.freeze();
    CHECK(!s2resolve::Evaluate(r,f.sources(),out,why), "validated call cannot target mapped data");
    r.validate_json="{}";
    CHECK(!s2resolve::Evaluate(r,f.sources(),out,why), "validated call requires a call-site string validator");
}
void multiple_segments_and_instruction_bounds() {
    Fixture f;
    // A second executable PT_LOAD has a different file offset and logical PC.
    put(f.file,56,5,2);
    const size_t ph=64+4*56;
    put(f.file,ph,1,4); put(f.file,ph+4,5,4); put(f.file,ph+8,0x1400,8);
    put(f.file,ph+16,0x2000,8); put(f.file,ph+32,0x100,8); put(f.file,ph+40,0x100,8);
    put(f.file,ph+48,1,8);
    f.bytes(0x1400,{0x41,0x56,0x55,0xc3});
    f.call(0x1000,0x2000,0x500);
    s2original::Identity id{7,9,Fixture::base,"abcd1234"};
    f.image=s2original::FromElf(f.file,id,id,{{0x100000,0x100800,0,7,9},
        {0x101000,0x101400,0x1000,7,9},{0x102000,0x102100,0x1400,7,9},
        {0x103000,0x104000,0x2000,7,9}},f.reason);
    CHECK(f.image,"multiple executable segments are verified");
    auto r=recipe("E8 ? ? ? ? 4C 8D 35 ? ? ? ?","validated-call");
    r.validate_json=caller_validation;
    s2resolve::Resolution out; std::string why;
    CHECK(s2resolve::Evaluate(r,f.sources(),out,why) && out.address==0x102000,
          "validated call crosses executable segments using logical PCs");
    r=recipe("41 56 55 C3"); r.validate_json=R"({"prologue":"41 56 55"})";
    CHECK(s2resolve::Evaluate(r,f.sources(),out,why) && out.address==0x102000,
          "direct scan and prologue validation include second executable segment");

    // Adjacent loads must not let a one-byte match borrow its operand from a
    // different original segment, even when that operand happens to be readable.
    std::fill(f.file.begin()+0x1000,f.file.begin()+0x100c,0x90);
    put(f.file,ph+16,0x1400,8); f.call(0x13ff,0x1200,0x500);
    f.image=s2original::FromElf(f.file,id,id,{{0x100000,0x100800,0,7,9},
        {0x101000,0x101400,0x1000,7,9},{0x101400,0x101500,0x1400,7,9},
        {0x103000,0x104000,0x2000,7,9}},f.reason);
    CHECK(f.image,"adjacent executable segments are verified");
    r=recipe("E8","validated-call"); r.validate_json=caller_validation;
    CHECK(!s2resolve::Evaluate(r,f.sources(),out,why),
          "whole rel32 instruction must fit within one original executable segment");

    Fixture edge; edge.bytes(0x13fd,{0x4c,0x8d,0x35}); edge.freeze();
    r=recipe("4C 8D 35","lea-disp"); r.use=s2resolve::TargetUse::MappedAddress;
    CHECK(!s2resolve::Evaluate(r,edge.sources(),out,why) && why.find("bounds")!=std::string::npos,
          "truncated LEA operand cannot cross an executable segment boundary");
    edge.bytes(0x13ff,{0xe8}); edge.freeze();
    r=recipe("E8","validated-call"); r.validate_json=caller_validation;
    CHECK(!s2resolve::Evaluate(r,edge.sources(),out,why),
          "truncated call site and caller validator fail without a live code fallback");
}
void data_targets() {
    for (const char* strategy : {"lea-disp","ctor-body-xref"}) {
        Fixture f;
        f.lea(0x1000,0x3800); // zero-filled BSS, outside file-backed data
        f.file[0x1007]=0xe8; put(f.file,0x1008,0x1200-0x100c,4); f.freeze();
        auto r=recipe(std::strcmp(strategy,"lea-disp")==0 ? "4C 8D 35 ? ? ? ?" : "55 48 89 E5 C3",strategy);
        s2resolve::Resolution out; std::string why;
        CHECK(!s2resolve::Evaluate(r,f.sources(),out,why), "executable consumers reject derived BSS address");
        r.use=s2resolve::TargetUse::MappedAddress;
        CHECK(s2resolve::Evaluate(r,f.sources(),out,why) && out.address==0x103800,
              "explicit mapped-address recipe resolves BSS");
        CHECK(out.validation_receipt.find("MappedAddress")!=std::string::npos,
              "data-target choice is retained in validation receipt");
        f.lea(0x1000,0x3100); f.freeze();
        CHECK(s2resolve::Evaluate(r,f.sources(),out,why) && out.address==0x103100,
              "explicit mapped-address recipe resolves file-backed writable data");
        r.validate_json=R"({"prologue":"00"})";
        CHECK(!s2resolve::Evaluate(r,f.sources(),out,why), "instruction validators cannot pass against a data target");
        r.validate_json.clear();
        for (size_t target : {0x2000,0x5000}) {
            f.lea(0x1000,target); f.freeze();
            auto sources=f.sources();
            sources.read_live=[](uintptr_t,void*,size_t) { CHECK(false,"invalid derived target must not be read"); return false; };
            CHECK(!s2resolve::Evaluate(r,sources,out,why), "mapped gap or out-of-range derivation is rejected");
        }
    }
}
void verified_rtti_data() {
    Fixture f; f.freeze();
    std::memcpy(f.live.data()+0x540,"12FixtureClass",15);
    put(f.live,0x3108,0x100540,8); // type_info+8 -> name
    put(f.live,0x3200,0,8); put(f.live,0x3208,0x103100,8); // offset-to-top + type_info
    put(f.live,0x3210,0x101200,8);
    auto read=f.sources().read_live;
    CHECK(s2vtable::GetVTableByName(*f.image,"FixtureClass",read)==reinterpret_cast<void**>(0x103210),
          "RTTI primary vtable comes from bounded live data in selected verified image");
    CHECK(s2vtable::GetVTableByName(*f.image,"Missing",read)==nullptr,
          "missing RTTI name has no pointer arithmetic fallback");
    put(f.live,0x3300,0,8); put(f.live,0x3308,0x103100,8); put(f.live,0x3310,0x101300,8);
    CHECK(s2vtable::GetVTableByName(*f.image,"FixtureClass",read)==nullptr,
          "two primary vtable chains fail ambiguous instead of choosing first");
    auto deny=[](uintptr_t,void*,size_t) { return false; };
    CHECK(s2vtable::GetVTableByName(*f.image,"FixtureClass",deny)==nullptr,
          "RTTI reader refusal fails closed");
}
void patched_virtual() {
    Fixture f; f.bytes(0x1200,{0x48,0x89,0x55,0xf8,0xc3}); f.freeze();
    put(f.live,0x3000,0xf00000,8); // live peer JIT pointer outside executable module
    auto s=f.sources();
    s.ops.vtable_from_image=[](const char*) -> void** { return reinterpret_cast<void**>(0x103000); };
    s.ops.original_virtual=[](void**,int i) -> void* { return i==0 ? reinterpret_cast<void*>(0x101200) : nullptr; };
    s2resolve::VirtualSlotResolution slot;
    std::string why;
    CHECK(s2resolve::EvaluateVirtualSlot("fixture","FixtureClass",0,s,slot,why) &&
              slot.target.address==0x101200 && slot.target.image==f.image &&
              slot.vtable==reinterpret_cast<void**>(0x103000) && slot.vtable_index==0,
          "structural virtual lookup judges the provider original and retains vtable identity");
    CHECK(slot.target.validation_receipt.find("FixtureClass")!=std::string::npos &&
              slot.target.validation_receipt.find("abcd1234")!=std::string::npos,
          "structural virtual lookup receipts class and verified build identity");
    auto r=recipe(""); r.kind=s2resolve::Kind::Virtual; r.class_name="FixtureClass"; r.vtable_index=0;
    r.validate_json=R"({"prologue":"48 89 55 F8","vtable-member":"FixtureClass"})";
    s2resolve::Resolution out;
    CHECK(s2resolve::Evaluate(r,s,out,why) && out.address==0x101200,
          "patched virtual resolves original before prologue and membership checks");
    r.validate_json="{}";
    CHECK(!s2resolve::Evaluate(r,s,out,why), "virtual recipe requires prologue validation");
    r.validate_json=R"({"prologue":"48 89 55 F8"})"; r.vtable_index=511;
    s.ops.vtable_from_image=[](const char*) -> void** { return reinterpret_cast<void**>(0x103ff8); };
    CHECK(!s2resolve::Evaluate(r,s,out,why), "virtual slot outside bounded live mapping is rejected before callback");
    CHECK(!s2resolve::EvaluateVirtualSlot("fixture","FixtureClass",511,s,slot,why) &&
              why.find("readable")!=std::string::npos,
          "structural virtual lookup rejects an incomplete live slot read");

    s=f.sources();
    CHECK(!s2resolve::EvaluateVirtualSlot("fixture","FixtureClass",0,s,slot,why) &&
              why.find("RTTI")!=std::string::npos,
          "structural virtual lookup requires RTTI resolution");
    s.ops.vtable_from_image=[](const char*) -> void** { return reinterpret_cast<void**>(0x103000); };
    CHECK(!s2resolve::EvaluateVirtualSlot("fixture","FixtureClass",0,s,slot,why) &&
              why.find("original")!=std::string::npos,
          "structural virtual lookup requires the stock original provider contact");
    s.ops.original_virtual=[](void**,int) -> void* { return nullptr; };
    CHECK(!s2resolve::EvaluateVirtualSlot("fixture","FixtureClass",0,s,slot,why) &&
              why.find("original")!=std::string::npos,
          "structural virtual lookup rejects a missing provider original");
    s.ops.original_virtual=[](void**,int) -> void* { return reinterpret_cast<void*>(0xf00000); };
    CHECK(!s2resolve::EvaluateVirtualSlot("fixture","FixtureClass",0,s,slot,why) &&
              why.find("executable")!=std::string::npos,
          "structural virtual lookup rejects a provider original outside the verified image");
    CHECK(!s2resolve::EvaluateVirtualSlot("fixture","",0,s,slot,why) &&
              why.find("class/index")!=std::string::npos,
          "structural virtual lookup rejects an empty class");
    CHECK(!s2resolve::EvaluateVirtualSlot("fixture","FixtureClass",512,s,slot,why) &&
              why.find("class/index")!=std::string::npos,
          "structural virtual lookup rejects an out-of-range index");
}
// Two classes can share a byte-identical override (a thin thunk). The pattern alone cannot pick
// one; the entry's own vtable-member validator can, and only an exactly-one survivor is accepted.
void validators_break_direct_ties() {
    Fixture f;
    f.bytes(0x1200,{0x48,0x85,0xf6,0x74,0x01,0xc3});
    f.bytes(0x1300,{0x48,0x85,0xf6,0x74,0x01,0xc3});
    f.freeze();
    put(f.live,0x3000,0x101300,8); // FixtureClass primary vtable: slot 0 is the SECOND twin
    auto s=f.sources();
    s.ops.vtable_from_image=[](const char*) -> void** { return reinterpret_cast<void**>(0x103000); };
    auto r=recipe("48 85 F6 74 01 C3");
    s2resolve::Resolution out; std::string why;
    CHECK(!s2resolve::Evaluate(r,s,out,why) && why.find("ambiguous")!=std::string::npos,
          "twins without a validator stay ambiguous");
    r.validate_json=R"({"vtable-member":"FixtureClass"})";
    CHECK(s2resolve::Evaluate(r,s,out,why) && out.address==0x101300,
          "vtable-member selects the one twin that is a slot of the class");
    put(f.live,0x3008,0x101200,8); // both twins are now slots
    CHECK(!s2resolve::Evaluate(r,s,out,why) && why.find("2 passed")!=std::string::npos,
          "two validated twins fail ambiguous and say how many passed");
    put(f.live,0x3000,0x101400,8); put(f.live,0x3008,0,8); // neither twin is a slot
    CHECK(!s2resolve::Evaluate(r,s,out,why) && why.find("0 passed")!=std::string::npos,
          "no validated twin fails and names the validator result");
    r.strategy="lea-disp";
    CHECK(!s2resolve::Evaluate(r,s,out,why) && why.find("ambiguous (>1 match)")!=std::string::npos,
          "derived strategies keep the strict single-match rule");
}
}
int main() {
    verified_rtti_data(); direct_and_closed_recipes(); validated_call_sites();
    multiple_segments_and_instruction_bounds(); data_targets(); patched_virtual();
    validators_break_direct_ties();
    if (failures) { std::cerr << "engine_resolver_test: " << failures << " failures\n"; return 1; }
    std::cout << "engine_resolver_test: all passed\n";
}
