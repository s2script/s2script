#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>
#include <array>
#include <vector>
#include <map>
#include "named_hooks.h"

namespace s2khook {

struct DeclarativeVoidObservation {
    bool installed = false;
    int pre = 0;
    int original = 0;
    int original_in_scope = 0;
    int receiver_ok = 0;
    bool expired = false;
    bool Passed() const {
        return installed && pre == 1 && original == 1 && original_in_scope == 1 &&
               receiver_ok == 1 && expired;
    }
    std::string Json() const {
        return std::string("{\"installed\":") + (installed ? "true" : "false") +
            ",\"pre\":" + std::to_string(pre) + ",\"original\":" + std::to_string(original) +
            ",\"original_in_scope\":" + std::to_string(original_in_scope) +
            ",\"receiver_ok\":" + std::to_string(receiver_ok) +
            ",\"expired\":" + (expired ? "true" : "false") + "}";
    }
};

struct DeclarativeMutationObservation {
    int pre = 0, original = 0, original_in_scope = 0;
    float value = 0;
    int32_t integer = 0;
    int64_t opaque_a = 0, opaque_b = 0;
    bool Passed() const {
        return pre == 1 && original == 1 && original_in_scope == 1 && value == 7.25f &&
            integer == -17 && static_cast<uint64_t>(opaque_a) == UINT64_C(0xf123456789abcdef) &&
            static_cast<uint64_t>(opaque_b) == UINT64_C(0x8123456789abcdef);
    }
    std::string Json() const {
        return std::string("{\"pre\":") + std::to_string(pre) +
            ",\"original\":" + std::to_string(original) +
            ",\"original_in_scope\":" + std::to_string(original_in_scope) +
            ",\"value\":" + std::to_string(value) + ",\"integer\":" + std::to_string(integer) +
            ",\"opaque_a\":\"" + std::to_string(static_cast<uint64_t>(opaque_a)) +
            "\",\"opaque_b\":\"" + std::to_string(static_cast<uint64_t>(opaque_b)) + "\"}";
    }
};
// Nesting/bypass/policy mechanics ride the controlled narrow-shape target (hook id 2). The
// retired v1 POST phase is gone, so only PRE callbacks and original-body counts are observable;
// `outer_method` is the method the outermost original body last saw (the old return value).
struct DeclarativeNestingObservation {
    int same_pre = 0, same_original = 0, same_restored = 0;
    int other_pre = 0, other_original = 0, other_restored = 0, stale_rejected = 0;
    int32_t outer_method = -1;
    bool Passed() const {
        return same_pre == 3 && same_original == 3 && same_restored == 2 &&
            other_pre == 2 && other_original == 2 && other_restored == 2 && stale_rejected == 2 &&
            outer_method == 40;
    }
    std::string Json() const {
        return std::string("{\"same_pre\":") + std::to_string(same_pre) +
            ",\"same_original\":" + std::to_string(same_original) +
            ",\"same_restored\":" + std::to_string(same_restored) + ",\"other_pre\":" + std::to_string(other_pre) +
            ",\"other_original\":" + std::to_string(other_original) + ",\"other_restored\":" + std::to_string(other_restored) +
            ",\"stale_rejected\":" + std::to_string(stale_rejected) +
            ",\"outer_method\":" + std::to_string(outer_method) + "}";
    }
};
struct DeclarativeBypassObservation {
    int pre = 0, original = 0;
    int pre_after_bypass = -1;
    int removal_refused = 0, reset_preserved_view = 0;
    bool Passed() const {
        return pre == 2 && original == 3 && pre_after_bypass == 0 &&
            removal_refused == 2 && reset_preserved_view == 2;
    }
    std::string Json() const {
        return std::string("{\"pre\":") + std::to_string(pre) +
            ",\"original\":" + std::to_string(original) + ",\"pre_after_bypass\":" + std::to_string(pre_after_bypass) +
            ",\"removal_refused\":" + std::to_string(removal_refused) +
            ",\"reset_preserved_view\":" + std::to_string(reset_preserved_view) + "}";
    }
};
struct DeclarativeSnapshot {
    bool forged_rejected=false,stale_rejected=false;
    int bypass_pair_pre=-1,bypass_pair_original=-1;
    bool policy_isolated=false,policy_restored=false;
    int policy_pre=0,policy_original=0;

