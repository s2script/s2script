#pragma once
#include "khook_binding.h"
// Per-entity SDKHooks VP hooks (wiki Touch + lifecycle families). Reads gamedata/sdkhooks, never s_gdCore.
// See docs/superpowers/specs/2026-08-30-sdkhooks-virtuals-design.md.
struct GameConfig;

// Resolve Touch-family signatures, derive vtable slots, Virtual::Configure(slot).
void S2SdkhooksVpLoad(const GameConfig& gd);
bool S2SdkhooksVpCanUnloadSync(const S2HookTerminalPermit& permit);
bool S2SdkhooksVpUnloadSync(const S2HookTerminalPermit& permit);
bool S2SdkhooksVpRemovalComplete();

// Boot-banner helper defined in s2script_mm.cpp (increments the gamedata OK/FAIL counters).
void S2GamedataResult(const char* name, bool ok, const char* reason);

#ifdef __cplusplus
extern "C" {
#endif
int s2_sdkhook_vp_add(int index, int serial, const char* type, int post);
int s2_sdkhook_vp_remove(int index, int serial, const char* type, int post);
int s2_sdkhook_vp_drop(int index, int serial);
#ifdef __cplusplus
}
#endif
