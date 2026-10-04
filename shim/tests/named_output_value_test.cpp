// Include the production adapter body, with only engine services substituted.
// The pinned SDK owns the variant definition, constructors and destructor.
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstddef>
#include <cstdint>
#include "variant.h"
#include "tier0/memalloc.h"
#include <string>
#include <vector>
#include <functional>
#include <sys/wait.h>
#include <unistd.h>

// The SDK contains unused default allocator branches referring to this symbol.
// Owned capture in this fixture uses the explicit TestAlloc/TestFree services below.
IMemAlloc* g_pMemAlloc=nullptr;

struct CPulseArgumentPack;
struct CPulseInputParamMap;
struct EntityIOOutputDesc_t { const char* m_pName; };
class CEntityIOOutput { public: EntityIOOutputDesc_t* m_pDesc; };
class CEntityInstance {
public:
    int handle;
    const char* GetClassname() const { return "logic_relay"; }
    struct Handle { int value; int ToInt() const { return value; } };
    Handle GetRefEHandle() const { return {handle}; }
};
static int failures=0, allocations=0, frees=0, pack_reads=0;
static std::string observed;
static std::vector<std::string> deliveries;
static std::function<void()> nested_dispatch;
static bool expect_clean=false;
static int dispatches=0;
#define CHECK(c,why) do { if (!(c)) { std::fprintf(stderr,"FAIL: %s\n",why); ++failures; } } while(0)
static void* TestAlloc(int n) { ++allocations; return std::malloc(n); }
static void TestFree(void* p) { ++frees; std::free(p); }
#ifdef MemAlloc_Alloc
#undef MemAlloc_Alloc
#endif
#ifdef MemAlloc_Free
#undef MemAlloc_Free
#endif
#define MemAlloc_Alloc(n) TestAlloc(n)
#define MemAlloc_Free(p) TestFree(p)
static int s2script_core_dispatch_output(const char* cls,const char* name,int act,int caller,
                                         const char* value,float delay) {
    CHECK(std::string(cls)=="logic_relay" && std::string(name)=="OnUser1","output identity preserved");
    CHECK(act==17 && caller==23 && delay==1.25f,"handles and float delay preserved");
    if(expect_clean) CHECK(allocations==frees,"engine captured value released before plugin entry");
    ++dispatches;
    observed=value;
    deliveries.emplace_back(value);
    const std::string before(value);
    if(nested_dispatch) {
        auto nested=std::move(nested_dispatch); nested_dispatch=nullptr; nested();
        CHECK(std::string(value)==before,"nested output preserves its caller's copied value");
    }
    return 2;
}

#include "named_output_adapter.inc"

// Invoke its real declared function type: no mismatched function pointer ABI cast.
template<class Fourth,class Sixth,class Seventh>
static int Invoke(int (*fn)(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,Fourth,float,Sixth,Seventh),
                  void* pack,const CVariant* value) {
    EntityIOOutputDesc_t desc{"OnUser1"}; CEntityIOOutput output{&desc};
    CEntityInstance act{17},caller{23};
    return fn(&output,&act,&caller,reinterpret_cast<Fourth>(pack),1.25f,
              reinterpret_cast<Sixth>(uintptr_t{0x2222}),
              reinterpret_cast<Seventh>(const_cast<CVariant*>(value)));
}


// Substitutes only the external engine wrapper. Its complete native implementation
// is separately checked by the matched-image audit and live output smoke test.
struct CPulseArgumentPack { std::vector<CVariant*> values; };
static void ExtractFirst(OutputCapturedVariant* destination,CPulseArgumentPack* arguments) {
    ++pack_reads;
    CHECK(destination->IsNull(),"capture initialized before empty-pack wrapper call");
    if(!arguments->values.empty()) arguments->values.front()->AssignTo(destination);
}

static void BorrowedFormatting() {
    CVariant value;
    auto check=[&](const char* expected) {
        CHECK(Invoke(&S2NamedOutputOp,nullptr,&value)==2,"formatter preserves verdict");
        CHECK(observed==expected,"formatter preserves supported value representation");
    };
    check("");
    value.m_type=FIELD_INT32; value.m_int32=-2147483647; check("-2147483647");
    value.m_type=FIELD_UINT32; value.m_uint32=4294967295U; check("4294967295");
    value.m_type=FIELD_INT64; value.m_int64=INT64_MIN; check("-9223372036854775808");
    value.m_type=FIELD_UINT64; value.m_uint64=UINT64_MAX; check("18446744073709551615");
    value.m_type=FIELD_FLOAT32; value.m_float32=1.5f; check("1.5");
    value.m_type=FIELD_FLOAT64; value.m_float64=-17.5; check("-17.5");
    value.m_type=FIELD_BOOLEAN; value.m_bool=true; check("true");
    value.m_bool=false; check("false");
    value.m_type=FIELD_CHARACTER; value.m_char='x'; check("x");
    value.m_type=FIELD_STRING; value.m_stringt=castable_string_t("string_t"); check("string_t");
    value.m_stringt=castable_string_t(); check("");
    value.m_type=FIELD_CSTRING; value.m_pszString="cstring"; check("cstring");
    value.m_pszString=nullptr; check("");
    Vector vector(1,2,3); value.m_type=FIELD_VECTOR; value.m_pVector=&vector; check("1 2 3");
    value.m_pVector=nullptr; check("");
    QAngle angle(4,5,6); value.m_type=FIELD_QANGLE; value.m_pQAngle=&angle; check("4 5 6");
    value.m_pQAngle=nullptr; check("");
    value.m_type=FIELD_COLOR32; value.m_pData=nullptr; check("");
    std::string long_text(300,'z');
    value.m_type=FIELD_CSTRING; value.m_pszString=long_text.c_str();
    Invoke(&S2NamedOutputOp,nullptr,&value);
    CHECK(observed==std::string(255,'z'),"existing 256-byte bounded string formatting preserved");
    char untouched='q'; CVariantToString(&value,&untouched,0);
    CHECK(untouched=='q',"zero-capacity formatting does not write");
    CHECK(allocations==0 && frees==0,"borrowed variants never enter capture allocator");
}

