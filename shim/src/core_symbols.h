#pragma once

using S2CoreDispatchHookFn = int (*)(int hookId, void* argView);
using S2CoreDispatchHookPostFn = int (*)(int hookId, void* argView, int skipped);

struct S2OptionalCoreEntries {
    S2CoreDispatchHookFn dispatch = nullptr;
    S2CoreDispatchHookPostFn dispatchPost = nullptr;
};

// Resolve version-tolerant core entries without turning their absence into a loader failure.
S2OptionalCoreEntries S2ResolveOptionalCoreEntries();
