# KHook vs SourceHook — research note (Metamod PR #223)

**Date:** 2026-09-14; corrected after implementation-source review the same day
**Primary source:** [alliedmodders/metamod-source PR #223](https://github.com/alliedmodders/metamod-source/pull/223)  
**s2script baseline:** vendored Metamod at `third_party/metamod-source` commit `26c03fa` (pre–PR-223, still ships SourceHook).  
**Post-merge Metamod inspected at:** `master` commit `7e24ce9e7a03` (“Improve khook plugin unload”), clone `/tmp/metamod-khook`.

---

## Executive summary

[PR #223](https://github.com/alliedmodders/metamod-source/pull/223) (**“Provide an alternative to SourceHook”**, author Kenzzer) **merged 2026-09-08** into `alliedmodders/metamod-source` `master` (merge commit [`a12f3cd5d7dcc6a255139ad0c8d988dd251c35ae`](https://github.com/alliedmodders/metamod-source/commit/a12f3cd5d7dcc6a255139ad0c8d988dd251c35ae)). It is **not a draft**; it is a **breaking Metamod 2.0–style change**: the entire `core/sourcehook/` tree is **removed** (~41k lines deleted) and replaced by **KHook**, vendored as submodule `third_party/khook` ([Kenzzer/KHook](https://github.com/Kenzzer/KHook)).

**What KHook is:** A C++17 detour **multiplexer** built on [SafetyHook](https://github.com/alliedmodders/safetyhook) (AlliedModders fork). KHook itself does not patch bytes; it **catalogues one detour per target address** (or one vtable-slot detour per `(vtable, index)`), JITs a trampoline that runs **PRE then original then POST** callbacks, and merges callback outcomes via `KHook::Action` / `KHook::Return<T>`. Metamod links KHook **standalone** into `metamod.*` and exposes a per-plugin `KHook::IKHook` through `ISmmAPI::GetDetourInterface`.

**Why replace SourceHook:** PR description ([PR #223 body](https://github.com/alliedmodders/metamod-source/pull/223)): Source 2 modding needs **non-vtable detours** and **single detour per address** across competing frameworks (CounterStrikeSharp, CS2Fixes, Swiftly, etc.); SourceHook is tightly bound to vtable hooking and Hookmangen; upgrading in place ≈ rewriting 90%; **Metamod 2.0 allows a hard break** with no dual-framework compatibility ([PR #223](https://github.com/alliedmodders/metamod-source/pull/223) — “Why blow up SourceHook?”).

**Target games / providers:** KHook is engine-agnostic; Metamod wires it in **Source 1** (`core/provider/source/`) and **Source 2** (`core/provider/source2/provider_source2.cpp`). CS2 uses the Source 2 provider path (`META_IS_SOURCE2` in `AMBuildScript` — [`/tmp/metamod-khook/AMBuildScript`](https://github.com/alliedmodders/metamod-source/blob/master/AMBuildScript)). There is **no CS2-specific fork** of KHook in the PR—CS2 is covered as **Linux x86_64** (Steam Runtime `linuxsteamrt64`).

**Plugin API bump:** `METAMOD_PLAPI_VERSION` / `PLAPI_MIN_VERSION` → **18** (“Introduction of KHook / Removal of SourceHook”) — [`core/metamod_plugins.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod_plugins.h) lines 67–70, [`core/ISmmPluginExt.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/ISmmPluginExt.h). Plugins reporting API **17** fail load with “Plugin uses old SourceHook Metamod build…” — [`core/metamod_plugins.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod_plugins.cpp) ~581–583.

---

## KHook — full API surface

Sources: [`third_party/khook/include/khook.hpp`](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp), [`third_party/khook/README.md`](https://github.com/Kenzzer/KHook/blob/master/README.md), implementation [`third_party/khook/src/detour.cpp`](https://github.com/Kenzzer/KHook/blob/master/src/detour.cpp), [`detour.hpp`](https://github.com/Kenzzer/KHook/blob/master/src/detour.hpp).

### Core types

| Symbol | Role | Source |
|--------|------|--------|
| `KHook::Action` | `Ignore`, `Override`, `Supersede` | `khook.hpp` L41–52 |
| `KHook::Return<T>` | `{ action, ret }`; `Return<void>` is action-only | `khook.hpp` L54–63 |
| `KHook::HookID_t` / `INVALID_HOOK` | Hook handle (`uint32_t`, `-1`) | `khook.hpp` L115–116 |
| `KHook::__Hook` / `Hook<RETURN>` | Base; stack-size helper `_copy_stack_size` | `khook.hpp` L65–107 |

**Action semantics** (README + `khook.hpp`):  
- **Ignore** — no effect on call/return.  
- **Override** — call original, replace return with callback value (PRE only for skip-original).  
- **Supersede** — skip original (PRE); on POST, original already ran ([README L31–32](https://github.com/Kenzzer/KHook/blob/master/README.md)).  
At pin `1e200e4`, the implementation accepts strictly higher actions: Ignore < Override < Supersede. The first return wins a tie, but a later Supersede replaces an earlier Override ([SaveReturnValue](https://github.com/Kenzzer/KHook/blob/1e200e4cc8e0badcb7cf941525268d6977f6a4e6/src/detour.cpp#L365)). The README's first-wins wording is incomplete.

### Low-level C API (`KHOOK_API`)

When `KHOOK_STANDALONE` is **not** defined (plugin builds), these inline to `__exported__khook->…` ([`khook.hpp` L2202–2278](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)). Metamod core defines `KHOOK_STANDALONE` + `KHOOK_EXPORT` and links the library ([`core/AMBuilder`](https://github.com/alliedmodders/metamod-source/blob/master/core/AMBuilder) L12–18).

| Function | Purpose |
|----------|---------|
| `SetupHook(void* function, void* context, void* removed_fn, void* pre, void* post, void* make_return, void* make_call_original, unsigned stack_size, bool async)` | **Inline/detour** at function address |
| `SetupVirtualHook(void** vtable, int index, …)` | **Vtable slot** hook |
| `RemoveHook(HookID_t, bool async, hook_removal_fn, context)` | Remove; async unload pattern in Metamod |
| `GetContextPtr()` / `GetContext<T>()` | Thread-local context in callbacks |
| `GetOriginalFunction()` | Trampoline to original (in callback) |
| `GetOriginalValuePtr()` / `GetOverrideValuePtr()` / `GetCurrentValuePtr(bool pop)` | Return-value buffers (thread-local) |
| `DestroyReturnValue()` | After callback loop |
| `FindOriginal(void*)` / `FindOriginalVirtual(void**, int)` | Bypass multiplexed detours |
| `LookupSignature(void* start, size_t, const char* sig)` | Sig scan on **original** bytes |
| `DoRecall` / `SaveReturnValue` | Re-enter hooked function with new args/return |
| `WasOriginalFunctionSkipped()` | POST: supersede happened |
| `Shutdown()` | Tear down all hooks (unsafe in callback) |

Member-function pointer helpers: `BuildMFP`, `FillMFP`, `ExtractMFP`, `GetVtableIndex` ([`khook.hpp` L124–191, L2088–2156](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)).

### High-level template wrappers (no SH-style macros)

| Class | Hook kind | Configure | Install |
|-------|-----------|-----------|---------|
| `KHook::Function<Ret, Args…>` | **Free function / absolute address** | `Configure(void*)` or function pointer | Constructor or `Configure` → `SetupHook` |
| `KHook::Member<Class, Ret, Args…>` | **Non-virtual member** (address) | `Configure(mfp)` or raw `void*` | Same |
| `KHook::Virtual<Class, Ret, Args…>` | **Virtual** | `Configure(mfp)` → vtable index, or `Configure(int index)` | **`Add(Class* this)`** / **`Remove(Class*)`**; **`AddGlobal`/`RemoveGlobal`** for shared vtable |

Virtual hooks defer patching until `Add` because index alone is insufficient ([README L88–89](https://github.com/Kenzzer/KHook/blob/master/README.md)). Per-instance filtering: `_KHook_Callback_Fixed` checks `_hooked_this` / `_hooked_global` before running plugin callbacks ([`khook.hpp` L1992–2002](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)).

**Call original / recall:**  
- `KHook::CallOriginal(f, args…)` — free or member ([`khook.hpp` L2174–2181](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)).  
- `Function::CallOriginal(args…)` / `Virtual::CallOriginal(this, args…)` — use `FindOriginal*` ([`khook.hpp` L655–657, L1873–1876](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)).  
- `KHook::Recall(…)` — thread-local re-invocation ([`khook.hpp` L363–371](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)).

### Detour mechanics (vtable vs inline)

`DetourCapsule` ([`detour.hpp` L49–104](https://github.com/Kenzzer/KHook/blob/master/src/detour.hpp)):

- **Inline / function address:** `safetyhook::InlineHook::create(detour_address, jit_handler)` — **one SafetyHook inline hook per unique address**; multiple KHook consumers share one capsule ([PR #223 body](https://github.com/alliedmodders/metamod-source/pull/223)).
- **Virtual:** `SetupVirtual` **rewrites vtable slot** to JIT entry (not mid-function inline) — [`detour.hpp` L96–103](https://github.com/Kenzzer/KHook/blob/master/src/detour.hpp).

JIT assumptions (PR description): save **all GPRs**, on x86_64 **all FP/XMM regs**, assume **112-byte** stack frame at first invocation, restore before each consumer callback ([PR #223](https://github.com/alliedmodders/metamod-source/pull/223)).

**Calling conventions:** Callbacks use the **same signature as the detoured function** (README). Linux x86_64 saves `rdi, rsi, rdx, rcx, r8, r9` + XMM0–7 ([`detour.cpp` L36–39](https://github.com/Kenzzer/KHook/blob/master/src/detour.cpp)); Windows x64 differs ([`detour.cpp` L31–34](https://github.com/Kenzzer/KHook/blob/master/src/detour.cpp)). `_copy_stack_size` accounts for Windows vs SysV stack args ([`khook.hpp` L83–103](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)).

### Supporting headers (KHook repo)

| Path | Role |
|------|------|
| `include/khook.hpp` | Public API (~2289 lines) |
| `include/khook/memory.hpp` | `KHook::Memory::SetAccess` — `mprotect` / `VirtualProtect` |
| `include/khook/asm.hpp`, `include/khook/asm/x86_64.hpp`, `x86.hpp` | JIT assembler |
| `src/detour.cpp`, `src/detour.hpp` | Multiplexing, JIT, hook lists |
| `src/ranges.cpp`, `src/ranges.hpp` | Tracked patched ranges |
| `src/main.cpp`, `src/main.hpp` | Library init (linked into `libkhook`) |
| `third_party/safetyhook/` | Inline hook backend (submodule) |

Build: C++17+, AMBuild primary ([README L11–18](https://github.com/Kenzzer/KHook/blob/master/README.md)). Tests: GoogleTest + gtest-parallel ([README L91–95](https://github.com/Kenzzer/KHook/blob/master/README.md)).

### Metamod plugin-facing surface

| Mechanism | Source |
|-----------|--------|
| `ismm->GetDetourInterface(id)` → `KHook::IKHook*` | [`core/ISmmAPI.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/ISmmAPI.h) L445; [`core/metamod.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod.cpp) L1079–1080 |
| `PLUGIN_SAVEVARS()` sets `KHook::__exported__khook` | [`core/ISmmPlugin.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/ISmmPlugin.h) L514–517 |
| Per-plugin `KHookImpl` on `CPlugin::m_khook` | [`core/metamod_khook.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod_khook.h); [`core/metamod_plugins.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod_plugins.h) L102 |
| `KHook::Shutdown()` on Metamod unload | [`core/metamod.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod.cpp) L557 |
| Async hook removal + `dlclose` gate (`Unloader`) | [`core/metamod_plugins.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod_plugins.cpp) L46–89; follow-up [`7e24ce9`](https://github.com/alliedmodders/metamod-source/commit/7e24ce9e7a03) |

`IKHook` mirrors the low-level API ([`khook.hpp` L2184–2200](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)).

---

## SourceHook (s2script vendored) — reference API

Source: [`third_party/metamod-source/core/sourcehook/sourcehook.h`](/workspace/third_party/metamod-source/core/sourcehook/sourcehook.h) (s2script pin `26c03fa`).

| Concept | SourceHook |
|---------|------------|
| Result protocol | `META_RES`: `MRES_IGNORED`, `MRES_HANDLED`, `MRES_OVERRIDE`, `MRES_SUPERCEDE` — L135–138 |
| Hook handler macros | `RETURN_META`, `RETURN_META_VALUE`, `RETURN_META_NEWPARAMS`, … — L597+ |
| Declarations | `SH_DECL_HOOKn`, `SH_DECL_HOOKn_void`, `SH_DECL_MANUALHOOKn` |
| Install | `SH_ADD_HOOK(iface, func, ptr, SH_MEMBER/SH_STATIC(handler), post)` — L710+ |
| Manual VP | `SH_ADD_MANUALHOOK`, `SH_MANUALHOOK_RECONFIGURE`, `SH_REMOVE_HOOK_ID` — L718+, sdkhooks |
| Call original | `SH_CALL`, `SH_MCALL` |
| Global ptr | `g_SHPtr` via `MetaFactory(MMIFACE_SOURCEHOOK)` — [`ISmmPlugin.h`](/workspace/third_party/metamod-source/core/ISmmPlugin.h) L514–516 |
| Multiplexing | Per **(vtable, slot)** or per **manual vtable entry**; separate detours per plugin at same slot unless shared hookman |
| Hook IDs | `SH_REMOVE_HOOK_ID` for manual hooks |

Hookmangen generates x86/x86_64 thunks ([`sourcehook_hookmangen*.cpp`](/workspace/third_party/metamod-source/core/sourcehook/)); PR #223 cites this as blocking full x64 SourceMod parity.

---

## KHook vs SourceHook — behavioral comparison

| Topic | SourceHook | KHook | Sources |
|-------|------------|-------|---------|
| **Primary model** | Vtable + manual VP hooks; optional detours via generated thunks | **Address-catalogued detours** (SafetyHook inline) + **vtable slot replace** | PR #223; `detour.hpp` |
| **Plugin API** | Macros + `META_RES` in handler | **`KHook::Return<T>`** return value | PR #223; `khook.hpp` |
| **PRE/POST** | `SH_ADD_HOOK(..., post bool)` | PRE/POST function pointers on `Function`/`Member`/`Virtual` | `khook.hpp` |
| **Skip original** | `MRES_SUPERCEDE` | `Action::Supersede` | `sourcehook.h`; `khook.hpp` |
| **Return override** | `MRES_OVERRIDE` + meta value ptrs | `Action::Override` / `Supersede` + `Return::ret` | both |
| **Parameter rewrite** | `RETURN_META_NEWPARAMS`, `SH_CALL` recall | `Recall`, `DoRecall`, `CallOriginal` | `sourcehook.h`; `khook.hpp` |
| **Multi-plugin same address** | Conflicting patches / hook chains per SH design | **One detour per address**; callbacks chained inside JIT | PR #223 |
| **Per-entity virtual** | `SH_ADD_MANUALHOOK` + `SH_MANUALHOOK_RECONFIGURE(slot)` | `Virtual::Add(entity*)` with `_hooked_this` filter | [`sdkhooks_vp.cpp`](/workspace/shim/src/sdkhooks_vp.cpp); `khook.hpp` L1843–2002 |
| **Per-interface singleton** | `SH_ADD_HOOK` on `m_gameClients`, etc. | `Virtual::AddGlobal(ptr)` or `Add` on known singleton | s2 sample [`plugin.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/samples/s2_sample_mm/src/plugin.cpp) L86–93 |
| **Thread safety** | SH context not thread-local (PR claim) | TLS for context/return ptrs ([PR #223](https://github.com/alliedmodders/metamod-source/pull/223)) | PR #223; `GetContextPtr` docs |
| **Hook install timing** | `g_SHPtr` at `Load` | `__exported__khook` null → **warn + INVALID_HOOK** if too early | `khook.hpp` L2207–2215 |
| **Unload** | `SH_REMOVE_HOOK` | `RemoveHook`; Metamod **async** removal before `dlclose` | `metamod_plugins.cpp` Unloader |

**No macro layer in KHook:** PR example uses `KHook::Virtual hook(&IServerGameDLL::GameInit, ctx, pre, post)` ([PR #223](https://github.com/alliedmodders/metamod-source/pull/223)).

---

## CS2 / Source 2 / linuxsteamrt64 relevance

| Claim | Evidence |
|-------|----------|
| Source 2 provider uses KHook | [`provider_source2.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/provider/source2/provider_source2.cpp) — e.g. `Hook_AllowDedicatedServers`, loop-mode hooks, `Hook_ClientCommand` with `Action::Supersede` |
| S2 sample plugin | [`samples/s2_sample_mm/src/plugin.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/samples/s2_sample_mm/src/plugin.cpp) — `KHook::Virtual<…>` + `Add(gameclients)` |
| **Linux x86_64 supported** | KHook README L7–8: “Windows (x86 & x86_64) and Linux (x86 & x86_64)” |
| **CS2 server ABI** | CS2 dedicated server on SteamRT is **64-bit Linux** (`linuxsteamrt64`); same as Metamod’s `PLATFORM_64BITS` / `-m64` builds (s2script uses this — [`shim/CMakeLists.txt`](/workspace/shim/CMakeLists.txt) L220–232) |
| **Not proven in this research** | No executed test on a live CS2 binary in this note; PR merged on Metamod `master`, s2script still pins pre-KHook Metamod for daily builds |

**Platform caveats from implementation:**

- **Linux vs Windows calling convention** in JIT register save ([`detour.cpp` L31–39](https://github.com/Kenzzer/KHook/blob/master/src/detour.cpp)).
- **GetVtableIndex** on Linux uses Itanium MFP layout ([`khook.hpp` L2145–2155](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)); Windows uses stub disassembly ([`khook.hpp` L2088–2131](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp)).
- **Vtable hook** makes slot RX-only after patch except during write ([`detour.hpp` L98–101](https://github.com/Kenzzer/KHook/blob/master/src/detour.hpp)).
- **Registration/removal timing:** typed helpers pass async=true; an existing capsule may queue callback insertion. A valid id is acceptance, not observed delivery. Removal can also defer; retain callback/context ownership until completion. Header comments do not precisely describe all implementation paths; verify the pinned implementation.
- **Apple / ARM:** KHook README lists no ARM; CS2 server is x86_64 only.

---

## PR #223 — file list (137 files)

Generated via GitHub API `pulls/223/files` (2026-09-14).

### Removed (representative)

- Entire **`core/sourcehook/**`** (headers, hookmangen, tests, generators) — 74+ files in s2script’s current vendor tree under [`third_party/metamod-source/core/sourcehook/`](/workspace/third_party/metamod-source/core/sourcehook/).

### Added / changed (Metamod repo)

| Area | Paths |
|------|-------|
| KHook submodule | `third_party/khook` (gitlink to Kenzzer/KHook) — [`.gitmodules`](https://github.com/alliedmodders/metamod-source/blob/master/.gitmodules) |
| Metamod ↔ KHook glue | [`core/metamod_khook.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod_khook.h), [`core/AMBuilder`](https://github.com/alliedmodders/metamod-source/blob/master/core/AMBuilder) |
| Plugin API | [`core/ISmmPlugin.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/ISmmPlugin.h), [`core/ISmmAPI.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/ISmmAPI.h), [`core/metamod_plugins.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod_plugins.cpp), [`core/metamod_plugins.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/metamod_plugins.h) |
| Providers | [`core/provider/source2/provider_source2.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/provider/source2/provider_source2.cpp), [`provider_source2.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/provider/source2/provider_source2.h), S1 [`provider_source.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/provider/source/provider_source.cpp), [`provider_source_console.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/provider/source/provider_source_console.cpp), [`vsp_bridge.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/vsp_bridge.cpp) |
| Loader | [`loader/gamedll.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/loader/gamedll.cpp), [`loader/serverplugin.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/loader/serverplugin.cpp), [`loader/AMBuilder`](https://github.com/alliedmodders/metamod-source/blob/master/loader/AMBuilder) |
| Samples | [`samples/s1_sample_mm/src/sample_mm.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/samples/s1_sample_mm/src/sample_mm.cpp), [`samples/s2_sample_mm/src/plugin.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/samples/s2_sample_mm/src/plugin.cpp), stub/sample build scripts |
| CI | [`.github/workflows/pr-checks.yml`](https://github.com/alliedmodders/metamod-source/blob/master/.github/workflows/pr-checks.yml) |
| Top-level | [`AMBuildScript`](https://github.com/alliedmodders/metamod-source/blob/master/AMBuildScript) |

### KHook repository file inventory (authoring; excluding vendored gtest/safetyhook sources)

| Path |
|------|
| `include/khook.hpp` |
| `include/khook/memory.hpp`, `include/khook/asm.hpp`, `include/khook/asm/x86_64.hpp`, `include/khook/asm/x86.hpp` |
| `src/detour.cpp`, `src/detour.hpp`, `src/main.cpp`, `src/main.hpp`, `src/ranges.cpp`, `src/ranges.hpp` |
| `test/*.cpp`, `test.py`, `AMBuilder`, `AMBuildScript`, `CMakeLists.txt`, `configure.py`, `PackageScript`, `README.md`, `LICENSE` |

---

## PR #223 — commits, review, known issues, follow-ups

### Status

- **Merged** 2026-09-08 ([PR #223](https://github.com/alliedmodders/metamod-source/pull/223)).
- **47 commits** on branch `k/sourcehook_alternative`; final PR head `1c6191a334f0768efc346cd67acfa7df1f29bd5a` (“Bump khook”).
- **Merge commit:** `a12f3cd5d7dcc6a255139ad0c8d988dd251c35ae`.

### Notable commit messages (fixes / behavior)

From `gh api …/pulls/223/commits`:

| Commit | Message |
|--------|---------|
| `fc7de97` | Introduce KHook |
| `2ba87cc` | Fix deadlock + Fix crash on x86 |
| `4aa9b8c` | Fix another deadlock |
| `e53af8a` | Slow down metamod load + fix memory flags |
| `ff1ba4f` | Add thread safety to return value |
| `1658b94` | Add Hook recall feature |
| `6152c62` | Fix stack size calculation on windows x86_64 |
| `8b3b3ec` | Added `KHook::LookupSignature` |
| `c95bd85` | Clean-up hooks on plugin unload |
| `1c6191a` | Bump khook (merge tip) |

### Review comments (issue comments only; **0** inline review threads)

| Author | Summary | URL |
|--------|---------|-----|
| GAMMACASE | Prefer KHook under `third_party/` not `core/` | [comment](https://github.com/alliedmodders/metamod-source/pull/223#issuecomment-2812973185) |
| Kenzzer | Submodule → AM transfer planned; location TBD | [comment](https://github.com/alliedmodders/metamod-source/pull/223#issuecomment-2812994264) |
| psychonic | `core` vs root vs `third_party` — no strong preference | [comment](https://github.com/alliedmodders/metamod-source/pull/223#issuecomment-2817232812) |
| Kenzzer | “Alright let's pull the plug!” (merge intent) | [comment](https://github.com/alliedmodders/metamod-source/pull/223#issuecomment-5588704611) |

### Follow-up PRs on Metamod (post-223, KHook-related)

| PR | Title | State |
|----|-------|-------|
| — | `Improve khook plugin unload` | Merged as [`7e24ce9`](https://github.com/alliedmodders/metamod-source/commit/7e24ce9e7a03) on `master` |
| — | `Bump khook` | [`bbdabac`](https://github.com/alliedmodders/metamod-source/commit/bbdabacaa4b2) on `master` |
| [#282](https://github.com/alliedmodders/metamod-source/pull/282) | Hide statically-linked symbols on Linux | Merged [`a8c72ea`](https://github.com/alliedmodders/metamod-source/commit/a8c72eaf29d4) |

Closed **without** merging: [#212 x64 linux sourcehook hookmangen](https://github.com/alliedmodders/metamod-source/pull/212) — obsoleted by KHook direction.

**Closes issues (per PR body):** #205, #176, #150, #215, #191, #134, #51.

---

## Compatibility and migration

### Can SourceHook and KHook coexist?

**In one Metamod build: no.** PR #223 deletes SourceHook entirely; `GetSHPtr` / `MetaFactory(MMIFACE_SOURCEHOOK)` path is gone from post-merge [`ISmmPlugin.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/ISmmPlugin.h). Kenzzer explicitly rejects dual-framework mode ([PR #223](https://github.com/alliedmodders/metamod-source/pull/223) — “providing two detouring framework… is just going to enable the initial problem to continue”).

**In one process:** Old plugins linked against SourceHook **cannot load** on KHook Metamod unless rewritten (PLAPI **18**, KHook hooks). s2script today ([`shim/CMakeLists.txt`](/workspace/shim/CMakeLists.txt) L195) still includes `${MMS}/core/sourcehook` and uses `PLUGIN_SAVEVARS` → `g_SHPtr` ([`s2script_mm.cpp`](/workspace/shim/src/s2script_mm.cpp) L4094).

### Migration path (Metamod perspective)

1. Bump vendored Metamod to **`master` ≥ merge commit** (recommend tip below).
2. Remove `#include <sourcehook.h>` / `SH_*` usage; add `#include <khook.hpp>`.
3. Replace `PLUGIN_SAVEVARS` / `PLUGIN_GLOBALVARS` with post-223 definitions (KHook export) — [`ISmmPlugin.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/ISmmPlugin.h).
4. Map each hook:
   - `SH_ADD_HOOK(I, F, ptr, handler, post)` → `KHook::Virtual<I, …>` member + `Add(ptr)` or `AddGlobal`.
   - `SH_ADD_MANUALHOOK` → `Virtual::Add(entity*)` + slot from `GetVtableIndex` / gamedata slot.
   - `RETURN_META(MRES_*)` → `return { KHook::Action::…, optional_value }`.
   - `SH_CALL` / `SH_MCALL` → `KHook::CallOriginal` / `FindOriginalVirtual`.
5. **Inline detours** (s2script `s2detour`, other frameworks): either migrate to `KHook::Function::Configure(addr)` for unified multiplexing, or keep private detours and accept **double-patch risk** (PR motivation).

**Metamod-internal hooks** (provider) already demonstrate S2 patterns — e.g. superseding `ClientCommand` for `meta` ([`provider_source2.cpp`](https://github.com/alliedmodders/metamod-source/blob/master/core/provider/source2/provider_source2.cpp) L476–490).

---

## Version / branch / commit pins (recommended)

| Component | Pin | Notes |
|-----------|-----|-------|
| **metamod-source** | **`7e24ce9e7a03`** (current `master` as of 2026-09-14) | Includes PR #223 + unload fix + khook bumps |
| **PR #223 merge** | **`a12f3cd5d7dcc6a255139ad0c8d988dd251c35ae`** | Minimum merge baseline |
| **KHook submodule** | **`1e200e4cc8e0badcb7cf941525268d6977f6a4e6`** | Recorded in Metamod `7e24ce9` [`.gitmodules` + submodule status] |
| **SafetyHook** (inside KHook) | **`ec3f698a1d9936d72c57c639536fbbedab6d7c8a`** | From KHook submodule init |
| **s2script today** | Metamod **`26c03fa`** | **Pre-KHook**; PLAPI **17** — [`ISmmPluginExt.h`](/workspace/third_party/metamod-source/core/ISmmPluginExt.h) L69 |

Branch: track **`alliedmodders/metamod-source` `master`**. KHook upstream URL still **`https://github.com/Kenzzer/KHook`** ([`.gitmodules`](https://github.com/alliedmodders/metamod-source/blob/master/.gitmodules)); PR notes eventual transfer to AlliedModders org.

---

## s2script — complete hook inventory (SourceHook + other engine touchpoints)

### A. SourceHook — global / interface hooks (`shim/src/s2script_mm.cpp`)

All use `SH_DECL_*` + `SH_ADD_HOOK` / `SH_REMOVE_HOOK` unless noted. Handlers in `S2ScriptPlugin` — [`shim/src/s2script_mm.h`](/workspace/shim/src/s2script_mm.h).

| # | Interface | Method | PRE/POST | Install timing | Handler(s) | Lines (decl / install) |
|---|-----------|--------|----------|----------------|------------|-------------------------|
| 1 | `ISource2Server` | `GameFrame` | PRE + POST | Lazy (`s2_request_hook` OnGameFrame) | `Hook_GameFramePre`, `Hook_GameFramePost` | decl L93; add L2888–2891 |
| 2 | `IGameEventManager2` | `FireEvent` | PRE | Lazy | `Hook_FireEventPre` | L96; L2904–2905 |
| 3 | `IGameEventSystem` | `PostEventAbstract` | PRE | Lazy (first usermsg sub) | `Hook_PostEvent` | L170; L1574–1575 |
| 4 | `ISource2GameClients` | `ClientCommand` | PRE | Load | `Hook_ClientCommand` | L101; L4225–4226 |
| 5 | `ISource2GameClients` | `OnClientConnected` | PRE | Load | `Hook_OnClientConnected` | L123; L4230–4231 |
| 6 | `ISource2GameClients` | `ClientPutInServer` | PRE | Load | `Hook_ClientPutInServer` | L124; L4232–4233 |
| 7 | `ISource2GameClients` | `ClientActive` | PRE | Load | `Hook_ClientActive` | L125; L4234–4235 |
| 8 | `ISource2GameClients` | `ClientFullyConnect` | PRE | Load | `Hook_ClientFullyConnect` | L126; L4236–4237 |
| 9 | `ISource2GameClients` | `ClientDisconnect` | PRE | Load | `Hook_ClientDisconnect` | L127; L4238–4239 |
| 10 | `ISource2GameClients` | `ClientSettingsChanged` | PRE | Load | `Hook_ClientSettingsChanged` | L128; L4240–4241 |
| 11 | `ISource2GameClients` | `ClientVoice` | POST | Load | `Hook_ClientVoice` | L136; L4245–4246 |
| 12 | `ISource2GameEntities` | `CheckTransmit` | POST | Load (after layout gate) | `Hook_CheckTransmit` | L163; L4278–4279 |
| 13 | `ICvar` | `DispatchConCommand` | PRE | Load | `Hook_DispatchConCommand` | L112; L4319–4320 |
| 14 | `IVEngineServer2` | `SetClientListening` | PRE | Load | `Hook_SetClientListening` | L137; L4341–4342 |
| 15 | `INetworkServerService` | `StartupServer` | POST | Load | `Hook_StartupServer` | L154; L4392–4394 |

**SourceHook infrastructure in shim:**

- `PLUGIN_SAVEVARS()` → `g_SHPtr` — [`s2script_mm.cpp`](/workspace/shim/src/s2script_mm.cpp) L4094.
- Include path: [`shim/CMakeLists.txt`](/workspace/shim/CMakeLists.txt) L195 `${MMS}/core/sourcehook`.
- `META_RES` usage in handlers — e.g. `Hook_FireEventPre` L5379–5418, `Hook_PostEvent` L5481–5512, `Hook_DispatchConCommand` L5530–5547, `Hook_SetClientListening` L5664–5668, `Hook_CheckTransmit` L5774+.
- **`SH_CALL`** once: `Hook_FireEventPre` → `IGameEventManager2::FireEvent` — L5406.

### B. SourceHook — per-entity manual VP hooks (`shim/src/sdkhooks_vp.cpp`)

Pattern: `SH_DECL_MANUALHOOK*` + `SH_ADD_MANUALHOOK` / `SH_MANUALHOOK_RECONFIGURE` / `SH_REMOVE_HOOK_ID`. Types map to SDKHooks VP names in core — [`core/src/sdkhooks.rs`](/workspace/core/src/sdkhooks.rs).

| Hook kind | SH manual hook | Return type | PRE/POST handlers | `SH_MCALL` original |
|-----------|----------------|-------------|-------------------|---------------------|
| StartTouch | `MHook_StartTouch` | void | yes / yes | — |
| Touch | `MHook_Touch` | void | yes / yes | — |
| EndTouch | `MHook_EndTouch` | void | yes / yes | — |
| Blocked | `MHook_Blocked` | void | yes / yes | — |
| Spawn | `MHook_Spawn` | void | yes / yes | — |
| Think | `MHook_Think` | void | yes / yes | — |
| PreThink | `MHook_PreThink` | void | yes / yes | — |
| PostThink | `MHook_PostThink` | void | yes / yes | — |
| Use | `MHook_Use` | void | yes / yes | — |
| GetMaxHealth | `MHook_GetMaxHealth` | int | PRE only | L302 |
| ShouldCollide | `MHook_ShouldCollide` | bool | PRE only | L315 |
| VPhysicsUpdate | `MHook_VPhysicsUpdate` | void | yes / yes | — |
| GroundEntChanged | `MHook_GroundEntChanged` | void | POST only | — |
| CanBeAutobalanced | `MHook_CanBeAutobalanced` | bool | PRE only | L328 |

Declarations: [`sdkhooks_vp.cpp`](/workspace/shim/src/sdkhooks_vp.cpp) L50–63. Install/remove: L340–376, L497, L543, L552.

### C. Custom inline detours (`s2detour` — **not** SourceHook)

Self-contained x86_64 prologue patch — [`shim/src/detour.cpp`](/workspace/shim/src/detour.cpp), [`detour.h`](/workspace/shim/src/detour.h). Documented as intentional alternative to SafetyHook ([`detour.h`](/workspace/shim/src/detour.h) L5–14).

| Target | Handler | Install site |
|--------|---------|--------------|
| `DispatchTraceAttack` (sig) | `Detour_DispatchTraceAttack` | [`s2script_mm.cpp`](/workspace/shim/src/s2script_mm.cpp) L4493–4494 |
| `Host_Say` / chat (sig) | `Detour_HostSay` | L4519–4520 |
| `FireOutputInternal` (sig) | `Hook_FireOutputInternal` | L4792–4793 |
| `ProcessUsercmds` (sig) | `Detour_ProcessUsercmds` | Lazy via `Shim_UsercmdHookInstall` L2467–2471 |
| Gamedata **`engine:hooks`** descriptors | Compiled thunks → `S2_HookInstall` | [`engine_hooks.cpp`](/workspace/shim/src/engine_hooks.cpp); shapes in [`hook_dispatch.h`](/workspace/shim/src/hook_dispatch.h) L20–26 |

Declarative hook shapes (ABI contract): `this_void`, `this_f32_i32_i32_i32`, `this_f32_i32_i64_i64`, `this_i64_i32_i64`, `this_i64_i64_i64` — [`hook_dispatch.h`](/workspace/shim/src/hook_dispatch.h); mirrored in [`core/src/gamedata_hooks.rs`](/workspace/core/src/gamedata_hooks.rs) L61–84. Up to **`S2_HOOK_MAX` (64)** slots — [`gamedata_hooks.rs`](/workspace/core/src/gamedata_hooks.rs) L41–45.

### D. Vtable slot swap (no SourceHook, no `s2detour` prologue)

| Target | Mechanism | File |
|--------|-----------|------|
| `OnPrecacheResource` on game rules class vtable | `WriteVtableSlot` + `Detour_OnPrecacheResource` | [`s2script_mm.cpp`](/workspace/shim/src/s2script_mm.cpp) L3694–3748, L5241–5245 |

Reason documented: prologue unsuitable for `s2detour::Install` (RIP-relative store) — L3498–3500.

### E. Core / comments only (no SH install in core)

Rust core references SourceHook for SDKHooks VP ops — [`core/src/sdkhooks.rs`](/workspace/core/src/sdkhooks.rs), [`core/src/usermsg.rs`](/workspace/core/src/usermsg.rs) (lazy SH install in shim). No direct `SH_*` in `core/`.

### F. `ISmmPlugin` usage

[`shim/src/s2script_mm.h`](/workspace/shim/src/s2script_mm.h) — `S2ScriptPlugin : public ISmmPlugin`; no override of `GetApiVersion()` → inherits **PLAPI 17** from vendored headers.

---

## Implications for s2script (summary)

1. **Metamod upgrade is a breaking shim port:** ~15 global SourceHook sites + 14× manual VP hooks + `META_RES`/`SH_CALL`/`RETURN_META_NEWPARAMS` must map to KHook patterns; `CMakeLists.txt` must include KHook headers instead of `core/sourcehook`.
2. **Per-entity SDKHooks** map conceptually to `KHook::Virtual::Add(CEntityInstance*)` but **slot configuration** today uses `SH_MANUALHOOK_RECONFIGURE` from gamedata — needs `Virtual::Configure(index)` + per-entity `Add`/`Remove`.
3. **Private `s2detour` layer** solves prologue/KHook gaps (precache, unrelocatable prologues) but **overlaps PR #223 goals**; migrating inline hooks to `KHook::Function` would unify multiplexing with other CS2 mods. The shim does **not** link SafetyHook — Metamod does.
4. **Live gate:** requires Metamod build that exports `GetDetourInterface` and **PLAPI 18**; current docker Metamod tarball in [`docker/metamod/`](/workspace/docker/metamod/) must be refreshed separately from this research.

---

## s2script-specific additions (this checkout)

These facts are load-bearing for the migration plan and were verified against `/workspace` on 2026-09-14.

### `SH_ADD_HOOK` last argument is SourceHook `post`

`false` = PRE, `true` = POST. The six client-lifecycle hooks are **PRE** (`s2script_mm.cpp` L4230–4241) even though `s2script_mm.h` L74 calls them “post-hooks” — that comment means *notify-only / do not alter flow*, not SourceHook POST. `ClientVoice` and `CheckTransmit` are genuine POST (`true`).

### `PLUGIN_SAVEVARS` today does not call `GetSHPtr`

Vendored `ISmmPlugin.h` L514–516 sets `g_SHPtr` via `ismm->MetaFactory(MMIFACE_SOURCEHOOK, NULL, NULL)`. The comment at `s2script_mm.cpp` L4094 (`GetSHPtr`) is stale. Post-223 `PLUGIN_SAVEVARS` sets `KHook::__exported__khook` from `ismm->GetDetourInterface(id)`.

### Voice mute is a Recall, not a return override

`Hook_SetClientListening` uses `RETURN_META_VALUE_NEWPARAMS(MRES_IGNORED, bListen, &IVEngineServer2::SetClientListening, (receiver, sender, false))` (`s2script_mm.cpp` L5664–5665). The original **still runs** with rewritten `bListen`. KHook equivalent is `KHook::Recall(&IVEngineServer2::SetClientListening, {KHook::Action::Ignore, bListen}, this, receiver, sender, false)` — not `Supersede`.

### `FireEvent` suppression is `SH_CALL` + `MRES_SUPERCEDE`

`Hook_FireEventPre` L5406–5416 re-invokes the original with a different `bDontBroadcast`, then SUPERCEDEs. Maps to `KHook::CallOriginal` (or `m_FireEvent.CallOriginal`) + `Action::Supersede`.

### `HookResult` (JS) is not `KHook::Action`

`core` collapses JS handlers with `Continue < Changed < Handled < Stop`. That is not a universal backend action mapping. Pointed-to events/damage info mutate in place and can Ignore; declarative by-value args must be forwarded with Recall. CanAcquire Changed is a return vote, folded with the engine decision, and can require a POST Override. Preserve Handled's implicit InvalidItem denial, bypass behavior, and the HUD-click notification ordering as well as the FireEvent/voice special cases above.

KHook's process merge accepts a strictly higher action and retains the first equal-priority return. Our JS multiplexer still collapses its own subscribers. A peer may skip the engine or win a return decision; JS POST sees the current effective result at its callback, and later peer POST callbacks can still change the eventual return.

### `Virtual::Remove` does not unpatch

`KHook::Virtual::Remove(this_ptr)` only erases the this-pointer filter (`khook.hpp` L1853–1858). The vtable-slot / SafetyHook trampoline stays until the `Virtual` object is destroyed or `Configure` changes the index. SDKHooks “last JS callback → `SH_REMOVE_HOOK_ID`” will no longer restore the original slot. That is acceptable (one detour per address) and cheaper; document it.

### `Function`/`Virtual` helpers add hooks with `async=true`

Both `_Configure` / `_Setup` pass `async=true`. At the pinned implementation a new capsule can insert immediately, but an existing capsule queues insertion. The helpers return void; neither that return nor `Virtual::IsActive()` proves delivery. The revised plan wraps the typed helpers with checked registration receipts, passive first-fire state and retained teardown ownership. Callback-safe filter removal is different from blocking physical destruction. No install-time engine call with dummy pointers is safe evidence.

### `KHook::Virtual` is VP-cost, not SourceHook `Hook_Normal`

`Add(entity)` still patches the **shared class vtable slot** (or the function it points at) once, then filters `this`. Every Touch/Think on every instance of that class enters the JIT, then returns immediately if `this` is not in `_hooked_this`. That is `SH_ADD_MANUALVPHOOK` cost, not per-instance vtable clone. High-frequency types (Think, Touch) will see more entries when one entity is hooked.

### Operator Metamod refresh

[`scripts/cloud/install.sh`](/workspace/scripts/cloud/install.sh) L67–75 downloads `https://mms.alliedmods.net/mmsdrop/2.0/mmsource-latest-linux` **only when** `docker/metamod/bin/linuxsteamrt64/metamod.2.cs2.so` is missing. A snapshot that already has 2.0.0.1403 will **keep the pre-KHook binary**. The live gate must force-replace that tree as part of PR A.

### `PLUGIN_EXPOSE` already grows the KHook export

Post-223 `PLUGIN_EXPOSE` in [`ISmmPlugin.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/ISmmPlugin.h) declares `namespace KHook { IKHook* __exported__khook = nullptr; }`. s2script already uses `PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin)` (`s2script_mm.cpp` L175) and `PLUGIN_GLOBALVARS()` in the header + `sdkhooks_vp.cpp`. After the pin, those macros are correct; do not add a second `__exported__khook` definition.

### Official bind pattern is ctor + `Add`, not `AddContext` first

Metamod’s s2 sample constructs each `KHook::Virtual` in the plugin constructor with `(mfp, this, pre, post)` and only `Add`s after `PLUGIN_SAVEVARS()`. That ctor calls `GetVtableIndex` only — it does **not** patch. `Function`/`Member` ctors that take an **address** call `Configure` → `SetupHook` immediately and **must not** run before `PLUGIN_SAVEVARS`.

`AddContext` exists (`khook.hpp`) and is for attaching extra context objects to an already-constructed helper. Do not use it as the primary bind for interface hooks.

`Configure(mfp)` on `Virtual` **returns void and silently no-ops** when `GetVtableIndex` is `-1` (`khook.hpp` L1902–1907). Call `KHook::GetVtableIndex` yourself and treat `-1` as a named boot fail.

### SDKHooks PRE and POST share one `Virtual`

Today `s2_sdkhook_vp_add(..., post)` installs **two** SourceHook ids for the same entity+kind (PRE and POST are separate `SH_ADD_MANUALHOOK`s, keyed `VpKey{p, kind, post}`). One `KHook::Virtual` already holds both callbacks. `Add(entity)` / `Remove(entity)` is a this-pointer filter for **both** phases. `Remove` when only PRE unsubscribes would also kill POST.

Rule: `Add(p)` on the first phase for that entity+kind; `Remove(p)` only when **both** PRE and POST rows for that entity+kind are gone. The FFI still returns `1`/`0` to core (`sdkhooks.rs` only tests non-zero); the shim-local `hook_id` field becomes unused.

### Named `s2detour` sites are not all “Ignore or Supersede”

| Site | Today | KHook |
|------|-------|-------|
| `DispatchTraceAttack` | PRE, original, POST; nested save/restore of info + victim; legacy exclusive sentinel probe | Shared checked Function; PRE/POST each scope both pointers; remove sentinel/dummy engine invocation; prove controlled native diversion and valid live damage separately. |
| `HostSay` | `suppress` skips orig | `Supersede` vs `Ignore` |
| `FireOutputInternal` | `result>=2` skips orig | `Supersede` vs `Ignore` |
| `ProcessUsercmds` | **always** calls orig; `Handled` mutates the cmd in place | **always `Ignore`** — never `Supersede` |

### Architecture thesis (`docs/ARCHITECTURE.md` §1)

s2script already cites Metamod issue [#215](https://github.com/alliedmodders/metamod-source/issues/215) as the reason competing CS2 frameworks fight over detours. PR #223 **is** the Metamod-level fix for #215. After migration, **Metamod/KHook owns the one process-wide detour**; s2script owns the JS `HookResult` contract among `.s2sp` plugins. Keeping `s2detour` on addresses other plugins also hook re-creates the #215 conflict with ourselves as the offending party.

---

## References (quick links)

- [PR #223 — Provide an alternative to SourceHook](https://github.com/alliedmodders/metamod-source/pull/223)
- [KHook README](https://github.com/Kenzzer/KHook/blob/master/README.md) / [`khook.hpp`](https://github.com/Kenzzer/KHook/blob/master/include/khook.hpp) / [repo](https://github.com/Kenzzer/KHook)
- [Metamod post-merge `ISmmPlugin.h`](https://github.com/alliedmodders/metamod-source/blob/master/core/ISmmPlugin.h)
- [Metamod s2 sample (`Virtual` ctor + `Add` after `PLUGIN_SAVEVARS`)](https://github.com/alliedmodders/metamod-source/blob/master/samples/s2_sample_mm/src/plugin.cpp)
- s2script vendored SourceHook: `third_party/metamod-source/core/sourcehook/sourcehook.h` (pin `26c03fa`)

## Corrections required by the execution plan

The original review found integration gaps beyond replacing API names:

- **Original bytes:** existing named, declarative and SDKHooks resolvers count/scan live
  module text, and validators decode it. Another KHook consumer may already have
  patched those bytes. The plan adds a verified original module view for lookup,
  uniqueness and decoding while preserving logical/live addresses and live
  range/liveness checks. LookupSignature alone is not a complete validator input.
- **Invocation lifetime:** synchronous Recall runs the remainder of the chain
  before returning; a scoped per-id invocation record can therefore keep ArgView
  and bypass/vote state alive through POST. Keep pointers to stable stack records,
  restore outer views, and do not treat Recall's returned decision as the final
  engine value.
- **CanAcquire:** retain the local vote, engine fold and implicit deny. Submit a
  necessary local Override with ManualReturn before reading GetCurrentReturn for
  JS POST. Do not claim Override when no vote changes the engine result.
- **Physical removal:** a KHook id differs from an s2script descriptor id.
  RemoveHook(..., true) does not permit immediate destruction of callback context.
- **PR atomicity:** the generated Metamod SHA changes with the pin. The native
  license-freshness gate requires regeneration in PR A; operator prerequisites and
  durable live-host refresh also accompany A.
- **Acceptance:** source compilation, grep hits and core synthetic liveness are
  not proofs of engine semantics. Exact native/JS fixture matrices are mandatory;
  required unavailable human-client checks stay pending.
- **Damage architecture:** typed damage dispatch is a transitional semantic
  adapter, not a distinct hooking mechanism. The KHook cutover shares installation
  and lifetime now; a later descriptor/borrowed-view slice removes the dedicated
  dispatch path. Ammo properties use schema accessors; ammo event interception
  should use that same shared hook substrate, not a new private installer.

See the revised design §§5.4–5.7 and implementation T1/T2/T8–T13. These
corrections do not claim that the migration or its acceptance has run.