    DeclarativeVoidObservation simple;
    DeclarativeMutationObservation mutation;
    DeclarativeNestingObservation nesting;
    DeclarativeBypassObservation bypass;
    bool Passed() const {
        return simple.Passed() && mutation.Passed() && nesting.Passed() && bypass.Passed();
    }
    std::string Json() const {
        return "{\"simple\":" + simple.Json() + ",\"mutation\":" + mutation.Json() +
            ",\"nesting\":" + nesting.Json() + ",\"bypass\":" + bypass.Json() + "}";
    }
};

// Main-runtime observations are never merged with the private production-TU
// snapshots above. Only a driver-owned invocation can accept JS marker calls.
struct MainBridgeObservation {
    int scenario=0, sequence=0, generation=0, order=0;
    int callbacks=0, original=0, peer_pre=0, peer_post=0, skipped=-1, effective=-1, result=-1;
    float value=0;
    int a=0,b=0,c=0;
    bool opaque_a=false,opaque_b=false;
    std::string trace;
    int bypass_original=-1,bypass_peer_pre=-1,bypass_peer_post=-1,bypass_callbacks=-1;
    std::string direct_trace;
    bool BypassObserved() const {
        return scenario==12 && bypass_original==1 && bypass_peer_pre==1 && bypass_peer_post==1 && bypass_callbacks==0 &&
            original-bypass_original==1 && peer_pre-bypass_peer_pre==1 && peer_post-bypass_peer_post==1 && callbacks-bypass_callbacks==1;
    }
    uintptr_t target_address=0;
    std::map<int,int> generation_callbacks;
};

struct PrecacheTokenObservation {
    int token=0,generation=0,map_generation=0,receiver=0,vtable=0,manifest=0;
    S2NamedPrecacheFrameV1 frame{};
    bool finished=false,added=false,peer_completed=false;
    int peer_pre=0;
    std::string peer_trace;
    std::string resource;
};

class PrecacheTokens {
public:
    void Reset(const std::string& run) { run_=run; next_=0; labels_.clear(); rows_.clear(); }
    int Begin(const std::string& run,int generation,int map,const S2NamedPrecacheFrameV1& frame) {
        if (run.empty() || run!=run_ || generation<=0 || map<=0 || frame.version!=1 ||
            frame.size!=sizeof frame || !frame.serial || !frame.receiver || !frame.vtable || !frame.manifest) return 0;
        for (const auto& row:rows_) if (row.frame.serial==frame.serial) return 0;
        PrecacheTokenObservation row;
        row.token=++next_; row.generation=generation; row.map_generation=map; row.frame=frame;
        row.receiver=Label(frame.receiver); row.vtable=Label(frame.vtable); row.manifest=Label(frame.manifest);
        rows_.push_back(row); return row.token;
    }
    bool Finish(const std::string& run,int token,int generation,const S2NamedPrecacheFrameV1& frame,
                const std::string& resource,bool added) {
        auto* row=Find(token);
        if (run!=run_ || !row || row->finished || row->generation!=generation || !Same(row->frame,frame) || resource.empty()) return false;
        row->finished=true; row->resource=resource; row->added=added; return true;
    }
    int Read(int token,int field,const S2NamedPrecacheFrameV1& frame) {
        auto* row=Find(token);
        if (!row || !row->finished || !Same(row->frame,frame)) return 0;
        switch (field) {
            case 0:return row->map_generation;
            case 1:return -1; // No ambiguous external peer/frame order inference.
            case 2:return row->receiver;
            case 3:return row->vtable;
            case 4:return row->manifest;
            default:return 0;
        }
    }
    bool ObservePeer(int token,const S2NamedPrecacheFrameV1& frame,int pre,const std::string& trace) {
        auto* row=Find(token);
        if (!row || !row->finished || row->peer_completed || !Same(row->frame,frame)) return false;
        row->peer_completed=true; row->peer_pre=pre; row->peer_trace=trace; return true;
    }
    const std::vector<PrecacheTokenObservation>& Rows() const { return rows_; }
private:
    static bool Same(const S2NamedPrecacheFrameV1& a,const S2NamedPrecacheFrameV1& b) {
        return b.version==1 && b.size==sizeof b && a.serial==b.serial && a.receiver==b.receiver &&
            a.vtable==b.vtable && a.manifest==b.manifest;
    }
    int Label(uintptr_t pointer) {
        auto it=labels_.find(pointer);
        if (it!=labels_.end()) return it->second;
        int label=static_cast<int>(labels_.size())+1; labels_[pointer]=label; return label;
    }
    PrecacheTokenObservation* Find(int token) {
        if (token<=0 || static_cast<size_t>(token)>rows_.size()) return nullptr;
        return &rows_[static_cast<size_t>(token)-1];
    }
    std::string run_;
    int next_=0;
    std::map<uintptr_t,int> labels_;
    std::vector<PrecacheTokenObservation> rows_;
};

enum class FacetApplicability { Unspecified, Applicable, Inapplicable };

struct FacetObservation {
    FacetApplicability applicability = FacetApplicability::Unspecified;
    int value = -1;
    bool ExplicitlyInapplicable() const {
        return applicability == FacetApplicability::Inapplicable && value == -1;
    }
    std::string Json() const {
        const char* label=applicability==FacetApplicability::Applicable ? "applicable" :
            applicability==FacetApplicability::Inapplicable ? "inapplicable" : "unspecified";
        return std::string("{\"applicability\":\"") + label + "\",\"value\":" +
            (applicability==FacetApplicability::Inapplicable && value==-1 ?
                "null" : std::to_string(value)) + "}";
    }
};

enum class MapGenerationSource { Unspecified, LevelLifetime };

struct NamedRemovalObservation {
    FacetApplicability applicability = FacetApplicability::Unspecified;
    int active_refused = -1;
    int terminal_preflight = -1;
    int terminal_remove = -1;
    int terminal_complete = -1;
    bool Pending() const {
        return applicability==FacetApplicability::Applicable && active_refused==1 &&
            terminal_preflight==-1 && terminal_remove==-1 && terminal_complete==-1;
    }
    bool Passed() const {
        return applicability==FacetApplicability::Applicable && active_refused==1 &&
            terminal_preflight==1 && terminal_remove==1 && terminal_complete==1;
    }
    std::string Json() const {
        const char* label=applicability==FacetApplicability::Applicable ? "applicable" :
            applicability==FacetApplicability::Inapplicable ? "inapplicable" : "unspecified";
        const char* state=Passed() ? "complete" : Pending() ? "pending" : "invalid";
        return std::string("{\"applicability\":\"") + label +
            "\",\"state\":\"" + state +
            "\",\"active_refused\":" + std::to_string(active_refused) +
            ",\"terminal_preflight\":" + std::to_string(terminal_preflight) +
            ",\"terminal_remove\":" + std::to_string(terminal_remove) +
            ",\"terminal_complete\":" + std::to_string(terminal_complete) + "}";
    }
};

struct NamedOrderObservation {
    std::string site,order,trace;
    int callbacks=0,peer_pre=0,peer_post=0,original=0,skipped=-1;
    int64_t effective=0;
};
struct NamedOrderSnapshot {
    std::vector<NamedOrderObservation> rows;
    std::string run;
    bool requested=false,completion=false,late_installed=false,active_refused=false;
    int retired_callbacks=0;
};
struct NamedSnapshot {
    std::array<int,4> output_vector_dispatch{{0,0,0,0}},output_vector_original{{0,0,0,0}},output_vector_skipped{{-1,-1,-1,-1}};
    std::array<int,2> chat_original_each{{0,0}};
    int precache_filtered_dispatch=-1;
    std::array<int,3> usercmd_original_by_batch{{0,0,0}};
    int usercmd_argument_matches=0,usercmd_batch_first=-1,usercmd_batch_second=-1;

