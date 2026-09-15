# KHook probe (test-only)

Native Metamod plugin used by suite A (and later B/C) of the KHook migration live gate.
**Never included in the production `s2script` release** (`scripts/package-addon.sh` /
`scripts/package-release.sh` do not copy it).

It is a second KHook consumer against the same unmodified Metamod/KHook pin.
The native `s2script` shim and probe remain loaded throughout this gate. Only the
acceptance `.s2sp` hot reload is tested; native binary updates require a server
restart and a fresh run. It hooks **controlled
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
node --test tools/khook-probe/testdata/fixture.test.mjs
python3 tools/khook-probe/testdata/native_fixture_test.py
python3 tools/khook-probe/testdata/script_reload_test.py
```

ASan must catch `legacy_post_uaf` (POST `EventNameIs` after the original deletes
the event). The fixed observer must pass without touching consumed memory.

`--from-file` fixtures (negative controls + R6 pending) are generated from the
frozen registry by `tools/khook-probe/testdata/gen_from_file.py`. These are synthetic parser inputs, not engine evidence. The JS VM harness executes the actual TypeScript fixture against the unavailable engine boundary; the native harnesses compile the actual phase-driver, command-verdict and script-reload driver functions. Reload negatives cover stale old callbacks, duplicate/missing replacement callbacks, suppressed originals, unchanged generations and an old marker that remains alive. The shared C++ observer tests original suppression, duplicate calls, missing POST, and effective voice bits with both callback orders.

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
s2_khook_probe prepare <run_id> [artifact_sha256]
s2_khook_accept prepare <run_id> [artifact_sha256]
s2_khook_probe bind <run_id> <artifact_sha256>
s2_khook_accept bind <run_id> <artifact_sha256>
s2_khook_probe collect <run_id>
s2_khook_accept collect <run_id>
s2_khook_probe report <run_id>
s2_khook_accept report <run_id>
s2_khook_accept restore <run_id>
s2_khook_accept teardown <run_id>
s2_khook_probe reload-arm <run_id>
s2_khook_accept reload-arm <run_id>
s2_khook_accept resume <run_id> <artifact_sha256>
s2_khook_probe runtime
```

Run IDs must be 1–64 ASCII letters/digits, underscores or hyphens, beginning with
a letter/digit. The controller rejects unsafe IDs before creating files or sending
commands. Prepare resets owned state once. Bind attaches the independently verified runtime
receipt digest without resetting observations; a changed binding is rejected.
Unbound passing observations are emitted as pending. Records include frozen
`source_revision` and `artifact_identity`; reporting does not reread a mutable
identity cvar. Unknown/mismatched collect/report runs emit failed records.
Restore/teardown reject a mismatched run. Report is read-only.

Restore removes voice and transmit policy while preserving the visible entity for
client capture. Teardown deletes owned entities after those captures. Complete
human phases before the map transition or script reload destroys their subjects.

Resume restores the same run and immutable artifact digest from an actual `.s2sp`
`previous()` state handoff. It restores completed terminal records and subscribes
the replacement only to the dedicated resident reload target; it does not replay
filter/phase stages. A fresh script load without a handoff cannot resume. The
native probe keeps its existing run and stage state throughout; no native unload
or resume is required. A lost native process requires a fresh run. JS presence
uses `s2_khook_accept_live` (generation while loaded, zero in OnPluginEnd), not the
existence of a persistent cvar.

`runtime` emits a separate read-only `kind:"khook-runtime"` object with the compiled
source revision, process ID, monotonic load-generation token, actual engine map
and build, and canonical loaded paths in `modules:{probe,shim,core}`. Missing or
ambiguous modules have null paths and result `pending`; all witnesses available
produces `ready`. It does not manufacture file hashes or an installation receipt.

## Command route (F5)

The controlled target is `s2khook_cc_entry`, deliberately **unregistered** as a
ConCommand. A real connected client issues these commands exactly once:

```text
s2khook_cc_entry <run_id> s2khook-continue
s2khook_cc_entry <run_id> s2khook-handled
s2khook_cc_entry <run_id> s2khook-ctrl-missing
s2khook_cc_entry <run_id> s2khook-ctrl-flip-continue
s2khook_cc_entry <run_id> s2khook-ctrl-flip-handled
```

Every command must carry the prepared run ID. RCON and other runs cannot satisfy
these checks. Reconnect a real client after prepare so both native and JS
connection observations bind that client to the run.

