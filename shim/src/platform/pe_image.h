#pragma once

#include "module.h"

#include <cstddef>
#include <cstdint>

namespace s2platform {

// Parse a PE32+ image in loaded layout. Every header, section, and image-size access is bounded by
// `available`; malformed images simply return false.
bool ParsePeImage(const uint8_t* image, size_t available, ModuleView* out);

// Return the executable section containing `address`, while retaining the complete image bounds.
// Unlike ParsePeImage's named-module scan range, this considers every executable section.
bool PeModuleViewForAddress(const uint8_t* image, size_t available, const void* address,
                            ModuleView* out);

// Resolve an MSVC x64 primary vtable from TypeDescriptor -> CompleteObjectLocator -> vtable[-1].
// `image` is a loaded-layout PE image, not raw file layout.
void** FindMsvcVTableInPe(const uint8_t* image, size_t available, const char* className);

}  // namespace s2platform
