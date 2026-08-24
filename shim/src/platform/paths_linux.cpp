#include "paths.h"

#include <dlfcn.h>

namespace s2platform {

std::string AddonRoot(const void* anchor) {
    if (!anchor) return {};
    Dl_info info{};
    if (!dladdr(anchor, &info) || !info.dli_fname) return {};
    return AddonRootFromModulePath(info.dli_fname);
}

}  // namespace s2platform
