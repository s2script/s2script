#pragma once
#include "original_module.h"
#include "call_validate.h"
#include <functional>

namespace s2resolve {
enum class Kind { Signature, Virtual };
enum class TargetUse { Executable, MappedAddress };
struct TargetRecipe {
    Kind kind = Kind::Signature;
    TargetUse use = TargetUse::Executable;
    std::string module, pattern, strategy = "direct";
    std::string class_name, validate_json;
    int vtable_index = -1;
};
struct Resolution {
    uintptr_t address = 0;
    std::shared_ptr<const s2original::Image> image;
    std::string recipe, validation_receipt;
};
// All external contacts of the recipe evaluator. mapped never dereferences its address;
// read_live must check its entire span before reading. Neither supplies instruction bytes.
struct Sources {
    std::shared_ptr<const s2original::Image> image;
    std::function<bool(uintptr_t, size_t)> mapped;
    std::function<bool(uintptr_t, void*, size_t)> read_live;
    s2validate::Ops ops;
};
bool Evaluate(const TargetRecipe& recipe, const Sources& sources, Resolution& out, std::string& reason);
bool Resolve(const TargetRecipe& recipe, Resolution& out, std::string& reason);
}
