#pragma once

#include <string>

namespace s2platform {

// Resolve the module containing `anchor`, then walk three parents from
// addons/s2script/bin/<platform>/<shim> to the addon root. Empty means unavailable.
std::string AddonRoot(const void* anchor);

// Engine-free lexical half, exposed so both slash conventions and malformed paths are testable.
std::string AddonRootFromModulePath(const std::string& modulePath);

}  // namespace s2platform
