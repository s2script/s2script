/*
 * pstdint.h for the vendored HDE (Slice 6.6, Linux build).
 *
 * The upstream HDE (via MinHook) shipped a Windows-only shim that #included <windows.h> and
 * typedef'd the int types from Windows types. On our Linux/GCC (Steam Runtime 3 sniper) target the
 * standard <stdint.h> is always available and provides the exact int8_t..uint64_t HDE needs, so this
 * compatibility header simply forwards to it. (HDE's own hde64.c/h are unchanged.)
 */
#pragma once
#include <stdint.h>