static void PackCaptureAndLifetime() {
    s_pPulseFirstArgumentToVariant=&ExtractFirst; expect_clean=true;
    CPulseArgumentPack empty;
    Invoke(&S2NamedOutputOp,&empty,nullptr);
    CHECK(observed.empty(),"empty pack publishes empty value");
    Invoke(&S2NamedOutputOp,nullptr,nullptr);
    CHECK(observed.empty(),"absent pack and optional value are safe");
    CVariant first("first-pack-string",false),second("second-unused-string",false);
    CPulseArgumentPack pack{{&first,&second}};
    const int reads_before=pack_reads;
    Invoke(&S2NamedOutputOp,&pack,&second);
    CHECK(observed=="second-unused-string" && pack_reads==reads_before,
          "optional value takes precedence without extracting pack");
    Invoke(&S2NamedOutputOp,&pack,nullptr);
    CHECK(observed=="first-pack-string","first pack argument is copied");
    CHECK(allocations==1 && frees==1,"owning pack string freed once through capture allocator");
    CHECK(std::string(first.m_pszString)=="first-pack-string" && first.m_flags==0,
          "borrowed source string is not destroyed");
    CVariant number(int32{42}); CPulseArgumentPack numeric{{&number}};
    Invoke(&S2NamedOutputOp,&numeric,nullptr);
    CHECK(observed=="42","ordinary numeric pack value is preserved");
    Vector vector(7,8,9); CVariant vector_value;
    vector_value.m_type=FIELD_VECTOR; vector_value.m_pVector=&vector;
    CPulseArgumentPack vector_pack{{&vector_value}};
    Invoke(&S2NamedOutputOp,&vector_pack,nullptr);
    CHECK(observed=="7 8 9" && allocations==2 && frees==2,"owning vector copied and released once");
    deliveries.clear();
    nested_dispatch=[&] { Invoke(&S2NamedOutputOp,&pack,&second); };
    Invoke(&S2NamedOutputOp,&pack,nullptr);
    CHECK(deliveries==std::vector<std::string>({"first-pack-string","second-unused-string"}),
          "nested dispatch retains independent copied values");
    CHECK(allocations==3 && frees==3,"nested output leaves no owning capture");
    const int prior=dispatches;
    s_pPulseFirstArgumentToVariant=nullptr;
    CHECK(Invoke(&S2NamedOutputOp,&pack,nullptr)==0 && dispatches==prior,
          "unavailable pack decoder falls through without publishing guessed value");
    CHECK(S2NamedOutputOp(nullptr,nullptr,nullptr,&pack,0,nullptr,nullptr)==0,
          "missing output descriptor falls through");
    CHECK(dispatches==prior,"missing descriptor does not dispatch");
}
static void OptionalValueRegression() {
    // A Pulse pack is opaque, not a variant. Deliberately variant-shaped bytes
    // reproduce the former invalid string read while arg7 supplies a valid value.
    CVariant misleading_pack; misleading_pack.m_type=FIELD_CSTRING;
    misleading_pack.m_pszString=reinterpret_cast<const char*>(uintptr_t{1});
    CVariant supplied("copied-final-value",false);
    CHECK(Invoke(&S2NamedOutputOp,&misleading_pack,&supplied)==2,"handler verdict preserved");
    CHECK(observed=="copied-final-value","argument seven supplies the output value, never argument four");
}

int main() {
    const pid_t child=fork();
    if(child==0) { OptionalValueRegression(); _exit(failures ? 1 : 0); }
    int status=0; waitpid(child,&status,0);
    CHECK(WIFEXITED(status) && WEXITSTATUS(status)==0,
          "optional value regression completed without reading the opaque argument pack as a variant");
    if(failures) return 1;
    BorrowedFormatting(); PackCaptureAndLifetime();
    if(failures) return 1;
    std::puts("named_output_value: production argument/value, formatting, ownership and nesting regressions passed");
    return 0;
}
