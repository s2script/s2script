#include <cstdint>

// Separate call site defeats same-TU virtual devirtualization and reloads the live slot that
// stock KHook patches. The fixture supplies a valid object and validated index.
extern "C" __attribute__((noinline)) void S2ProbeNamedInvokeVirtual(void* object,int index,void* manifest) {
    if (!object || index<0) return;
    void** vtable=*reinterpret_cast<void***>(object);
    if (!vtable) return;
    reinterpret_cast<void (*)(void*,void*)>(vtable[index])(object,manifest);
}
