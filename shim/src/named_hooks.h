#pragma once

#include "engine_resolver.h"
#include "khook_binding.h"

#include <cstddef>
#include <cstdint>

class CEntityIOOutput;
class CEntityInstance;
class CEntityIdentity;
struct CPulseArgumentPack;
struct CPulseInputParamMap;
class CVariantDefaultAllocator;
template <typename A> class CVariantBase;
typedef CVariantBase<CVariantDefaultAllocator> CVariant;

enum class S2NamedHookSite { Chat, Output, Usercmd, Precache, Input, CvarResponse, UsercmdPost };

struct S2NamedHookOps {
    int (*chat)(void*, void*, bool, int, const char*) = nullptr;
    int (*output)(CEntityIOOutput*, CEntityInstance*, CEntityInstance*,
                  CPulseArgumentPack*, float, CPulseInputParamMap*, const CVariant*) = nullptr;
    int (*usercmd_slot)(void*) = nullptr;
    int (*usercmd_dispatch)(int) = nullptr;
    void (*usercmd_neutralize)() = nullptr;
    // Post phase: one command finished running (movement applied and written back). `service` is
    // the per-command run's receiver (the pawn's movement services), `message` the command's
    // protobuf. Read-only by construction: the frame it runs in has no mutable command.
    void (*usercmd_post_dispatch)(void* service, void* message) = nullptr;
    void (*precache)() = nullptr;
    // CEntityIdentity::AcceptInput: identity, input symbol (CUtlSymbolLarge*), activator, caller, value.
    int (*input)(CEntityIdentity*, void*, CEntityInstance*, CEntityInstance*, const CVariant*) = nullptr;
    void (*cvar_response)(void* client, const void* message) = nullptr;
    void (*observe_action)(S2NamedHookSite, bool post, KHook::Action) = nullptr;
};

void S2NamedHooksSetOps(const S2NamedHookOps& ops);
S2HookReceipt S2NamedConfigureChat(const void* target);
S2HookReceipt S2NamedConfigureOutput(const void* target);
S2HookReceipt S2NamedConfigureInput(const void* target);
void S2NamedSetUsercmdTarget(const void* target);
S2HookReceipt S2NamedInstallUsercmd();
// The per-command run (void(service, CUserCmd*)): the post phase hooks it, lazily, on the first
// post subscriber.
void S2NamedSetUsercmdPostTarget(const void* target);
S2HookReceipt S2NamedInstallUsercmdPost();
S2HookReceipt S2NamedConfigurePrecache(const s2resolve::VirtualSlotResolution& resolved);
S2HookReceipt S2NamedConfigureCvarResponse(const s2resolve::VirtualSlotResolution& resolved);
S2HookReceipt S2NamedHookSnapshot(S2NamedHookSite site);

// The command in scope (pre or post), for reads.
void* S2NamedCurrentUsercmd();
// The same command, but only in the pre phase: post is read-only, so every write goes through this
// and gets null there.
void* S2NamedCurrentUsercmdMutable();
// The receiver of the hook that scoped the current command, and which phase it is. Pre: the batch
// entry's receiver. Post: the movement services that ran the command. Null outside a dispatch.
void* S2NamedCurrentUsercmdReceiver(bool* post);
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
