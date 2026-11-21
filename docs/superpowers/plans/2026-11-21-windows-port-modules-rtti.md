# Windows port W2: PE module images, MSVC RTTI, first win64 gamedata

Spec: `docs/superpowers/specs/2026-11-18-windows-port-design.md` (slice W2). Exit gate: on a
Windows CS2 server the GameFrame hook installs, plugins load at map start, the hello plugin
prints, and schema, entities and game events work.

## Why

The W1 live boot (2026-11-21, CS2 1.42.0.0, Metamod 2.0.0-dev+1491) loaded s2script and ticked
for 5.5 minutes, but every gamedata entry failed (`0 ok, 72 FAILED`): no `win64` keys, and the
production resolver (signatures, virtual slots) refused off Linux. So the GameFrame hook never
installed and no plugin loaded.

## Tasks

1. **PE original image.** `s2original::FromPe(file, identity, read_live, reason)`: a portable
   PE32+ x64 parser (headers, sections, debug directory, CodeView RSDS). The loaded header bytes
   up to the end of the section table must equal the file's except ImageBase; the live CodeView
   record must equal the file's. Executable sections are the instruction segments (file bytes,
   `min(VirtualSize, SizeOfRawData)`); readable sections plus the headers are readable ranges.
   Shared caching moved to `Image::Intern`, used by `FromElf` too. Fixture:
   `shim/tests/pe_fixture.h`, tests in `original_module_test.cpp`.
2. **Windows discovery.** `OpenLoadedModule` on `_WIN32`: select by `s2plat::ForEachModule`
   (largest executable section among path matches), open by `GetModuleFileNameW`, identity from
   `GetFileInformationByHandle` (refuse if the file changes during the read), `read_live` by
   `ReadProcessMemory`.
3. **MSVC RTTI.** `Identity::format` selects the RTTI scheme in
   `GetVTableByName(const Image&, ...)`. MSVC: `.?AV<name>@@\0` / `.?AU<name>@@\0` name, descriptor
   = name − 16, a 4-aligned dword equal to the descriptor RVA at locator + 12 with signature 1,
   offset 0 and self RVA equal to the locator RVA, then an 8-aligned qword equal to the locator
   address; the vftable starts after it. More than one winner fails. Fixture test in
   `engine_resolver_test.cpp`.
4. **Resolver on Windows.** `production_sources` / `Resolve` / `ResolveVirtualSlot` compile on
   `_WIN32` (KHook's `FindOriginalVirtual` is the same).
5. **Gamedata.** `win64` hook slots + pins (29), `GameEntitySystem` 88 (CGameResourceService slot
   34 is `mov [rcx+0x58], rdx; ret`), `GameEventManager`, `AddListenerEntity`,
   `RemoveListenerEntity`.
6. **Live gate** on the NUCLEUS25 rig: `GAMEDATA VALIDATION` shows the new entries OK,
   `interface OK: GameEventManager`, the hello plugin prints `[hello] loaded`, `sm plugins list`
   lists it, and a bot spawn prints `[hello] spawn #N`.

## Checked offline (build 26798866)

Running `FromPe` + `GetVTableByName` + `Evaluate` over the real DLLs (mapped at their preferred
base) resolves CGameEventManager, CGameRulesGameSystem, CNavPhysicsInterface (server.dll) and
CServerSideClient, CNetworkGameServer (engine2.dll) to the same vftables an independent RTTI
walk finds; the three signatures each match once: GameEventManager instance RVA 0x1e062d0 (its
first qword is the CGameEventManager vftable), AddListenerEntity 0xbde0d0, RemoveListenerEntity
0xc145a0. One server.dll lookup takes about 0.3 s.

## Not in this slice

Engine calls through the declarative bridge (W3, Microsoft x64), detours (W4), the rest of the
win64 signatures and gdgen PE support (W5), Breakpad on Windows (W6).