The probe's ClientCommand virtual PRE/POST return Ignore and count the same token.
An independent Function consumer of the validated ClientCommand implementation
counts its original execution. JS supplies Continue/Handled through
`command.onClientCommand`; the shim forwards the actual ClientCommand listener
seam as well as the existing DispatchConCommand seam. These are different engine
routes, and DispatchConCommand counters never satisfy this case.

| token | JS callbacks | native PRE/POST | engine original | automatic skipped |
| --- | --- | --- | --- | --- |
| continue | 1 | 1 / 1 | 1 | false |
| handled | 1 | 1 / 1 | 0 | true |

This controlled name reaches the engine's unknown-command handler. The accepted
scope of the test is real ClientCommand entry and actual original/suppression;
it does not claim a gameplay effect. To exercise an available game command that
uses ClientCommand, set `s2_khook_accept_command` before loading the JS fixture,
then use that command name with the same run/token arguments. The Function
witness will reject a route that bypasses ClientCommand.

Control tokens deliberately omit the JS decision or invert it. Run the
omitted-plugin control **first**, with the acceptance `.s2sp` absent:

```text
sm plugins unload @example/khook-acceptance
# Controller prepare binds the native run; absent JS cannot answer yet.
# Wait a native frame, then controller collect saves the omitted-plugin control.
sm plugins load @example/khook-acceptance
s2_khook_accept prepare <run_id> <artifact_sha256>
```

Only prepare the newly loaded JS fixture at this point. Keep the already prepared
native run and collected controller records. Then reconnect the real clients and
perform the normal cases. Do not use resume here: this is the first script
preparation, and the final reload handoff has not happened. Terminal observations
are latched; later pending reports cannot erase a completed control. The
controller separately rejects absent JS delivery in continue/handled checks.

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
python3 scripts/rcon.py "s2_khook_probe prepare <run_id> <artifact_sha256>"
python3 scripts/rcon.py "s2_khook_accept prepare <run_id> <artifact_sha256>"
# real client (not RCON): s2khook_cc_entry <run_id> s2khook-continue
# real client (not RCON): s2khook_cc_entry <run_id> s2khook-handled
python3 scripts/rcon.py "s2_khook_probe collect <run_id>"
python3 scripts/rcon.py "s2_khook_accept collect <run_id>"
python3 scripts/rcon.py "s2_khook_probe report <run_id>"
python3 scripts/rcon.py "s2_khook_accept report <run_id>"
bash scripts/test-khook-live.sh A --prepare --run-dir build/khook-acceptance/run-001
bash scripts/test-khook-live.sh --self-test
```

RCON controls prepare/collect/report. Only a real connected client may send the command-test tokens.

After the initial omitted-plugin control, live collect order is **accept prepare →
(client actions / wait frames) → probe collect → accept collect**. `report` is read-only. Repeat `prepare` with the same
plugins loaded resets counters and removes owned `trigger_push` entities. Reuse/map identities live in cvars and resident probe state. Reload also requires
the real JS state handoff and native generation-tagged before/after observations.
The receipt keeps its original map identity even when the current map changes.

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
s2khook_cc_entry <run_id> s2khook-continue
s2khook_cc_entry <run_id> s2khook-handled
```

Need a connected client for `js_client_connected` / identity join. Control tokens
(real client only): `s2khook-ctrl-missing`, `s2khook-ctrl-flip-continue`,
`s2khook-ctrl-flip-handled`. Complete the omitted-plugin control first using the
Command route sequence above, before preparing the JS fixture.

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
Drive with **one invoke per acknowledged stage** (self-unsub uses two stages). Exact counts after
each step: subscribe PRE+POST → PRE=1 POST=1 original=1; remove PRE → 0/1/1;
restore PRE, remove POST → 1/0/1; restore both, PRE self-unsub → first both, second
POST only; remove final → 0/0/1. Native `original` comes from an independent Function witness on the same JS entity. JS expected/actual are exact PRE/POST counts, including the first POST during self-unsubscribe. Native acknowledges each completed invocation with run ID, entity index and stage; stage 5 executes after final unsubscribe. Zero callbacks without that acknowledgement stay pending. A no-op `logic_relay` is
not this test. Pending with the missing live adapter condition until those snapshots
exist.

### 9. `entity_slot_reuse_map_teardown` (R6 native+JS)

Complete the human phases, filter and SDKHooks phase stages before the map
transition. Collect each completed stage so terminal observations are retained.
Then change level under the same run and collect the map checks. A post-map Touch
uses EntByIndex only when the occupant remains a validated `trigger_push`; a
wrong-class occupant is never invoked.

```bash
python3 scripts/rcon.py "changelevel de_dust2"
bash scripts/test-khook-live.sh A --collect --run-dir build/khook-acceptance/run-001
```