    bool installed = false;
    int chat_dispatch = 0, chat_original = 0;
    int chat_peer_before = 0, chat_peer_after = 0, chat_peer_order = 0;
    int chat_post_observed = 0;
    std::array<int,2> chat_actions{{-1,-1}}, chat_skipped{{-1,-1}};
    FacetObservation chat_current_return;
    int output_dispatch = 0, output_original = 0;
    int output_post_observed = 0;
    std::array<int,2> output_actions{{-1,-1}}, output_skipped{{-1,-1}};
    FacetObservation output_current_return;
    int usercmd_dispatch = 0, usercmd_neutralized = 0, usercmd_original = 0;
    int usercmd_return = 0, usercmd_nested_restored = 0, usercmd_expired = 0;
    int usercmd_ignore = 0, usercmd_post_observed = 0, usercmd_skipped = 0;
    int usercmd_current_return = 0, usercmd_current_return_matches = 0;
    std::vector<std::string> precache_manifest_trace;
    int precache_frame_receiver_matches=0,precache_frame_vtable_matches=0;
    int precache_dispatch = 0, precache_original = 0, precache_receiver_ok = 0;
    int precache_nested_restored = 0, precache_filtered_original = 0, precache_expired = 0;
    int precache_peer_before = 0, precache_peer_after = 0, precache_peer_order = 0;
    int precache_ignore = 0, precache_post_observed = 0, precache_skipped = 0;
    FacetObservation precache_current_return;
    MapGenerationSource precache_generation_source = MapGenerationSource::Unspecified;
    uint64_t precache_map_generation = 0, precache_observed_map_generation = 0;
    int precache_generation_observations = 0;
    FacetObservation bypass;
    NamedRemovalObservation removal;
    bool CorePassed() const {
        return installed &&
            chat_dispatch == 2 && chat_original == 1 && chat_peer_before == 2 &&
            chat_peer_after == 2 && chat_peer_order == 123123 && chat_post_observed == 2 &&
            chat_actions == std::array<int,2>{{0,2}} && chat_skipped == std::array<int,2>{{0,1}} &&
            chat_current_return.ExplicitlyInapplicable() &&
            output_dispatch == 2 && output_original == 1 && output_post_observed == 2 &&
            output_actions == std::array<int,2>{{0,2}} && output_skipped == std::array<int,2>{{0,1}} &&
            output_current_return.ExplicitlyInapplicable() &&
            usercmd_dispatch == 3 && usercmd_neutralized == 2 && usercmd_original == 3 &&
            usercmd_return == 37 && usercmd_nested_restored == 1 && usercmd_expired == 1 &&
            usercmd_ignore == 3 && usercmd_post_observed == 3 && usercmd_skipped == 0 &&
            usercmd_current_return == 37 && usercmd_current_return_matches == 3 &&
            precache_dispatch == 2 && precache_original == 2 && precache_receiver_ok == 2 &&
            precache_nested_restored == 1 && precache_filtered_original == 1 &&
            precache_expired == 1 && precache_peer_before == 2 && precache_peer_after == 2 &&
            precache_peer_order == 121233 && precache_ignore == 2 &&
            precache_post_observed == 2 && precache_skipped == 0 &&
            precache_current_return.ExplicitlyInapplicable() &&
            precache_generation_source == MapGenerationSource::LevelLifetime &&
            precache_map_generation != 0 &&
            precache_observed_map_generation == precache_map_generation &&
            precache_generation_observations == 2 && bypass.ExplicitlyInapplicable() &&
            removal.applicability == FacetApplicability::Applicable && removal.active_refused == 1;
    }
    bool InvocationPassed() const { return CorePassed() && removal.Pending(); }
    bool Passed() const { return CorePassed() && removal.Passed(); }
    std::string Json() const {
        return std::string("{\"installed\":") + (installed ? "true" : "false") +
            ",\"chat\":{\"dispatch\":" + std::to_string(chat_dispatch) +
            ",\"original\":" + std::to_string(chat_original) +
            ",\"peer_before\":" + std::to_string(chat_peer_before) +
            ",\"peer_after\":" + std::to_string(chat_peer_after) +
            ",\"peer_order\":" + std::to_string(chat_peer_order) +
            ",\"post_observed\":" + std::to_string(chat_post_observed) +
            ",\"actions\":[" + std::to_string(chat_actions[0]) + "," + std::to_string(chat_actions[1]) +
            "],\"skipped\":[" + std::to_string(chat_skipped[0]) + "," + std::to_string(chat_skipped[1]) +
            "],\"current_return\":" + chat_current_return.Json() + "}" +
            ",\"output\":{\"dispatch\":" + std::to_string(output_dispatch) +
            ",\"original\":" + std::to_string(output_original) +
            ",\"post_observed\":" + std::to_string(output_post_observed) +
            ",\"actions\":[" + std::to_string(output_actions[0]) + "," + std::to_string(output_actions[1]) +
            "],\"skipped\":[" + std::to_string(output_skipped[0]) + "," + std::to_string(output_skipped[1]) +
            "],\"current_return\":" + output_current_return.Json() + "}" +
            ",\"usercmd\":{\"dispatch\":" + std::to_string(usercmd_dispatch) +
            ",\"neutralized\":" + std::to_string(usercmd_neutralized) +
            ",\"original\":" + std::to_string(usercmd_original) +
            ",\"return\":" + std::to_string(usercmd_return) +
            ",\"nested_restored\":" + std::to_string(usercmd_nested_restored) +
            ",\"expired\":" + std::to_string(usercmd_expired) +
            ",\"ignore\":" + std::to_string(usercmd_ignore) +
            ",\"post_observed\":" + std::to_string(usercmd_post_observed) +
            ",\"skipped\":" + std::to_string(usercmd_skipped) +
            ",\"current_return\":" + std::to_string(usercmd_current_return) +
            ",\"current_return_matches\":" + std::to_string(usercmd_current_return_matches) + "}" +
            ",\"precache\":{\"dispatch\":" + std::to_string(precache_dispatch) +
            ",\"original\":" + std::to_string(precache_original) +
            ",\"receiver_ok\":" + std::to_string(precache_receiver_ok) +
            ",\"nested_restored\":" + std::to_string(precache_nested_restored) +
            ",\"filtered_original\":" + std::to_string(precache_filtered_original) +
            ",\"expired\":" + std::to_string(precache_expired) +
            ",\"peer_before\":" + std::to_string(precache_peer_before) +
            ",\"peer_after\":" + std::to_string(precache_peer_after) +
            ",\"peer_order\":" + std::to_string(precache_peer_order) +
            ",\"ignore\":" + std::to_string(precache_ignore) +
            ",\"post_observed\":" + std::to_string(precache_post_observed) +
            ",\"skipped\":" + std::to_string(precache_skipped) +
            ",\"current_return\":" + precache_current_return.Json() +
            ",\"generation_source\":\"" +
                (precache_generation_source==MapGenerationSource::LevelLifetime ? "level_lifetime" : "unspecified") +
            "\",\"map_generation\":" + std::to_string(precache_map_generation) +
            ",\"observed_map_generation\":" + std::to_string(precache_observed_map_generation) +
            ",\"generation_observations\":" + std::to_string(precache_generation_observations) + "}" +
            ",\"bypass\":" + bypass.Json() + ",\"removal\":" + removal.Json() + "}";
    }
};

using IntTarget = int (*)(int);

// The volatile function-pointer read is intentional. These functions are patched by KHook at
// runtime, which same-TU optimization cannot see; a direct call lets IPA discard hook side effects.
inline int InvokeOpaque(IntTarget volatile& target, int argument) {
    return target(argument);
}

template <typename Allocate, typename Release>
bool ReplaceOwnedCString(void* pointer_storage, const std::string& value,
                         Allocate allocate, Release release) {
    if (!pointer_storage) return false;
    char* replacement = allocate(value.size() + 1);
    if (!replacement) return false;
    std::memcpy(replacement, value.c_str(), value.size() + 1);
    char* previous = nullptr;
    std::memcpy(&previous, pointer_storage, sizeof(previous));
    std::memcpy(pointer_storage, &replacement, sizeof(replacement));
    if (previous) release(previous);
    return true;
}

class LevelLifetime {
public:
    void OnLevelInit() {
        if (!active_) {
            ++generation_;
        }
        active_ = true;
    }
    void OnLevelShutdown() { active_ = false; }
    bool MayTouchWorld() const { return active_; }
    bool MayTouchOwnedWorld(std::uint64_t owned_generation) const {
        return active_ && owned_generation != 0 && owned_generation == generation_;
    }
    std::uint64_t ClaimCurrentWorld() const { return active_ ? generation_ : 0; }
    std::uint64_t Generation() const { return generation_; }

private:
    bool active_ = false;
    std::uint64_t generation_ = 0;
};

struct PeerActionObservation {
    int pre_a;
    int pre_b;
    int original;
    int ret;
    int pre_order;
};

struct PeerActionsObservation {
    PeerActionObservation ab_io;
    PeerActionObservation ab_oo;
    PeerActionObservation ab_os;
    PeerActionObservation ba_io;
    PeerActionObservation ba_oo;
    PeerActionObservation ba_os;

