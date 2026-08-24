#if defined(S2_ABI_PROBE_MICROSOFT) && !defined(_WIN32)
#define S2_HOOK_ABI __attribute__((ms_abi))
#else
// Microsoft x64 has one native calling convention; MSVC rejects legacy __fastcall annotations as
// meaningful distinctions on x64, so the compiler default is the exact ABI we need.
#define S2_HOOK_ABI
#endif
#define S2_HOOK_MICROSOFT 1
#include "hook_abi_impl.inc"
