#include "vtable.h"

#include "platform/image.h"

namespace s2vtable {

void** GetVTableByName(const char* module, const char* className) {
    return s2platform::FindVTableByName(module, className);
}

}  // namespace s2vtable