    bool Passed() const {
        return ab_io.pre_a == 1 && ab_io.pre_b == 1 && ab_io.original == 1 &&
               ab_io.ret == 42 && ab_io.pre_order == 21 &&
               ab_oo.pre_a == 1 && ab_oo.pre_b == 1 && ab_oo.original == 1 &&
               ab_oo.ret == 99 && ab_oo.pre_order == 21 &&
               ab_os.pre_a == 1 && ab_os.pre_b == 1 && ab_os.original == 0 &&
               ab_os.ret == 99 && ab_os.pre_order == 21 &&
               ba_io.pre_a == 1 && ba_io.pre_b == 1 && ba_io.original == 1 &&
               ba_io.ret == 42 && ba_io.pre_order == 12 &&
               ba_oo.pre_a == 1 && ba_oo.pre_b == 1 && ba_oo.original == 1 &&
               ba_oo.ret == 7 && ba_oo.pre_order == 12 &&
               ba_os.pre_a == 1 && ba_os.pre_b == 1 && ba_os.original == 0 &&
               ba_os.ret == 99 && ba_os.pre_order == 12;
    }

    std::string Json() const {
        const auto pair = [](const PeerActionObservation& value) {
            return std::string("{\"pre_a\":") + std::to_string(value.pre_a) +
                   ",\"pre_b\":" + std::to_string(value.pre_b) +
                   ",\"orig\":" + std::to_string(value.original) +
                   ",\"ret\":" + std::to_string(value.ret) +
                   ",\"pre_order\":" + std::to_string(value.pre_order) + "}";
        };
        return std::string("{\"ab_io\":") + pair(ab_io) +
               ",\"ab_oo\":" + pair(ab_oo) +
               ",\"ab_os\":" + pair(ab_os) +
               ",\"ba_io\":" + pair(ba_io) +
               ",\"ba_oo\":" + pair(ba_oo) +
               ",\"ba_os\":" + pair(ba_os) + "}";
    }

    static std::string ExpectedJson() {
        return PeerActionsObservation{
            {1, 1, 1, 42, 21}, {1, 1, 1, 99, 21}, {1, 1, 0, 99, 21},
            {1, 1, 1, 42, 12}, {1, 1, 1, 7, 12}, {1, 1, 0, 99, 12},
        }.Json();
    }
};

struct OnceObservation {
    int pre;
    int post;
    int original;
    int ret;

    bool Passed() const { return pre == 1 && post == 1 && original == 1 && ret == 15; }

    std::string Json() const {
        return std::string("{\"pre\":") + std::to_string(pre) +
               ",\"post\":" + std::to_string(post) +
               ",\"orig\":" + std::to_string(original) +
               ",\"return\":" + std::to_string(ret) + "}";
    }

    static std::string ExpectedJson() { return OnceObservation{1, 1, 1, 15}.Json(); }
};

}  // namespace s2khook
