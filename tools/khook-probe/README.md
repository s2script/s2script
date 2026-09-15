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

Live collect order is **probe prepare → accept prepare → (client actions / wait frames) →
probe collect → accept collect**. `report` is read-only. Repeat `prepare` with the same
plugins loaded resets counters and removes owned `trigger_push` entities. Identities for
reuse/map/reload live in cvars (`s2_khook_accept_old_*`, `s2_khook_accept_map`,
`s2_khook_accept_instance`, `s2_khook_accept_unloaded`) plus probe statics — not in the
JS plugin heap.

Human observations use R4's file interface only:

```bash
bash scripts/test-khook-live.sh A --judge --run-dir build/khook-acceptance/run-001 \
  --observations build/khook-acceptance/run-001/human.json
```

Capture files belong under the run dir (`build/khook-acceptance/run-001/captures/`).
Schema/examples stay **pending**. Never invent a human pass. A missing actor list is
invalid (exit 1), not a pass.

Host `--from-file` fixtures (not live evidence) live under `tools/khook-probe/testdata/`
and are generated by `gen_from_file.py`: `r6-phase-transitions.jsonl` (exit 2, humans
pending), `r6-negative-filter.jsonl` / `r6-stale-post-map.jsonl` /
`r6-cleanup-reprepare.jsonl` / `r6-missing-actor.jsonl` (exit 1),
`r6-slot-reuse-pending.jsonl` / `r6-human-pending.jsonl` /
`r6-map-no-invoke.jsonl` / `r6-reload-no-delivery.jsonl` (exit 2). R5 negatives remain
exit 1.

## Twelve cases — commands, client actions, pending/retry

Controller:

```bash
bash scripts/test-khook-live.sh A --prepare --run-dir build/khook-acceptance/run-001
# client actions for the cases below
bash scripts/test-khook-live.sh A --collect --run-dir build/khook-acceptance/run-001
bash scripts/test-khook-live.sh A --judge --run-dir build/khook-acceptance/run-001 \
  --observations build/khook-acceptance/run-001/human.json
```

Or RCON the same prepare/collect/report names. Failed subchecks fail the case (exit 1).
Missing live/human evidence stays pending (exit 2). Retry: `prepare` again (explicit
cleanup) or keep the same `run_id` and `--collect` after the missing condition exists.
Do not reuse a prior run's JSON.

### 1. `new_capsule_registration` (R5 native)

No client. Probe collect invokes `TargetNew`. Pending only if Configure never activates.

### 2. `shared_capsule_registration` (R5 native)

No client. Two consumers on `TargetShare` plus shared `GameFrame`. Pending until a live
frame is observed.

### 3. `peer_actions_both_orders` (R5 native)

No client. Ignore/Override/Supersede matrix on `TargetAB`/`TargetBA`.

### 4. `one_normal_invocation` (R5 native)

No client. One PRE+POST+orig on `TargetOnce`.

### 5. `frame_client_command_hooks` (R5 native+JS)

```text
# real client, not RCON:
s2khook_cc_entry
s2_khook_probe_token s2khook-continue
s2_khook_probe_token s2khook-handled
```

Need a connected client for `js_client_connected` / identity join. Control tokens
(RCON allowed): `s2khook-ctrl-missing`, `s2khook-ctrl-flip-continue`,
`s2khook-ctrl-flip-handled`. Omitted-plugin collect: unload khook-acceptance, collect,
then load it again before the real continue/handled collect.

### 6. `fire_event_no_suppression` (R5 native+JS)

No client. Probe collect fires `player_activate` inside a caller-owned
`FireEventInvocationScope`. JS must not Handled-hook that name. Distinct from
Handled+CallOriginal+Supersede (case 12).

### 7. `sdkhooks_one_of_two_entities` (R6 native+JS)

No extra client. Prepare independently spawns two `trigger_push` entities (A hooked,
B not). Probe GameFrame invokes `CTriggerPush::Touch` **once on A and once on B**
through the **SDKHooks adapter** (not a 48-frame hammer; Dummy Virtuals are supporting
evidence only). Expect A=1, B=0. Both spawns must succeed before judging. B delivered
= filter fail. Neither delivered = registration fail. Pending if CreateEntity /
DispatchSpawn / Touch slot unresolved.

### 8. `sdkhooks_phase_removal` (R6 native+JS)

No extra client. Same Touch adapter; PRE and POST are separate KHook Virtuals.
Drive with **one invoke per stage** (self-unsub uses two frames). Exact counts after
each step: subscribe PRE+POST → PRE=1 POST=1 original=1; remove PRE → 0/1/1;
restore PRE, remove POST → 1/0/1; restore both, PRE self-unsub → first both, second
POST only; remove final → 0/0/1. Native `original` is the vtable witness. JS expected/
actual are PRE/POST only (do not fabricate `original: 1`). A no-op `logic_relay` is
not this test. Pending with the missing live adapter condition until those snapshots
exist.

### 9. `entity_slot_reuse_map_teardown` (R6 native+JS)

