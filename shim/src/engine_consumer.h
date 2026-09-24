#pragma once

#include "engine_resolver.h"
#include "gamedata.h"

#include <functional>
#include <string>
#include <vector>

namespace s2consumer {

using Resolver = std::function<bool(const s2resolve::TargetRecipe&,
                                    s2resolve::Resolution&, std::string&)>;
using Reporter = std::function<void(const char*, bool, const char*)>;
using LiveReader = std::function<bool(uintptr_t, void*, size_t)>;
using OriginalVirtual = std::function<void*(void**, int)>;

bool ResolveEngineTarget(const char* kind, const char* module, const char* pattern,
                         const char* strategy, const char* class_name, int vtable_index,
                         const char* validate_json, s2resolve::Resolution& out,
                         std::string& reason, Resolver resolver = s2resolve::Resolve);

bool ResolveNamedSignature(const char* name, const SigSpec& sig, s2resolve::TargetUse use,
                           s2resolve::Resolution& out, std::string& reason,
                           Reporter reporter, Resolver resolver = s2resolve::Resolve);

bool ResolveBuiltinOffset(const char* name, const SigSpec& sig,
                          std::vector<s2resolve::Resolution>& retained, int64_t& offset,
                          std::string& reason, Reporter reporter,
                          Resolver resolver = s2resolve::Resolve);

bool FindSdkhookSlot(const s2resolve::Resolution& resolved, void** vtable, int max_slots,
                     int& slot, std::string& reason, LiveReader read_live,
                     OriginalVirtual original_virtual);

class CallRecords {
public:
    explicit CallRecords(size_t capacity) : capacity_(capacity) {}
    int Add(s2resolve::Resolution resolved, std::string& reason);
    uintptr_t Address(int id) const;
    bool CopyForAddress(const void* address, s2resolve::Resolution& out) const;

private:
    size_t capacity_;
    std::vector<s2resolve::Resolution> records_;
};

} // namespace s2consumer
