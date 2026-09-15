# KHook probe (test-only)

Native Metamod plugin used by suite A (and later B/C) of the KHook migration live gate.
**Never included in the production `s2script` release** (`scripts/package-addon.sh` /
`scripts/package-release.sh` do not copy it).

It is a second KHook consumer against the same Metamod pin. It hooks **controlled
native functions and dummy virtuals with valid objects** (no sentinel/dummy engine
pointers). After `PLUGIN_SAVEVARS` it may also `Add` engine virtuals (`GameFrame`,
`ClientCommand`, `DispatchConCommand`, `OnClientConnected`, `IGameEventManager2::FireEvent`
on the real manager) so those capsules are shared with `s2script`.

FireEvent observation uses `acceptance_observer.h`: a caller-owned invocation
scope surrounds the fixture `FireEvent` call. PRE/POST correlate by pointer
value and **never dereference a consumed `IGameEvent*`**. The engine listener may
read the event during its valid callback. `KHook::WasOriginalFunctionSkipped` is
the automatic original; engine delivery is the listener count.

## Build (sniper / SteamRT)

From the repo root, after the SDK/shim toolchain is available:

```bash
cmake -S tools/khook-probe -B build/khook-probe -DS2_SOURCE_DIR="$PWD" -DCMAKE_BUILD_TYPE=Release
cmake --build build/khook-probe -j
```

Requires `third_party/metamod-source` at the KHook pin (nested `third_party/khook`)
and `third_party/hl2sdk`. Reuses the shim’s include paths and
`_GLIBCXX_USE_CXX11_ABI=0` / `-m64` / `META_NO_HL2SDK` conventions.

Release builds keep interception: controlled targets are out-of-line
`noinline` functions; dummy virtuals are invoked through a runtime-selected
pointer so the compiler cannot devirtualize the call.

The post-build step stages (still not a production package):

- `build/khook-probe/stage/addons/s2script/bin/linuxsteamrt64/s2_khook_probe.so`
- `build/khook-probe/stage/addons/metamod/s2_khook_probe.vdf`

## Host observer (no CS2)

```bash
bash scripts/test-khook-observer.sh
```

ASan must catch `legacy_post_uaf` (POST `EventNameIs` after the original deletes
the event). The fixed observer must pass without touching consumed memory.

`--from-file` fixtures (negative controls + R6 pending) are generated from the
frozen R4 registry by `tools/khook-probe/testdata/gen_from_file.py`.

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

## Command protocol

```text
s2_khook_probe prepare <run_id>
s2_khook_probe collect <run_id>
s2_khook_probe report <run_id>
s2_khook_accept prepare <run_id>
s2_khook_accept collect <run_id>
s2_khook_accept report <run_id>
s2_khook_accept teardown <run_id>
```

Unknown or mismatched `run_id` values emit fail records (`unknown_or_mismatched_run`).
They never replay a prior run's passes. `report` is read-only.

Records are schema=1 JSON objects (`run_id`, `source_revision`, `case`,
`subcheck`, `producer`, `result`, typed `expected`/`actual`, `evidence`).

## Command route (F5)

Two independent names — do **not** require both on the same registered ConCommand.

### 1. Continue / handled original: `s2_khook_probe_token`

The probe registers this ConCommand. Its engine callback is the original-delivery
counter. The probe's `ClientCommand` and `DispatchConCommand` hooks return
**Ignore**. JS must **not** `command("s2_khook_probe_token")` — only
`command.onClientCommand`.

| token | JS `onClientCommand` | expected original |
| --- | --- | --- |
| `s2khook-continue` | Continue | js=1, native_pre=1, native_post=1, engine=1, skipped=false |
| `s2khook-handled` | Handled | js=1, native_pre=1, native_post=1, engine=0, skipped=true |

Registered ConCommands skip `ISource2GameClients::ClientCommand` and go through
`ICvar::DispatchConCommand` (`shim/src/s2script_mm.cpp`: `player_ping` /
`jointeam` / `drop` never see ClientCommand). The **validated original
boundary** is therefore `ICvar::DispatchConCommand` PRE+POST plus the probe
callback. A real client slot is required (`CCommandContext` slot >= 0). RCON
is a control operation and does not count toward continue/handled original.

Continue and handled are judged **independently**. Issuing only continue
leaves handled **pending**, not fail.

### 2. ClientCommand entry: `s2khook_cc_entry`

This name is **not** registered as a ConCommand. A real client typing it is
the ClientCommand-validated unrecognized command. Probe ClientCommand PRE/POST
count it as `clientcommand_entry`. **DispatchConCommand-only counters are never
labeled ClientCommand evidence.**

```text
# real client (not RCON):
s2khook_cc_entry
s2_khook_probe_token s2khook-continue
s2_khook_probe_token s2khook-handled
```

Control tokens (RCON allowed; cannot satisfy continue/handled original):

- `s2khook-ctrl-missing` — JS hook ignores; engine still runs
- `s2khook-ctrl-flip-continue` — JS Handled (Continue suppressed)
- `s2khook-ctrl-flip-handled` — JS Continue (Handled unsuppressed)

Omitted acceptance plugin: native looks for ConVar `s2_khook_accept_run`.
Omitted-plugin pass and JS delivery pass cannot both be true in one snapshot
(plugin present → omitted pending; plugin absent → JS delivery fail/missing).
R7 must run the omitted-plugin collect, then reset and load the plugin before
the real continue/handled collect. Do not last-write-wins a control pass over
the real run.

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
python3 scripts/rcon.py "s2_khook_probe prepare <run_id>"
python3 scripts/rcon.py "s2_khook_accept prepare <run_id>"
# real client (not RCON): s2khook_cc_entry
# real client (not RCON): s2_khook_probe_token s2khook-continue
# real client (not RCON): s2_khook_probe_token s2khook-handled
python3 scripts/rcon.py "s2_khook_probe collect <run_id>"
python3 scripts/rcon.py "s2_khook_accept collect <run_id>"
python3 scripts/rcon.py "s2_khook_probe report <run_id>"
python3 scripts/rcon.py "s2_khook_accept report <run_id>"
bash scripts/test-khook-live.sh A --prepare --run-dir build/khook-acceptance/run-001
bash scripts/test-khook-live.sh --self-test
```

RCON of `s2_khook_probe_token` is a control operation, not continue/handled original
and not ClientCommand evidence. Type `s2khook_cc_entry` from a connected client
for ClientCommand evidence.

## Unload

Probe `Unload` follows the checked-binding lifetime rules: refuse while a
callback (including the token ConCommand trampoline) is active; instance
`Remove` then `BeginRemove` on engine virtuals, dummy Virtuals
(`virtA`/`virtB`/`virtPre`/`virtPost`), and `fn*` once; retry until
`S2Hook_DrainRetirement() && RetirementPending()==0`. Binding objects stay
alive until Removed. Do not `S2Hook_SetLifecycle` from the probe (that flag
is process-global and would retire s2script).