```text
# persist happens at prepare (cvars + probe statics). Then:
python3 scripts/rcon.py "changelevel de_dust2"   # same run_id
# one post-map Touch attempt via EntByIndex, only if the occupant is still
# trigger_push (or vt[slot] is the resolved Touch). Wrong-class occupants are
# not invoked. If no live trigger remains, *_map_teardown_clears stays pending.
# JS unload/reload with probe still loaded (R2 pending/retry):
python3 scripts/rcon.py "s2script_reload @example/khook-acceptance"
# probe invokes the new entity A once. Collect requires that delivery, not mere SDKHook
# registration. Missing restoration must stay visible. Keep s2_khook_probe loaded.
```

Slot reuse is bounded (64 attempts). If the slot is not reused with a new serial,
that subcheck stays **pending** — not a false pass. A cached pre-map counter reused
after map teardown is a **fail**. Zero post-map callbacks with no trigger_push
EntByIndex invoke is **pending** (wrong-class occupant is not an invoke), not a pass.

### 10. `voice_recall` (R6 native+JS+human)

Need **three** real clients. Prepare prints `NEED_CLIENTS:` with speaker / allowed /
denied slots. JS applies `Voice.setAudibleTo(speaker, [allowed])`. Native observes
`SetClientListening` bits + original once.

| phase | client action | human subcheck | capture |
| --- | --- | --- | --- |
| allowed | speaker talks; allowed hears | `human_voice_allowed_hears` | `captures/voice-allowed.txt` |
| denied | speaker talks; denied silent | `human_voice_denied_silent` | `captures/voice-denied.txt` |
| unmuted | `s2_khook_accept teardown` (Voice.reset) then speaker talks; formerly denied hears | `human_voice_unmuted_hears` | `captures/voice-unmuted.txt` |

Pending without three clients or without `--observations`. Do not fill a pass from logs.

### 11. `check_transmit` (R6 native+JS+human)

Need **two** real clients in PVS of a networked visible entity (`point_worldtext`
`S2KHOOK TRANSMIT`, not `logic_relay`). JS `Transmit.setVisibleTo` A-only + SetTransmit
deny B, writing dedicated `s2_khook_accept_tx_a` / `tx_b` (not mask slots). Native
`native_first_fire_layout` is the first CheckTransmit fire that inspects that entity's
bitvec — not any CheckTransmit with an int32 at offset 576.

| phase | client action | human subcheck | capture |
| --- | --- | --- | --- |
| A visible | both stand in PVS; A sees the green text | `human_client_a_visible` | `captures/tx-a.txt` |
| B denied | B does not see it | `human_client_b_denied` | `captures/tx-b.txt` |
| restored | `s2_khook_accept teardown` then both see it | `human_visibility_restored` | `captures/tx-restored.txt` |

### 12. `fire_event_handled_recipient_mask` (R6 native+JS+human)

Need **two** real clients. Probe GameFrame fires `player_changename` in a **separate**
`FireEventInvocationScope` token `fire_event_handled_recipient_mask` (not the R5
`player_activate` no-suppression scope). JS `hook.onPre` returns Handled +
`Events.setRecipients([slotA])`, then empty recipients (all-suppressed). Native:
`orig:1, automatic_skipped:true, listener:1` (CallOriginal+Supersede, not zero engine
calls) and PostEventAbstract outgoing bits. Sending to every human is not a mask test.

| phase | client action | human subcheck | capture |
| --- | --- | --- | --- |
| subset | watch client console for `player_changename`; A receives, B does not | `human_subset_receipt` / `human_excluded_nonreceipt` | `captures/mask-subset.txt` |
| all-suppressed | second fire; neither client receives | `human_all_suppressed` | `captures/mask-all.txt` |

Collect twice if the first collect only saw the subset (JS then switches mask mode to
`all`). Posts may flush later in the frame — wait one GameFrame before judging.

## Failed / pending examples

- Filtering failure: B delivered while only A was hooked → `js_hook_b_filtered` fail.
- Registration failure: neither A nor B delivered → both JS filter subchecks fail.
- Stale post-map record: pre-map count copied after changelevel →
  `native_map_teardown_clears` fail.
- Map ended but no remaining `trigger_push` to invoke via EntByIndex →
  `*_map_teardown_clears` pending.
- Reload registered SDKHook but it did not deliver once →
  `js_fresh_subscription_after_reload` pending.
- Repeat prepare leaked counters (A=2) → `native_spawn_a_ok` fail.
- Slot reuse not achieved in 64 attempts → `native_*slot_reuse*` pending, not pass.
- Missing human actors with `result=pass` → judge invalid (exit 1).
- `report` before `collect` → pending envelopes (`report before collect`).

## Unload

Probe `Unload` follows the checked-binding lifetime rules: refuse while a
callback (including the token ConCommand trampoline) is active; instance
`Remove` then `BeginRemove` on engine virtuals, dummy Virtuals
(`virtA`/`virtB`/`virtPre`/`virtPost`), and `fn*` once; retry until
`S2Hook_DrainRetirement() && RetirementPending()==0`. Binding objects stay
alive until Removed. Do not `S2Hook_SetLifecycle` from the probe (that flag
is process-global and would retire s2script).
