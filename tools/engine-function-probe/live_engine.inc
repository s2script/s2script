// Included inside the fixture namespace. Every native pointer stays in this DSO.
// Entity and schema contacts match the production shim's existing read-only paths.
ISchemaSystem* schema = nullptr;
IVEngineServer2* engine = nullptr;
void* resources = nullptr;
int entity_system_offset = -1;
GameConfig game_config, core_config;
std::string addon_root, game_name;
std::map<std::string,std::string> gamedata_bytes;
s2resolve::Resolution acquire_resolution;
Owned* real_acquire = nullptr;
using AcquirePeer = S2CheckedFunction<std::int32_t,void*,void*,std::int32_t,void*>;
std::unique_ptr<AcquirePeer> acquire_peer;
std::uint64_t real_peer_pre = 0, real_peer_post = 0;
std::uint64_t real_pre = 0, real_post = 0, real_non_skipped = 0;
int real_depth = 0;
bool real_used = false, inner_ok = false;
std::int32_t inner_result = -1;
std::vector<std::int32_t> real_peer_results, real_results;
nlohmann::json real_invocations = nlohmann::json::array();
std::string FileBytes(const std::string& path) {
    std::ifstream in(path,std::ios::binary);
    Require(static_cast<bool>(in),"cannot read deployed gamedata");
    std::string bytes((std::istreambuf_iterator<char>(in)),{});
    Require(!in.bad(),"deployed gamedata read failed"); return bytes;
}
void RetainFiles(const GameConfig& config,const std::string& owner) {
    auto files=config.filesLoaded; files.push_back("master.gamedata.jsonc");
    for(const auto& file:files) {
        const auto path=addon_root+"/gamedata/"+owner+"/"+file;
        gamedata_bytes[path]=FileBytes(path);
    }
}
bool GamedataUnchanged() {
    for(const auto& file:gamedata_bytes) if(FileBytes(file.first)!=file.second) return false;
    std::string error;
    const auto game=LoadGameConfig(addon_root+"/gamedata","cs2","source2",game_name,"linuxsteamrt64",error);
    if(!error.empty() || game.mergedJson!=game_config.mergedJson || game.filesLoaded!=game_config.filesLoaded) return false;
    const auto core=LoadGameConfig(addon_root+"/gamedata","core","source2",game_name,"linuxsteamrt64",error);
    return error.empty() && core.mergedJson==core_config.mergedJson && core.filesLoaded==core_config.filesLoaded;
}
int CvarInt(const char* name) {
    const auto ref=cvars->FindConVar(name,false);
    if(!ref.IsValidRef()) return -1;
    auto* data=cvars->GetConVarData(ref);
    if(!data || data->GetType()!=EConVarType_Int32) return -1;
    const auto offset=sizeof(ConVarData)-sizeof(CVValue_t)*MAX_SPLITSCREEN_CLIENTS;
    return reinterpret_cast<const CVValue_t*>(reinterpret_cast<const char*>(data)+offset)->m_i32Value;
}
struct ScopedMarker { CVValue_t* slot; int previous; ~ScopedMarker() { slot->m_i32Value=previous; } };
ScopedMarker SetScope(int value) {
    // Fixture-only synchronous marker, not a general CVar mutation API. No
    // callbacks subscribe to this marker; public witnesses read it in scope.
    const auto ref=cvars->FindConVar("s2_engine_fixture_scope",false);
    Require(ref.IsValidRef(),"fixture scope marker unavailable");
    auto* data=cvars->GetConVarData(ref);
    Require(data && data->GetType()==EConVarType_Int32,"fixture scope marker type mismatch");
    const auto offset=sizeof(ConVarData)-sizeof(CVValue_t)*MAX_SPLITSCREEN_CLIENTS;
    auto* slot=reinterpret_cast<CVValue_t*>(reinterpret_cast<char*>(data)+offset);
    const int previous=slot->m_i32Value; slot->m_i32Value=value;
    return {slot,previous};
}
int Field(CSchemaClassInfo* info,const char* name,int depth=0) {
    if(!info || depth>32) return -1;
    for(int i=0;i<info->m_nFieldCount;++i) {
        const auto& f=info->m_pFields[i];
        if(f.m_pszName && std::strcmp(f.m_pszName,name)==0) return f.m_nSingleInheritanceOffset;
    }
    for(int b=0;b<info->m_nBaseClassCount;++b) {
        const auto& base=info->m_pBaseClasses[b];
        const int offset=Field(base.m_pClass,name,depth+1);
        if(offset>=0) return base.m_nOffset+offset;
    }
    return -1;
}
int Offset(const char* cls,const char* field) {
    auto* scope=schema->FindTypeScopeForModule("libserver.so");
    auto* info=scope?scope->FindRawClassBinding(cls):nullptr;
    if(!info) { scope=schema->GlobalTypeScope(); info=scope?scope->FindRawClassBinding(cls):nullptr; }
    return Field(info,field);
}
CEntityIdentity* Identity(int index) {
    if(index<0 || index>=MAX_TOTAL_ENTITIES) return nullptr;
    CGameEntitySystem* system=nullptr;
    std::memcpy(&system,static_cast<char*>(resources)+entity_system_offset,sizeof(system));
    if(!system) return nullptr;
    auto* chunk=system->m_EntityList.m_pIdentityChunks[index/MAX_ENTITIES_IN_LIST];
    if(!chunk) return nullptr;
    auto* identity=&chunk[index%MAX_ENTITIES_IN_LIST];
    return !(identity->m_flags&EF_IS_INVALID_EHANDLE) && identity->m_pInstance?identity:nullptr;
}
bool OwnedBot(void* services) {
    const int slot=CvarInt("s2_engine_fixture_slot");
    if(CvarInt("s2_engine_fixture_active")!=static_cast<int>(generation) || slot<0 || slot>=64) return false;
    auto* controller=Identity(slot+1);
    if(!controller || engine->GetClientXUID(CPlayerSlot(slot))!=0 || engine->GetPlayerNetInfo(CPlayerSlot(slot))) return false;
    const int pawn_offset=Offset("CCSPlayerController","m_hPlayerPawn");
    const int services_offset=Offset("CBasePlayerPawn","m_pItemServices");
    if(pawn_offset<0 || services_offset<0) return false;
    std::uint32_t raw=0;
    std::memcpy(&raw,reinterpret_cast<char*>(controller->m_pInstance)+pawn_offset,sizeof(raw));
    const CEntityHandle handle(raw);
    auto* pawn=Identity(handle.GetEntryIndex());
    if(!pawn || pawn->GetRefEHandle()!=handle || handle.GetEntryIndex()!=CvarInt("s2_engine_fixture_pawn")) return false;
    void* current=nullptr;
    std::memcpy(&current,reinterpret_cast<char*>(pawn->m_pInstance)+services_offset,sizeof(current));
    return current && current==services;
}
KHook::Return<std::int32_t> RealPeerPre(void* services,void*,std::int32_t,void*) {
    auto guard=acquire_peer->Observe();
    if(S2Hook_EnterDispatch(guard) && OwnedBot(services)) ++real_peer_pre;
    return {KHook::Action::Ignore, std::int32_t{0}};
}
KHook::Return<std::int32_t> RealPeerPost(void* services,void*,std::int32_t,void*) {
    auto guard=acquire_peer->Observe();
    if(S2Hook_EnterDispatch(guard) && OwnedBot(services)) {
        ++real_peer_post;
        real_peer_results.push_back(KHook::GetCurrentReturn<std::int32_t>());
        if(KHook::WasOriginalFunctionSkipped()) Record("real-acquire-peer-skipped",false);
    }
    return {KHook::Action::Ignore, std::int32_t{0}};
}
void RealDispatch(DispatchFrame& f) {
    if(!OwnedBot(f.receiver.Get<void*>())) return;
    if(f.phase==Phase::Pre) {
        ++real_pre;
        if(real_depth || real_used) return;
        Require(GamedataUnchanged(),"gamedata changed before real engine stimulus");
        Require(CvarInt("s2_engine_fixture_scope")==1 && CvarInt("s2_engine_fixture_operation")>0,"outer invocation marker unavailable");
        real_used=true;
        struct Depth { int& value; explicit Depth(int& v):value(v){++value;} ~Depth(){--value;} } depth(real_depth);
        const auto marker=SetScope(2);
        // This is a deliberate nested Call during the one armed giveNamedItem,
        // never a replay: these unchanged pointers remain borrowed from this PRE.
        NativeValue args[]{f.receiver,f.arguments[0],f.arguments[1],f.arguments[2]};
        const auto nested=real_acquire->binding->Call(args,4);
        inner_ok=static_cast<bool>(nested);
        if(nested) inner_result=nested.value.Get<std::int32_t>();
    } else {
        ++real_post; if(!f.original_skipped) ++real_non_skipped;
        real_results.push_back(f.result.Get<std::int32_t>());
        const int id=CvarInt("s2_engine_fixture_scope");
        Require(id==1 || id==2,"native completion has no scoped invocation marker");
        const int definition_offset=Offset("CEconItemView","m_iItemDefinitionIndex");
        Require(definition_offset>=0 && f.arguments[0].Get<void*>(),"item definition identity unavailable");
        std::uint16_t definition=0;
        std::memcpy(&definition,static_cast<char*>(f.arguments[0].Get<void*>())+definition_offset,sizeof(definition));
        real_invocations.push_back({{"id",id},{"operation",CvarInt("s2_engine_fixture_operation")},
            {"slot",CvarInt("s2_engine_fixture_slot")},{"pawn",CvarInt("s2_engine_fixture_pawn")},
            {"method",f.arguments[1].Get<std::int32_t>()},{"defIndex",definition},
            {"result",f.result.Get<std::int32_t>()},{"skipped",f.original_skipped}});
        // Do not change arguments, policy or return value of either invocation.
    }
}
void PrepareReal(ISmmAPI* api) {
    Dl_info path{}; Require(dladdr(reinterpret_cast<void*>(&PrepareReal),&path) && path.dli_fname,"probe path unavailable");
    addon_root=std::filesystem::canonical(path.dli_fname).parent_path().parent_path().parent_path().string();
    game_name=std::filesystem::path(addon_root).parent_path().parent_path().filename().string();
    std::string error;
    game_config=LoadGameConfig(addon_root+"/gamedata","cs2","source2",game_name,"linuxsteamrt64",error);
    Require(error.empty() && game_config.filesFailed.empty(),"deployed cs2 gamedata unavailable");
    core_config=LoadGameConfig(addon_root+"/gamedata","core","source2",game_name,"linuxsteamrt64",error);
    Require(error.empty() && core_config.filesFailed.empty(),"deployed core gamedata unavailable");
    RetainFiles(game_config,"cs2"); RetainFiles(core_config,"core");
    using Json=nlohmann::json;
    const auto hook=game_config.hooks.find("onCanAcquire"); Require(hook!=game_config.hooks.end(),"CanAcquire hook absent");
    const auto declaration=Json::parse(hook->second);
    Require(declaration.value("shape","")=="this_i64_i32_i64","CanAcquire shape mismatch");
    auto target=declaration.at("target"); Require(target.value("kind","")=="signature","CanAcquire target kind mismatch");
    if(target.value("pattern","").empty()) {
        const auto sig=game_config.signatures.find(target.value("name","")); Require(sig!=game_config.signatures.end(),"CanAcquire signature absent");
        target["module"]=sig->second.module; target["pattern"]=sig->second.pattern; target["resolve"]=sig->second.resolve;
        if(!target.contains("validate") && !sig->second.validate.empty()) target["validate"]=Json::parse(sig->second.validate);
    }
    Require(target.contains("validate") && !target["validate"].empty(),"CanAcquire semantic validator required");
    s2resolve::TargetRecipe recipe;
    recipe.module=target.value("module",""); recipe.pattern=target.value("pattern",""); recipe.strategy=target.value("resolve","direct"); recipe.validate_json=target["validate"].dump();
    Require(s2resolve::Resolve(recipe,acquire_resolution,error),error.c_str());
    auto factory=api->GetEngineFactory(false); Require(factory!=nullptr,"engine factory missing");
    int status=0;
    schema=static_cast<ISchemaSystem*>(factory(core_config.interfaces.at("SchemaSystem").c_str(),&status)); Require(schema && status==0,"schema interface unavailable");
    engine=static_cast<IVEngineServer2*>(factory(core_config.interfaces.at("EngineToServer").c_str(),&status)); Require(engine && status==0,"engine interface unavailable");
    resources=factory(core_config.interfaces.at("GameResourceService").c_str(),&status); Require(resources && status==0,"resource interface unavailable");
    entity_system_offset=core_config.offsets.at("GameEntitySystem"); Require(entity_system_offset>=0,"entity system offset unavailable");
    if(!acquire_peer) {
        acquire_peer=std::make_unique<AcquirePeer>(&RealPeerPre,&RealPeerPost);
        const auto receipt=acquire_peer->Configure(reinterpret_cast<void*>(acquire_resolution.address)); Require(receipt.Accepted(),receipt.reason.c_str());
    }
    AbiSignature signature; signature.receiver="entity"; signature.parameters={{"ptr"},{"i32"},{"ptr"}}; signature.returns={"i32"};
    real_acquire=&Bind(signature,reinterpret_cast<void*>(acquire_resolution.address)); real_acquire->sink.fn=RealDispatch;
    real_pre=real_post=real_non_skipped=real_peer_pre=real_peer_post=0; real_depth=0; real_used=false; inner_ok=false; real_peer_results.clear(); real_results.clear(); real_invocations=nlohmann::json::array();
}
void CollectReal() {
    Require(owned.size()==5 && !retiring,"real acquisition generation is not armed");
    const bool pass=real_used && inner_ok && real_pre==2 && real_post==2 && real_non_skipped==2 && real_peer_pre==2 && real_peer_post==2 &&
        real_peer_results.size()==2 && real_peer_results==real_results && real_peer_results[0]==inner_result && real_acquire->sink.error.empty() && GamedataUnchanged();
    Record("real-acquire",pass,"{\"invocations\":"+real_invocations.dump()+",\"outer_pre\":"+std::to_string(real_used?1:0)+",\"deliberate_nested_pre\":"+std::to_string(real_pre?real_pre-1:0)+
        ",\"non_skipped_completions\":"+std::to_string(real_non_skipped)+",\"peer_pre\":"+std::to_string(real_peer_pre)+
        ",\"peer_post\":"+std::to_string(real_peer_post)+",\"nested_result\":"+std::to_string(inner_result)+
        ",\"outer_result\":"+std::to_string(real_results.size()==2?real_results[1]:-1)+",\"validation_receipt\":\""+Escape(acquire_resolution.validation_receipt)+"\",\"direct_body_counter\":false}","real-engine-acquire");
}
