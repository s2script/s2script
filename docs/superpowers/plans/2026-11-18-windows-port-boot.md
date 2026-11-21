# Windows port W1 (build + boot) — plan

Spec: [2026-11-18-windows-port-design.md](../specs/2026-11-18-windows-port-design.md).

## Tasks

1. **Core builds for `x86_64-pc-windows-msvc`.** The native-extension loader's `dlopen` family moves
   behind a `cfg(unix)` module; on Windows it reports "not supported" (native extensions are a later
   slice). The gamedata platform key becomes `crate::GAMEDATA_PLATFORM` (`win64` on Windows). The
   CRT is linked statically (`.cargo/config.toml`). Done.
2. **Platform seam.** `shim/src/platform.h` with `platform_posix.cpp` and `platform_win.cpp`. Every
   Linux call site in the shim (module walk, own path, exports, readable probe, executable ranges,
   thread id, directories, `lib*.so` file names) moves onto it with unchanged behaviour. Done.
3. **Windows stubs with named reasons.** Plugin function overrides and native crash capture refuse
   with "not yet ported to Windows"; the engine-call ABI, engine-function bridge, engine resolver
   and vtable-by-name already refuse off Linux. Done.
4. **MSVC ABI facts the shim needs now.** The PMF-to-vtable-slot decoder reads MSVC vcall thunks.
   Mangled tier0 export names for addons. Done.
5. **Shim CMake for Windows.** `shim/cmake/Windows.cmake`, the clang-cl toolchain file, libffi
   without autotools (`LibffiWin.cmake`), the memoverride replacement and the link-time leaves.
   Done.
6. **Delay-loaded core.** `/DELAYLOAD:s2script_core.dll` plus `PreloadSibling` at the top of
   `Load()`. Done.
7. **Build script, packaging, CI.** `scripts/build-windows.sh` (with import checks),
   `S2_PLATFORM=win64` in `package-addon.sh`, and the `windows` ci-native shard. Done.
8. **Live gate.** On a CS2 Windows dedicated server with Metamod: s2script loads, V8 boots, a hello
   plugin prints from `OnPluginStart`, `OnGameFrame` ticks, `sm plugins list` answers. Gamedata
   descriptors without a `win64` entry are expected to log as degraded.
   **Run 2026-11-21** (CS2 1.42.0.0 / ServerVersion 2000948, Metamod 2.0.0-dev+1491, NUCLEUS25):
   Metamod loads s2script, V8 starts, `sm` answers, the server ticks for 5.5 minutes with no crash,
   and every interface resolves. Plugins do not load: the GameFrame hook needs its `win64` vtable
   slot (`ISource2Server_GameFrame`) and gamedata has no `win64` entries yet (`0 ok, 72 FAILED`), so
   that half of the gate moves to W2. The run also showed the optional core entries never binding on
   Windows (a weak PE import stays null), fixed by importing them normally there.

## Acceptance

- `scripts/ci-native.sh windows` passes on the Linux CI runner.
- The Linux shards are unchanged (`make ci-native`).
- Live gate in task 8.
