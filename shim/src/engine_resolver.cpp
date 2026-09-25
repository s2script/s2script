#include "engine_resolver.h"
#include "sigscan.h"
#include "vtable.h"
#include "../third_party/json.hpp"
#include <cstring>
#if defined(__linux__) && !defined(S2_RESOLVER_ENGINE_FREE)
#include <khook.hpp>
#endif

namespace s2resolve {
namespace {
using nlohmann::json;
bool fail(std::string& reason, const std::string& why) { reason=why; return false; }
bool c_string(const std::string& value) { return value.find('\0')==std::string::npos; }
s2validate::ModuleView view(const Sources& sources) {
    s2validate::ModuleView mv;
    mv.read_code=[image=sources.image](uintptr_t at,void* out,size_t n) { return image->read(at,out,n); };
    mv.executable=[image=sources.image](uintptr_t at,size_t n) { return image->executable(at,n); };
    mv.read_live=sources.read_live;
    return mv;
}
bool validate(const TargetRecipe& recipe, const s2validate::ModuleView& mv,
              const Sources& sources, uintptr_t address, std::string& reason) {
    char why[512]={};
    if (!s2validate::Run(recipe.validate_json.c_str(),mv,recipe.module.c_str(),
                         reinterpret_cast<void*>(address),sources.ops,why,sizeof why))
        return fail(reason,why);
    return true;
}
std::vector<uintptr_t> matches(const s2original::Image& image,const std::vector<int>& pattern) {
    std::vector<uintptr_t> result;
    for (const auto& segment : image.executable_segments())
        for (size_t at : s2sig::FindPatterns(segment.bytes.data(),segment.bytes.size(),pattern))
            result.push_back(segment.live_begin+at);
    return result;
}
bool relative(const s2original::Image& image,uintptr_t pc,size_t offset,size_t length,uintptr_t& target) {
    uintptr_t operand=0; int32_t displacement=0;
    return image.executable(pc,length) && offset<=length && sizeof displacement<=length-offset &&
           s2sig::AddRelative(pc,offset,0,operand) && image.read(operand,&displacement,sizeof displacement) &&
           s2sig::AddRelative(pc,length,displacement,target);
}
bool ctor_target(const s2original::Image& image,uintptr_t ctor,uintptr_t& target,std::string& reason) {
    uintptr_t caller=0; size_t count=0;
    for (uintptr_t at : matches(image,{0xe8,-1,-1,-1,-1})) {
        uintptr_t callee=0;
        if (relative(image,at,1,5,callee) && callee==ctor) { caller=at; ++count; }
    }
    if (count!=1) return fail(reason,count ? "ctor-body-xref ambiguous constructor callers" : "ctor-body-xref constructor caller not found");
    // Whole LEA must precede the call; never interpret bytes overlapping the E8 operand.
    for (size_t back=7; back<=32 && back<=caller; ++back) {
        uint8_t bytes[7]; uintptr_t at=caller-back;
        if (image.read(at,bytes,sizeof bytes) && bytes[0]==0x4c && bytes[1]==0x8d &&
            (bytes[2]==0x35 || bytes[2]==0x2d)) {
            if (relative(image,at,3,7,target)) return true;
            return fail(reason,"ctor-body-xref derived address overflow");
        }
    }
    return fail(reason,"ctor-body-xref preceding LEA not found");
}
#if defined(__linux__) && !defined(S2_RESOLVER_ENGINE_FREE)
bool production_sources(const std::string& module,Sources& sources,std::string& reason) {
    auto image=s2original::OpenLoadedModule(module.c_str(),reason);
    if (!image) return false;
    sources={};
    sources.image=image;
    sources.mapped=[image](uintptr_t at,size_t n) { return image->mapped(at,n); };
    sources.read_live=[image](uintptr_t at,void* out,size_t n) { return image->read_live(at,out,n); };
    sources.ops.vtable_from_image=[image](const char* cls) { return s2vtable::GetVTableByName(*image,cls); };
    sources.ops.original_virtual=&KHook::FindOriginalVirtual;
    return true;
}
#endif
}

bool EvaluateVirtualSlot(const std::string& module,const std::string& className,int index,
                         const Sources& sources,VirtualSlotResolution& out,std::string& reason) {
    out={}; reason.clear();
    if (!sources.image) return fail(reason,"verified original module image unavailable");
    if (!c_string(module) || !c_string(className))
        return fail(reason,"virtual slot input contains an interior NUL");
    if (module.empty()) return fail(reason,"virtual module invalid");
    if (className.empty() || index<0 || index>=512)
        return fail(reason,"virtual class/index invalid");
    if (!sources.ops.vtable_from_image && !sources.ops.vtable_by_name)
        return fail(reason,"RTTI vtable resolution unavailable");
    void** vtable=sources.ops.vtable_from_image ? sources.ops.vtable_from_image(className.c_str()) :
        sources.ops.vtable_by_name(module.c_str(),className.c_str());
    uintptr_t slot=0; void* live=nullptr;
    if (!vtable || !s2sig::AddRelative(reinterpret_cast<uintptr_t>(vtable),
                                       size_t(index)*sizeof(void*),0,slot) ||
        !sources.read_live || !sources.read_live(slot,&live,sizeof live))
        return fail(reason,"virtual slot outside readable module data");
    if (!sources.ops.original_virtual)
        return fail(reason,"virtual original provider unavailable");
    void* original=sources.ops.original_virtual(vtable,index);
    if (!original) return fail(reason,"virtual original provider returned null");
    const uintptr_t target=reinterpret_cast<uintptr_t>(original);
    if (!sources.image->executable(target))
        return fail(reason,"virtual original outside original executable image");
    out.target.address=target;
    out.target.image=sources.image;
    out.target.recipe=json{{"kind","VirtualSlot"},{"module",module},
        {"class",className},{"index",index}}.dump();
    out.target.validation_receipt=json{{"validated","structural-virtual-slot"},
        {"build_id",sources.image->identity().build_id},{"class",className},
        {"index",index},{"vtable",reinterpret_cast<uintptr_t>(vtable)},
        {"original",target}}.dump();
    out.vtable=vtable;
    out.vtable_index=index;
    return true;
}

bool Evaluate(const TargetRecipe& recipe,const Sources& sources,Resolution& out,std::string& reason) {
    out={}; reason.clear();
    if (!sources.image) return fail(reason,"verified original module image unavailable");
    if (!c_string(recipe.module) || !c_string(recipe.pattern) || !c_string(recipe.strategy) ||
        !c_string(recipe.class_name) || !c_string(recipe.validate_json))
        return fail(reason,"recipe contains an interior NUL");
    if (recipe.kind!=Kind::Signature && recipe.kind!=Kind::Virtual) return fail(reason,"unknown target kind");
    if (recipe.use!=TargetUse::Executable && recipe.use!=TargetUse::MappedAddress) return fail(reason,"unknown target use");
    const std::string strategy=recipe.strategy.empty() ? "direct" : recipe.strategy;
    if (strategy!="direct" && strategy!="lea-disp" && strategy!="ctor-body-xref" && strategy!="validated-call")
        return fail(reason,"unknown resolver strategy: "+strategy);
    auto mv=view(sources);
    uintptr_t target=0;
    bool call_site_validated=false;
    if (recipe.kind==Kind::Virtual) {
        if (strategy!="direct") return fail(reason,"virtual recipe requires direct strategy");
        if (recipe.class_name.empty() || recipe.vtable_index<0 || recipe.vtable_index>=512)
            return fail(reason,"virtual class/index invalid");
        if (!s2validate::DeclaresPrologue(recipe.validate_json.c_str()))
            return fail(reason,"virtual target requires validate.prologue");
        VirtualSlotResolution slot;
        if (!EvaluateVirtualSlot(recipe.module,recipe.class_name,recipe.vtable_index,
                                 sources,slot,reason)) return false;
        target=slot.target.address;
    } else {
        auto pattern=s2sig::ParsePattern(recipe.pattern);
        if (pattern.empty()) return fail(reason,"malformed signature pattern");
        auto candidates=matches(*sources.image,pattern);
        if (strategy=="validated-call") {
            auto validation=json::parse(recipe.validate_json,nullptr,false,true);
            if (!validation.is_object() || !validation.contains("string-xref"))
                return fail(reason,"validated-call requires validate.string-xref on the call site");
            size_t count=0; uintptr_t caller=0; std::string rejection="pattern did not match";
            for (uintptr_t at : candidates) {
                uint8_t opcode=0;
                if (!sources.image->read(at,&opcode,1) || opcode!=0xe8) {
                    rejection="site is not E8 rel32"; continue;
                }
                if (!validate(recipe,mv,sources,at,rejection)) continue;
                caller=at; ++count;
            }
            if (count!=1) return fail(reason,count ? "validated-call ambiguous (>1 validated call site)" :
                                                    "validated-call: no validated call site ("+rejection+")");
            if (!relative(*sources.image,caller,1,5,target)) return fail(reason,"validated-call displacement out of bounds");
            call_site_validated=true;
        } else {
            // A direct recipe may break a tie with its own validators (e.g. two classes' byte-identical
            // thunks, told apart by vtable-member). Exactly one survivor is required; derived
            // strategies keep the strict single-match rule because their validators judge the
            // derived target, not the match.
            if (candidates.size()>1 && strategy=="direct" && !recipe.validate_json.empty() && recipe.validate_json!="{}") {
                std::vector<uintptr_t> passed; std::string rejection;
                for (uintptr_t at : candidates) if (validate(recipe,mv,sources,at,rejection)) passed.push_back(at);
                if (passed.size()!=1)
                    return fail(reason,"signature ambiguous ("+std::to_string(candidates.size())+" matches, "+
                                std::to_string(passed.size())+" passed validators)");
                candidates=passed;
            }
            if (candidates.size()!=1) return fail(reason,candidates.empty() ? "signature not found" : "signature ambiguous (>1 match)");
            target=candidates.front();
            if (strategy=="lea-disp" && !relative(*sources.image,target,3,7,target))
                return fail(reason,"lea-disp displacement out of bounds");
            if (strategy=="ctor-body-xref" && !ctor_target(*sources.image,target,target,reason)) return false;
        }
    }
    if (recipe.use==TargetUse::Executable || call_site_validated || recipe.kind==Kind::Virtual) {
        if (!sources.image->executable(target)) return fail(reason,"resolved target outside original executable image");
    } else if (!sources.mapped || !sources.mapped(target,1)) {
        return fail(reason,"resolved target outside readable mapped module data");
    }
    // A caller-relative validator is deliberately not replayed against its callee.
    // Run itself refuses every non-empty instruction validator against a data target.
    if (!call_site_validated && !validate(recipe,mv,sources,target,reason)) return false;
    const char* use=recipe.use==TargetUse::Executable ? "Executable" : "MappedAddress";
    out.address=target; out.image=sources.image;
    out.recipe=json{{"kind",recipe.kind==Kind::Signature ? "Signature" : "Virtual"},{"use",use},
        {"module",recipe.module},{"pattern",recipe.pattern},{"strategy",strategy},{"class",recipe.class_name},
        {"index",recipe.vtable_index},{"validate",recipe.validate_json}}.dump();
    out.validation_receipt=json{{"use",use},{"build_id",sources.image->identity().build_id},
        {"target",target},{"validated",call_site_validated ? "call-site" : "target"}}.dump();
    return true;
}

bool Resolve(const TargetRecipe& recipe,Resolution& out,std::string& reason) {
    out={}; reason.clear();
#if defined(__linux__) && !defined(S2_RESOLVER_ENGINE_FREE)
    Sources sources;
    if (!production_sources(recipe.module,sources,reason)) return false;
    return Evaluate(recipe,sources,out,reason);
#else
    (void)recipe;
    return fail(reason,"production resolver requires Linux and Metamod (use Evaluate for fixtures)");
#endif
}

bool ResolveVirtualSlot(const std::string& module,const std::string& className,int index,
                        VirtualSlotResolution& out,std::string& reason) {
    out={}; reason.clear();
#if defined(__linux__) && !defined(S2_RESOLVER_ENGINE_FREE)
    if (!c_string(module) || !c_string(className))
        return fail(reason,"virtual slot input contains an interior NUL");
    if (module.empty()) return fail(reason,"virtual module invalid");
    if (className.empty() || index<0 || index>=512)
        return fail(reason,"virtual class/index invalid");
    Sources sources;
    if (!production_sources(module,sources,reason)) return false;
    return EvaluateVirtualSlot(module,className,index,sources,out,reason);
#else
    (void)module; (void)className; (void)index;
    return fail(reason,"production resolver requires Linux and Metamod (use EvaluateVirtualSlot for fixtures)");
#endif
}
}
