#include "engine_consumer.h"

#include <algorithm>
#include <limits>
#include <utility>

namespace s2consumer {
namespace {

const char* text(const char* value) { return value ? value : ""; }

void report(const Reporter& reporter, const char* name, bool ok, const std::string& reason) {
    if (reporter) reporter(name, ok, ok ? nullptr : reason.c_str());
}

bool legacy_offset(const s2resolve::Resolution& resolved, int64_t& offset, std::string& reason) {
    if (!resolved.image || resolved.image->executable_segments().empty()) {
        reason = "resolved target has no verified executable image";
        return false;
    }
    const auto& segments = resolved.image->executable_segments();
    const auto base = std::max_element(segments.begin(), segments.end(),
        [](const s2original::Segment& a, const s2original::Segment& b) {
            return a.bytes.size() < b.bytes.size();
        })->live_begin;
    if (resolved.address >= base) {
        const uintptr_t distance = resolved.address - base;
        if (distance > static_cast<uintptr_t>(std::numeric_limits<int64_t>::max())) {
            reason = "resolved target offset does not fit int64";
            return false;
        }
        offset = static_cast<int64_t>(distance);
        return true;
    }
    const uintptr_t distance = base - resolved.address;
    const uint64_t negative_limit = uint64_t(std::numeric_limits<int64_t>::max()) + 1;
    if (uint64_t(distance) > negative_limit) {
        reason = "resolved target offset does not fit int64";
        return false;
    }
    offset = distance == negative_limit ? std::numeric_limits<int64_t>::min()
                                        : -static_cast<int64_t>(distance);
    return true;
}

} // namespace

bool ResolveEngineTarget(const char* kind, const char* module, const char* pattern,
                         const char* strategy, const char* class_name, int vtable_index,
                         const char* validate_json, s2resolve::Resolution& out,
                         std::string& reason, Resolver resolver) {
    s2resolve::TargetRecipe recipe;
    if (kind && std::string(kind) == "signature") recipe.kind = s2resolve::Kind::Signature;
    else if (kind && std::string(kind) == "vtable") recipe.kind = s2resolve::Kind::Virtual;
    else {
        out = {};
        reason = "unknown target kind";
        return false;
    }
    recipe.use = s2resolve::TargetUse::Executable;
    recipe.module = module && module[0] ? module : "libserver.so";
    recipe.pattern = text(pattern);
    recipe.strategy = strategy && strategy[0] ? strategy : "direct";
    recipe.class_name = text(class_name);
    recipe.vtable_index = vtable_index;
    recipe.validate_json = validate_json && validate_json[0] ? validate_json : "{}";
    return resolver && resolver(recipe, out, reason);
}

bool ResolveNamedSignature(const char* name, const SigSpec& sig, s2resolve::TargetUse use,
                           s2resolve::Resolution& out, std::string& reason,
                           Reporter reporter, Resolver resolver) {
    s2resolve::TargetRecipe recipe;
    recipe.kind = s2resolve::Kind::Signature;
    recipe.use = use;
    recipe.module = sig.module;
    recipe.pattern = sig.pattern;
    recipe.strategy = sig.resolve.empty() ? "direct" : sig.resolve;
    recipe.validate_json = sig.validate.empty() ? "{}" : sig.validate;
    if (resolver && resolver(recipe, out, reason)) return true;
    if (reason.empty()) reason = "resolver unavailable";
    report(reporter, name, false, reason);
    return false;
}

bool ResolveBuiltinOffset(const char* name, const SigSpec& sig,
                          std::vector<s2resolve::Resolution>& retained, int64_t& offset,
                          std::string& reason, Reporter reporter, Resolver resolver) {
    const bool data_recipe = sig.resolve == "ctor-body-xref" || sig.resolve == "lea-disp";
    s2resolve::Resolution resolved;
    if (!ResolveNamedSignature(name, sig,
            data_recipe ? s2resolve::TargetUse::MappedAddress : s2resolve::TargetUse::Executable,
            resolved, reason, reporter, std::move(resolver))) return false;
    if (!legacy_offset(resolved, offset, reason)) {
        report(reporter, name, false, reason);
        return false;
    }
    retained.push_back(resolved);
    report(reporter, name, true, reason);
    return true;
}

bool FindSdkhookSlot(const s2resolve::Resolution& resolved, void** vtable, int max_slots,
                     int& slot, std::string& reason, LiveReader read_live,
                     OriginalVirtual original_virtual) {
    slot = -1;
    if (!resolved.image || !vtable || max_slots <= 0 || !read_live || !original_virtual) {
        reason = "SDKHook slot lookup unavailable";
        return false;
    }
    const uintptr_t begin = reinterpret_cast<uintptr_t>(vtable);
    for (int i = 0; i < max_slots; ++i) {
        const size_t byte_offset = size_t(i) * sizeof(void*);
        if (byte_offset / sizeof(void*) != size_t(i) ||
            begin > std::numeric_limits<uintptr_t>::max() - byte_offset) break;
        void* live = nullptr;
        if (!read_live(begin + byte_offset, &live, sizeof live)) break;
        void* original = original_virtual(vtable, i);
        if (!resolved.image->executable(reinterpret_cast<uintptr_t>(original))) break;
        if (reinterpret_cast<uintptr_t>(original) == resolved.address) {
            slot = i;
            reason.clear();
            return true;
        }
    }
    reason = "sig-resolved address is not a vtable slot";
    return false;
}

int CallRecords::Add(s2resolve::Resolution resolved, std::string& reason) {
    for (size_t i = 0; i < records_.size(); ++i)
        if (records_[i].address == resolved.address) return static_cast<int>(i);
    if (records_.size() >= capacity_) {
        reason = "engine-call table is full";
        return -1;
    }
    records_.push_back(std::move(resolved));
    reason.clear();
    return static_cast<int>(records_.size() - 1);
}

uintptr_t CallRecords::Address(int id) const {
    if (id < 0 || static_cast<size_t>(id) >= records_.size()) return 0;
    return records_[static_cast<size_t>(id)].address;
}

bool CallRecords::CopyForAddress(const void* address, s2resolve::Resolution& out) const {
    const uintptr_t target = reinterpret_cast<uintptr_t>(address);
    for (const auto& record : records_) {
        if (record.address == target) {
            out = record;
            return true;
        }
    }
    out = {};
    return false;
}

} // namespace s2consumer
