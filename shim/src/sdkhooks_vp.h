#pragma once
// Per-entity SDKHooks VP hooks (wiki Touch family). Reads gamedata/sdkhooks, never s_gdCore.
// See docs/superpowers/specs/2026-08-30-sdkhooks-virtuals-design.md.
struct GameConfig;

// Resolve Touch-family signatures, derive vtable slots, SH_MANUALHOOK_RECONFIGURE.
void S2SdkhooksVpLoad(const GameConfig& gd);
// SH_REMOVE leftover per-entity hooks before the core isolate dies.
void S2SdkhooksVpUnload();

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
