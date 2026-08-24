#include "core_symbols.h"

#include "s2script_core.h"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

namespace {

template <typename T>
T OptionalEntry(HMODULE core, const char* name) {
    return core ? reinterpret_cast<T>(GetProcAddress(core, name)) : nullptr;
}

}  // namespace

S2OptionalCoreEntries S2ResolveOptionalCoreEntries() {
    // A required imported entry is a stable address anchor for the loaded core image. This avoids
    // assuming a basename while keeping both optional names out of s2script.dll's import table.
    HMODULE core = nullptr;
    if (!GetModuleHandleExA(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            reinterpret_cast<LPCSTR>(&s2script_core_init_v2), &core)) {
        return {};
    }
    return {
        OptionalEntry<S2CoreDispatchHookFn>(core, "s2script_core_dispatch_hook"),
        OptionalEntry<S2CoreDispatchHookPostFn>(core, "s2script_core_dispatch_hook_post"),
    };
}
