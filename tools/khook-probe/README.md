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

Chosen command: **`s2_khook_probe_token`** with tokens:

| token | JS `onClientCommand` | expected |
| --- | --- | --- |
| `s2khook-continue` | Continue | js=1, native_pre=1, native_post=1, engine=1, skipped=false |
| `s2khook-handled` | Handled | js=1, native_pre=1, native_post=1, engine=0, skipped=true |

The probe registers that ConCommand; its engine callback is the original-delivery
counter. The probe's `ClientCommand` and `DispatchConCommand` hooks return
**Ignore** (they must not Supersede on s2script's behalf). JS must **not**
`command("s2_khook_probe_token")` — only `command.onClientCommand`.

**Verified from engine comments (live ClientCommand traversal is still pending):**
s2script's `DispatchConCommand` hook is the `onClientCommand` seam because
**registered ConCommands (including `player_ping` / `jointeam` / `drop`) do not
go through `ISource2GameClients::ClientCommand`**, which is a fallback for
unrecognised names. RCON and `fakeCommand` are DispatchConCommand-only.

Therefore the **validated original boundary** for this ConCommand is
`ICvar::DispatchConCommand`, observed with a test-only KHook PRE+POST on that
virtual. `WasOriginalFunctionSkipped` on that POST is the continue/handled
signal. **DispatchConCommand-only counters are not ClientCommand evidence.**

Pass for `native_command_continue_original` / `native_command_handled_skipped`
requires **both**:

1. real-client `ClientCommand` PRE/POST for that token (`clientcommand_entry=true`)
2. DispatchConCommand original counts matching the table above

If only DispatchConCommand fired (RCON, or ConCommand bypass with no
ClientCommand), those subchecks stay **pending** with the missing condition:
a connected client must type `s2_khook_probe_token s2khook-continue` then
`s2_khook_probe_token s2khook-handled`. Do not use probe votes to manufacture
that outcome.

Control tokens (cannot satisfy the real continue/handled subchecks):

- `s2khook-ctrl-missing` — JS hook ignores; engine still runs
- `s2khook-ctrl-flip-continue` — JS Handled (Continue suppressed)
- `s2khook-ctrl-flip-handled` — JS Continue (Handled unsuppressed)

Omitted acceptance plugin: native looks for ConVar `s2_khook_accept_run`.

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
# real client (not RCON): s2_khook_probe_token s2khook-continue
# real client (not RCON): s2_khook_probe_token s2khook-handled
python3 scripts/rcon.py "s2_khook_probe collect <run_id>"
python3 scripts/rcon.py "s2_khook_accept collect <run_id>"
python3 scripts/rcon.py "s2_khook_probe report <run_id>"
python3 scripts/rcon.py "s2_khook_accept report <run_id>"
bash scripts/test-khook-live.sh A --prepare --run-dir build/khook-acceptance/run-001
bash scripts/test-khook-live.sh --self-test
```

RCON of `s2_khook_probe_token` is a control operation, not ClientCommand evidence.

## Unload

Probe `Unload` follows the checked-binding lifetime rules: refuse while a
callback (including the token ConCommand trampoline) is active; `BeginRemove`
once; retry until `S2Hook_DrainRetirement() && RetirementPending()==0`. Binding
objects stay alive until Removed. Do not `S2Hook_SetLifecycle` from the probe
(that flag is process-global and would retire s2script).