Slot reuse is bounded (64 attempts). If the slot is not reused with a new serial,
that subcheck stays **pending**. A cached pre-map counter reused after map teardown
fails. Zero post-map callbacks without a live EntByIndex invocation stay pending.

**Script reload is the final stage.** Before arming it, inspect the collected run
and require every other native, JS and human subcheck to pass. Only
`script_hot_reload` and `js_fresh_subscription_after_reload` may remain pending.
Pending old stages cannot become completed merely by restoring the handoff.
Do not issue prepare again or change maps after arming the target.

```bash
python3 scripts/rcon.py "s2_khook_probe reload-arm <run_id>"
python3 scripts/rcon.py "s2_khook_accept reload-arm <run_id>"
# Wait frames; inspect the before-ack and trace before reloading:
python3 scripts/rcon.py "s2_khook_accept_reload_ack"
python3 scripts/rcon.py "s2_khook_accept_reload_trace"
# ack must be stage=before, same run/digest, original=1;
# trace must be exactly <old_generation>:pre,<old_generation>:post,
python3 scripts/rcon.py "sm plugins reload @example/khook-acceptance"
python3 scripts/rcon.py "s2_khook_accept resume <run_id> <artifact_sha256>"
# Wait at least three game frames for owner cleanup and the after invocation.
bash scripts/test-khook-live.sh A --collect --run-dir build/khook-acceptance/run-001
```

The native probe creates a dedicated `trigger_push` after the final map transition
and retains its index/serial across the `.s2sp` reload. Both generations subscribe
PRE+POST to that same target. The outgoing script deliberately leaves these
subscriptions to owner-ledger teardown. No outgoing-generation callback may fire
on the after invocation. Its marker is a world-owned `createEntity` entity,
explicitly removed by the fixture's `OnPluginEnd` cleanup; marker disappearance
checks that cleanup, not automatic entity-ledger disposal. A surviving marker
fails the native reload record even with correct replacement callbacks. The replacement
must deliver PRE=1 and POST=1; an independent Function hook must observe original=1
both before and after. Native target identity and the original run/digest must
remain unchanged. Merely registering a subscription, changing a counter, or
restarting the native probe cannot satisfy this proof.

`script_hot_reload` records exact before/after counts, old callback count zero,
generation change and old-resource removal. Missing before/after observation is
pending; an observed missing/duplicate/stale callback, missing original, unchanged
generation or retained marker fails. Repeated collects replay the same terminal
observations. Native unload/reload is outside this acceptance workflow.

### 10. `voice_recall` (R6 native+JS+human)

Need **three** real clients. Prepare prints `NEED_CLIENTS:` with speaker / allowed /
denied slots. JS applies `Voice.setAudibleTo(speaker, [allowed])`. Native observes
`SetClientListening` effective arguments at the original Function boundary, the stored GetClientListening value, and one original execution per request. These are checked for allowed, denied and restored requests; incoming virtual PRE arguments are not the effective-bit witness.

| phase | client action | human subcheck | capture |
| --- | --- | --- | --- |
| allowed | speaker talks; allowed hears | `human_voice_allowed_hears` | `captures/voice-allowed.txt` |
| denied | speaker talks; denied silent | `human_voice_denied_silent` | `captures/voice-denied.txt` |
| unmuted | `s2_khook_accept restore <run_id>` (Voice.reset) then speaker talks; formerly denied hears | `human_voice_unmuted_hears` | `captures/voice-unmuted.txt` |

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
| restored | `s2_khook_accept restore <run_id>` then both see the same entity | `human_visibility_restored` | `captures/tx-restored.txt` |

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
- Reload registered SDKHook but no native invocation has occurred →
  `js_fresh_subscription_after_reload` pending.
- Native after-invocation occurred but a callback/original is missing, doubled or
  from the outgoing generation → `script_hot_reload` fail.
- Repeat prepare leaked counters (A=2) → `native_spawn_a_ok` fail.
- Slot reuse not achieved in 64 attempts → `native_*slot_reuse*` pending, not pass.
- Missing human actors with `result=pass` → judge invalid (exit 1).
- `report` before `collect` → pending envelopes (`report before collect`).

## Native residency

Leave `s2script` and `s2_khook_probe` loaded throughout the twelve cases. Their
native hot unload/reload is not an acceptance requirement. Update native binaries
by stopping the server, replacing verified artifacts, restarting and preparing a
fresh run. The local fixture tests do not establish live CS2 behavior; Linux build,
real engine delivery, human observations and process shutdown evidence remain
separate gates.
