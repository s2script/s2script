# KHook probe (test-only)

Native Metamod plugin used by suite A (and later B/C) of the KHook migration live gate.
**Never included in the production `s2script` release** (`scripts/package-addon.sh` /
`scripts/package-release.sh` do not copy it).

It is a second KHook consumer against the same Metamod pin. It hooks **controlled
native functions and dummy virtuals with valid objects** (no sentinel/dummy engine
pointers). After `PLUGIN_SAVEVARS` it may also `Add` engine virtuals (`GameFrame`,
`ClientCommand`, `OnClientConnected`, `IGameEventManager2::FireEvent` on the real
manager) so those capsules are shared with `s2script`.

## Build (sniper / SteamRT)

From the repo root, after the SDK/shim toolchain is available:

```bash
cmake -S tools/khook-probe -B build/khook-probe -DS2_SOURCE_DIR="$PWD"
cmake --build build/khook-probe -j
```

Requires `third_party/metamod-source` at the KHook pin (nested `third_party/khook`)
and `third_party/hl2sdk`. Reuses the shim’s include paths and
`_GLIBCXX_USE_CXX11_ABI=0` / `-m64` / `META_NO_HL2SDK` conventions.

The post-build step stages (still not a production package):

- `build/khook-probe/stage/addons/s2script/bin/linuxsteamrt64/s2_khook_probe.so`
- `build/khook-probe/stage/addons/metamod/s2_khook_probe.vdf`

## Install on the live CS2 gate

Copy the staged `.so` next to `s2script.so` and the VDF into the Metamod plugins
directory (same tree as `s2script.vdf`):

```bash
cp build/khook-probe/stage/addons/s2script/bin/linuxsteamrt64/s2_khook_probe.so \
   dist/addons/s2script/bin/linuxsteamrt64/
cp build/khook-probe/stage/addons/metamod/s2_khook_probe.vdf \
   docker/metamod/
```

Restart with `docker compose -f docker/docker-compose.yml restart cs2` (not
`--force-recreate`). `meta list` should show `s2_khook_probe` alongside `s2script`.

## Command

```text
s2_khook_probe run A
```

Prints **one JSON object per suite A case** (`case`, `expected`, `actual`, `result`
=`pass`|`fail`|`pending`). Controlled-function cases can pass on a loaded probe
without extra clients. `fire_event_no_suppression` fires `player_activate` once with
Continue and counts PRE vs original (JS Handled uses `player_changename`).
`frame_client_command_hooks` stays pending until live frames, a client connect, and
two `khook_probe_ping` ClientCommands (Ignore then Supercede). Voice, recipient-mask,
CheckTransmit, live SDKHooks, and map/teardown stay `pending` until humans / live
SDKHook dispatch. `s2_khook_accept report` is read-only; use `teardown` to unhook.

## JS fixture

```bash
node packages/sdk/dist/cli.js build examples/khook-acceptance
```

(`cd examples/khook-acceptance && node ../../packages/sdk/dist/cli.js build` from
inside this repo walks up to the s2script workspace root and builds `plugins/*`
instead. Pass the example path from the repo root.)

Drop `examples/khook-acceptance/dist/*.s2sp` into `dist/addons/s2script/plugins/`.
It is an example fixture, not a base plugin, and is not in the runtime zip.

## Drive

```bash
python3 scripts/rcon.py "s2_khook_accept prepare"
python3 scripts/rcon.py "khook_probe_ping"   # twice: continuation then suppression
python3 scripts/rcon.py "khook_probe_ping"
python3 scripts/rcon.py "s2_khook_probe run A"
python3 scripts/rcon.py "s2_khook_accept report"   # read-only
bash scripts/test-khook-live.sh A
bash scripts/test-khook-live.sh --self-test   # host parser; no CS2
```

RCON `khook_probe_ping` may only hit `DispatchConCommand`. Native `ClientCommand`
suppression/continuation still needs a real (or bot) client issuing the command.
