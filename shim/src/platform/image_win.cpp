#include "image.h"

#include "module.h"
#include "pe_image.h"

namespace s2platform {

void** FindVTableByName(const char* module, const char* className) {
    if (!module || !className || !className[0]) return nullptr;
    const ModuleView image = FindModule(module);
    if (!image.lo || !image.hi || image.hi <= image.lo) return nullptr;
    return FindMsvcVTableInPe(image.lo, static_cast<size_t>(image.hi - image.lo), className);
}

}  // namespace s2platform
