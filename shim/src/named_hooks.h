#pragma once

#include "engine_resolver.h"
#include "khook_binding.h"

#include <cstddef>
#include <cstdint>

class CEntityIOOutput;
class CEntityInstance;
class CVariantDefaultAllocator;
template <typename A> class CVariantBase;
typedef CVariantBase<CVariantDefaultAllocator> CVariant;

enum class S2NamedHookSite { Chat, Output, Usercmd, Precache };

struct S2NamedHookOps {
    int (*chat)(void*, void*, bool, int, const char*) = nullptr;
    int (*output)(CEntityIOOutput*, CEntityInstance*, CEntityInstance*,
                  const CVariant*, float, void*, char*) = nullptr;
    int (*usercmd_slot)(void*) = nullptr;
    int (*usercmd_dispatch)(int) = nullptr;
    void (*usercmd_neutralize)() = nullptr;
    void (*precache)() = nullptr;
    void (*observe_action)(S2NamedHookSite, bool post, KHook::Action) = nullptr;
};

void S2NamedHooksSetOps(const S2NamedHookOps& ops);
S2HookReceipt S2NamedConfigureChat(const void* target);
S2HookReceipt S2NamedConfigureOutput(const void* target);
void S2NamedSetUsercmdTarget(const void* target);
S2HookReceipt S2NamedInstallUsercmd();
S2HookReceipt S2NamedConfigurePrecache(const s2resolve::VirtualSlotResolution& resolved);
S2HookReceipt S2NamedHookSnapshot(S2NamedHookSite site);

void* S2NamedCurrentUsercmd();
void* S2NamedCurrentPrecacheManifest();

bool S2NamedHooksCanUnloadSync(const S2HookTerminalPermit& permit);
bool S2NamedHooksUnloadSync(const S2HookTerminalPermit& permit);
bool S2NamedHooksRemovalComplete();

// Read-only native observation ABI. Borrowed identities are valid only during
// the actual production virtual callback. No ownership, dispatch or mutation.
struct S2NamedPrecacheFrameV1 {
    uint32_t version=0;
    uint32_t size=0;
    uint64_t serial=0;
    uintptr_t receiver=0;
    uintptr_t vtable=0;
    uintptr_t manifest=0;
};
#if defined(__ELF__) && defined(__GNUC__)
#define S2_NAMED_FRAME_EXPORT __attribute__((visibility("protected")))
#elif defined(__GNUC__)
#define S2_NAMED_FRAME_EXPORT __attribute__((visibility("default")))
#else
#define S2_NAMED_FRAME_EXPORT
#endif
extern "C" S2_NAMED_FRAME_EXPORT bool S2NamedReadPrecacheFrameV1(S2NamedPrecacheFrameV1* out,size_t size);
#undef S2_NAMED_FRAME_EXPORT
