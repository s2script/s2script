#pragma once

#include <cstddef>
#include <cstdint>
#include <string>

namespace s2platform {

// The two views needed by signature resolution and validators: executable code plus the complete
// mapped image. `imageBase` and `path` are retained for platform RTTI lookup.
struct ModuleView {
    const uint8_t* text = nullptr;
    size_t textSize = 0;
    const uint8_t* lo = nullptr;
    const uint8_t* hi = nullptr;
    uintptr_t imageBase = 0;
    std::string path;

    bool ContainsExecutable(const void* address) const {
        if (!text || !address) return false;
        const auto* p = static_cast<const uint8_t*>(address);
        return p >= text && p < text + textSize;
    }
};

// Find the loaded module whose name/path contains `name`. If more than one matches, the module
// owning the largest executable segment wins (the Metamod proxy disambiguation rule).
ModuleView FindModule(const char* name);

// Find the loaded image and executable segment containing `address`.
bool ModuleViewForAddress(const void* address, ModuleView* out);
bool IsExecutableAddress(const void* address);
bool IsReadableAddress(const void* address, size_t size);

}  // namespace s2platform
