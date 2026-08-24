#include "core_symbols.h"

#include "s2script_core.h"

S2OptionalCoreEntries S2ResolveOptionalCoreEntries() {
    return {
        &s2script_core_dispatch_hook,
        &s2script_core_dispatch_hook_post,
    };
}
