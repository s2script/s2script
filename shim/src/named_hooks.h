#pragma once

#include "engine_resolver.h"
#include "khook_binding.h"

#include <cstddef>

class CEntityIOOutput;
class CEntityInstance;
class CVariant;

struct S2NamedHookOps {
    void (*damage_pre)() = nullptr;
    void (*damage_post)() = nullptr;
    int (*chat)(void*, void*, bool, int, const char*) = nullptr;
    int (*output)(CEntityIOOutput*, CEntityInstance*, CEntityInstance*,
                  const CVariant*, float, void*, char*) = nullptr;
    int (*usercmd_slot)(void*) = nullptr;
    int (*usercmd_dispatch)(int) = nullptr;
    void (*usercmd_neutralize)() = nullptr;
    void (*precache)() = nullptr;
};

enum class S2NamedHookSite { Damage, Chat, Output, Usercmd, Precache };

void S2NamedHooksSetOps(const S2NamedHookOps& ops);
S2HookReceipt S2NamedConfigureDamage(const void* target);
S2HookReceipt S2NamedConfigureChat(const void* target);
S2HookReceipt S2NamedConfigureOutput(const void* target);
void S2NamedSetUsercmdTarget(const void* target);
S2HookReceipt S2NamedInstallUsercmd();
S2HookReceipt S2NamedConfigurePrecache(const s2resolve::VirtualSlotResolution& resolved);
S2HookReceipt S2NamedHookSnapshot(S2NamedHookSite site);

void* S2NamedDamageInfo();
void* S2NamedDamageVictim();
void* S2NamedCurrentUsercmd();
void* S2NamedCurrentPrecacheManifest();

bool S2NamedDispatchSyntheticDamage(void* victim, void* info);

bool S2NamedHooksCanUnloadSync(const S2HookTerminalPermit& permit);
bool S2NamedHooksUnloadSync(const S2HookTerminalPermit& permit);
bool S2NamedHooksRemovalComplete();
