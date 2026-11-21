# Windows server port — design

**Status:** Slice W1 (build + boot) in progress, 2026-11-18. Not live-tested.
**Audience:** core and shim maintainers.
**Why now:** a server that runs Windows binaries is the prerequisite for any second Source 2 game
(Deadlock ships Windows server binaries only) and is useful on its own: today s2script runs only on
CS2's Linux dedicated server. The port is proven on CS2 Windows first, where every expected engine
fact can be cross-checked against the Linux build we already trust, so "the port is wrong" and "the
game facts are wrong" are never debugged at the same time.

---

## 1. What is Linux-only today

| Layer | Linux assumption | Windows equivalent |
|---|---|---|
| core (Rust) | 5 `dlopen`/`dlsym` calls in the native-extension loader; the gamedata platform key `linuxsteamrt64` in two consts | `LoadLibrary`/`GetProcAddress` (later slice); key `win64` |
| shim: module discovery | `dl_iterate_phdr` + ELF program headers (largest `PF_X` segment wins over Metamod's proxy) | `EnumProcessModules` + PE section headers (largest executable section wins) |
| shim: own path | `dladdr` + `dirname` | `GetModuleHandleExW(FROM_ADDRESS)` + `GetModuleFileNameW` |
| shim: symbols | `dlsym(RTLD_DEFAULT, …)` | `GetProcAddress` on the named module (`tier0.dll`) |
| shim: memory | `mincore`, `/proc/self/maps`, `mprotect`, `process_vm_readv` | `VirtualQuery`, `VirtualProtect`, `ReadProcessMemory` |
| shim: vtables by name | Itanium RTTI (`_ZTV…` typeinfo strings in `.rodata`) | MSVC RTTI (`.?AV<name>@@` type descriptors → complete object locators) |
| shim: engine calls | SysV x86-64 invoke and validators (`rdi`… argument registers) | Microsoft x64 (4 register args, shadow space) |
| shim: engine-function bridge | libffi `FFI_UNIX64`, built with autotools | libffi `FFI_WIN64` |
| shim: detours | own near-island allocator (`MAP_FIXED_NOREPLACE`) | `VirtualAlloc` near the target |
| shim: crash capture | Breakpad Linux client + signals | Breakpad Windows client + SEH |
| build | GCC/Clang ELF `.so`, `$ORIGIN` rpath to find core | clang-cl PE `.dll`, delay-loaded core preloaded from the plugin's own directory |
| gamedata | every entry keyed `linuxsteamrt64` | a `win64` key beside it; absent → that descriptor degrades with "other-platform" |

The gamedata format already has the platform axis (`MergeFile` takes the platform and reports
`other-platform` for an entry without it), and core already receives the platform from the shim for
game-package selection. No format change is needed.

## 2. The platform seam

One header, `shim/src/platform.h` (namespace `s2plat`), with two implementations,
`platform_posix.cpp` and `platform_win.cpp`, chosen by CMake. Everything that asks the OS about the
process goes through it:

- `PlatformKey()` — `"linuxsteamrt64"` / `"win64"`: the gamedata key and the `bin/` directory name.
- `SelfModulePath()` — the shim's own file path; `AddonRoot()` = three parents up.
- `ForEachModule(fn)` — every loaded module as a `ModuleView` (path, load base, segments: ELF
  `PT_LOAD`s or the PE header block plus its sections, each with file size, memory size and an
  executable flag). The callers keep their own policy on top, e.g. "the module whose name contains
  `server` and whose largest executable segment is the largest" (Metamod's thin proxy never wins).
- `FindExport(module, symbol)` — `dlsym`/`GetProcAddress`; `module` null means any loaded module.
- `IsReadable(p)`, `ExecutableRanges(fn)`, `PageSize()`, `ThreadId()`, `MakeDir(path)`.
- `PreloadSibling(file)` — Windows: load `<bin>/<file>` by full path (see below); Linux: no-op.
- Macros for file names: `S2_MODULE("server")` is `libserver.so` / `server.dll`, plus the Steam API,
  tier0 export scope and core library names.

Linux call sites move onto the seam with their behaviour unchanged; the seam is a refactor there,
not a rewrite. Subsystems whose Windows half is a real port (vtable-by-name, engine-call ABI,
engine-function bridge, detour islands, crash capture, native extensions) compile on Windows as a
stub that refuses with a named reason (`"<subsystem>: not yet ported to Windows"`). That is the
existing per-descriptor degradation doctrine applied to a platform: the framework boots, and each
missing capability says so.

## 3. Toolchain

The Windows build is a **cross build from Linux with clang-cl + lld-link** against the MSVC CRT and
Windows SDK fetched by `xwin` (the same splat `cargo xwin` uses for core). One toolchain for both
platforms, reproducible in the existing Linux CI, and the same compiler family as the Linux build.

Compatibility adjustments, all in `shim/cmake/Windows.cmake` or Windows-only shim files; the
vendored SDK stays byte-clean:

1. `__restrict` is defined empty. The SDK's `platform_compile_base.h` defines `RESTRICT` as
   `__restrict` for MSVC, and Clang rejects calling a non-restrict member through a restrict `this`
   (Valve's `Vector::IsZeroFast`, `bf_read`). An overlay header with `RESTRICT` empty does not work:
   clang-cl's MSVC include-stack lookup finds the SDK's own copy first. Stripping the qualifier
   changes only the mangled names of out-of-line restrict members, which would fail at link, never
   silently.
2. `_ALLOW_COMPILER_AND_STL_VERSION_MISMATCH`: the current MSVC STL asks for Clang 19; CI runners
   ship 18. Remove once the runner's clang is ≥ 19.
3. The static CRT (`/MT`, and `+crt-static` for core). Valve's `libprotobuf.lib` is built `/MT` and
   refuses to link otherwise; for core it means no MSVC redistributable is needed on the server.
4. `public/tier0/memoverride.cpp` is replaced by `shim/src/memoverride_win.cpp`. The SDK file's
   Windows half replaces the CRT's own malloc family against CRT internals the current UCRT lacks;
   the shim needs only its `operator new`/`delete` routing through `g_pMemAlloc` (one heap with the
   engine for compiled KV3/EKV code), and that half is what the Windows file carries.
5. Link-time leaves. A PE image cannot carry undefined symbols, and MSVC mangles a variable's type
   into its name. So the tier0 leaves that `tier1_shims.cpp` defines for Linux come from
   `tier0.lib` instead, `win_link_stubs.cpp` defines the mathlib constants with their real types,
   and `CEntitySystem::FindFirstEntityHandleByName` (a by-name EHandle EKV read, which on Linux binds
   lazily and is never hit) returns "no entity" until name lookup is ported.

Core is linked through its import library and **delay-loaded**. Windows does not search a plugin's
own directory for its dependencies, so `Load()` first calls `LoadLibraryExW` on
`<bin>/s2script_core.dll` by full path; the delay-load helper then binds to that module by name.

`scripts/build-windows.sh` drives the whole build and checks the result: core imports only system
DLLs, the shim imports only system DLLs and `tier0.dll`, delay-loads core and exports
`CreateInterface`. `S2_PLATFORM=win64 scripts/package-addon.sh` lays out `bin/win64/` and a Metamod
VDF pointing at it. The `windows` shard of `scripts/ci-native.sh` runs both.

## 4. Slices

| Slice | Delivers | Exit gate |
|---|---|---|
| **W1 build + boot** (this spec) | core and shim build for Windows; platform seam; deep subsystems stubbed with named reasons; `win64` packaging; a CI shard that cross-builds both | On a CS2 Windows server: Metamod loads s2script, V8 boots, a hello plugin's `OnPluginStart` prints, `OnGameFrame` ticks, `sm plugins list` answers |
| W2 module + RTTI | PE signature scan and MSVC RTTI vtable lookup; `win64` interface/offset/vtable gamedata | Schema, entities and game events work |
| W3 calling convention | Microsoft x64 engine-call invoke + validators; libffi `FFI_WIN64` bridge | Plugin-declared engine calls and engine functions run |
| W4 hooks + detours | detour islands via `VirtualAlloc`; remaining hook descriptors | SDKHooks, usercmd, chat hooks |
| W5 gamedata + treadmill | `win64` signatures for every core / sdkhooks / cs2 entry; `gdgen` PE loader | Self-test re-derives Windows signatures across two builds |
| W6 crash capture | Breakpad Windows client | A forced fault produces an incident on next boot |

## 5. Not in W1

Any `win64` signature value; the engine-call and engine-function ABIs; native extensions; crash
capture; Windows CI *execution* (nothing Windows-native runs in CI; the gate is that it builds, and
the live gate is a real server).
