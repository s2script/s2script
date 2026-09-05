# Spectator delivery experiment (test-only)

**Prepared, not run. No delivery or confidentiality result has been measured.**
This plugin is an opt-in experiment, excluded from shipped base plugins. It creates
at most two independent `custom_hud_layout` entities using the existing published
`s2script_lib.xml`. A uses the top-left badge, B the top-right. Each starts hidden
from every recipient. Every operation is server-console/RCON-only and explicit.
It does not change teams, maps, client settings, workshop items, or public HUD APIs.

The question is whether recipient filtering actually stops the spectating client
receiving the target's entity, and whether the owner still sees its own entity
when viewing another player's slot. Per-slot state and per-recipient delivery are
separate variables in this probe. `paint … all` writes identical **synthetic** state
into all 64 slots of one entity; it does not broaden its recipient filter.

The small test-local `globalThis.__s2pkg_cs2_calls` bridge calls the same bound,
status-gated engine descriptors as `games/cs2/js/ui.js`. It exists only to bypass
the public library's one-entity-per-resource cache. No new native binding, raw
pointer, schema offset, or public privacy interface is introduced. This bridge is
an internal implementation detail and can break across runtime versions.

## Prerequisites and isolation

Use a disposable CS2 test session with two independently observable, authenticated
human clients. Record both screens, including observer mode and target. Bots cannot
establish a receiving client's rendering. No client automation is assumed.

Both clients must have workshop addon **3790153369** mounted through the existing
MultiAddonManager setup. Do not republish it for this experiment. First prove the
baseline badge actually loads on both clients; otherwise all negative results are
inconclusive. Use a build whose existing HUD descriptor statuses are `available`.

Do not run alongside TTT/MAUL or other HUD/transmit plugins: these can overwrite
panels, combine visibility rules, and contaminate global transmit counters. Arrange
an isolated addon/plugin directory and session first. Do not disable plugins on a
shared server just to satisfy this prerequisite.

Read-only preflight on 2026-09-04 found SSH/RCON access to
`ghirakawa@nebula.gkh.dev`, container `s2script-hudlab`, port **27019**. It reported
`ttt_skate`, zero human clients and zero bots, with MultiAddonManager and s2script
loaded. Its addon directory contains TTT/MAUL packages. This is **not evidence of
an isolated test server**. No files were deployed and no server writes/restarts
were performed. Its runtime commit is unknown (the remote directory has no `.git`).

## Build and stage (future, after arranging the isolated session)

From the repository root, after the normal npm dependency install and SDK build:

```sh
node packages/sdk/dist/cli.js build examples/hud-privacy-probe --packages-dir packages
node --test examples/hud-privacy-probe/test/probe.test.mjs
```

The strict typecheck and lint gate produces
`examples/hud-privacy-probe/dist/_demo_hud-privacy-probe.s2sp`. Stage that archive in
the **approved isolated** runtime's `addons/s2script/plugins/` directory using its
normal plugin deployment procedure. Do not copy it into the existing nebula server
as part of preparation. Confirm `sm plugins list` lists `@demo/hud-privacy-probe`
and `s2_hudprivacy status` reports both calls available before continuing. Record
CS2 version, runtime binary hashes/version, probe commit, map, addon revision and
plugin list. A successful build is not a successful engine test.

The following are server-console commands. On an approved nebula session they can
be sent using `python3 ~/s2script/scripts/rcon.py --port 27019 "COMMAND"` over SSH;
that helper uses the existing local RCON configuration. Do not print credentials.
Replace `0` and `1` below with the slots reported by `s2_hudprivacy status`; server
user IDs and zero-based slots are different. Recheck slots after every reconnect.

## Two-client protocol

Use fresh markers at every step; never use actual roles, balances or secrets.
Take status snapshots before and after each stage, wait several seconds and record
both clients. Record first-person and chase-camera observation separately. If either
baseline fails, stop and fix the setup before interpreting filtered negatives.

1. **Shared baseline.** A is active (slot 0), B (slot 1) spectates A. Run:

   ```text
   s2_hudprivacy clean
   s2_hudprivacy create A
   s2_hudprivacy paint A 0 BASELINE_1
   s2_hudprivacy audience A all
   s2_hudprivacy status
   ```

   A must render `BASELINE_1`; record whether B does. This reproduces the known
   spectator behavior on this exact build. Also test B alive/viewing itself with
   `paint A all BASELINE_ALL`: both must render it. Return B to spectating A.

2. **Warm strip and reversal.** With B still watching A:

   ```text
   s2_hudprivacy audience A 0
   s2_hudprivacy paint A 0 FILTERED_2
   s2_hudprivacy status
   s2_hudprivacy audience A none
   s2_hudprivacy paint A 0 NOBODY_3
   s2_hudprivacy status
   s2_hudprivacy audience A all
   s2_hudprivacy paint A 0 RESTORED_4
   s2_hudprivacy status
   ```

   Pause/record between each command pair. Under owner-only filtering A should
   update to `FILTERED_2`, while B should not receive that new marker. Under
   nobody filtering neither should receive `NOBODY_3`. Reset must restore new
   updates on both. Record any stale cached panel separately from fresh updates:
   filtering cannot erase information a client already received. Repeat
   owner-only → all at least twice with fresh markers to rule out timing errors.

3. **Cold recipient.** Clean, reconnect both clients, check new slot mapping, then
   create A (starts audience=none), paint A's slot with `COLD_5`, and allow only A.
   Confirm A renders and B does not. Reconnect B while the owner-only rule stays
   active, then spectate A and update to `COLD_6`. Repeat with B connected before
   entity creation and with B connecting afterward. This checks first receipt,
   sign-on baselines and the spawn/filter ordering, not just a warmed cache.

4. **Owner spectating someone else.** Let B play and A spectate B; keep A's
   entity filtered to A. Start with fresh entity/state to avoid previous `all`
   writes contaminating this control:

   ```text
   s2_hudprivacy clean
   s2_hudprivacy create A
   s2_hudprivacy audience A 0
   s2_hudprivacy paint A 0 OWNER_SLOT_7
   s2_hudprivacy status
   s2_hudprivacy paint A 1 VIEWED_SLOT_8
   s2_hudprivacy status
   s2_hudprivacy paint A all MIRRORED_9
   s2_hudprivacy status
   ```

   Record what A sees at each stage and whether B sees anything. If A only
   renders `VIEWED_SLOT_8` or `MIRRORED_9`, recipient filtering alone does not
   solve owner rendering: the entity needs an explicit viewed-slot state policy.
   The all-slot write is a diagnostic, not a proposed production implementation.

5. **Two independent recipient entities.** Clean; create A and B; paint A all
   `ONLY_A_10`, B all `ONLY_B_11`; set A audience=0, B audience=1. Confirm each
   client sees only its own corner/marker while alive and while spectating the
   other. Swap recipients (A audience=1, B audience=0) and write fresh markers;
   then restore. Repeat with the participant roles reversed. If only one layout
   instance renders, log duplicate-resource behavior as a separate failure.

6. **Lifecycle.** With synthetic markers only, exercise observer target changes,
   reconnect/slot reuse and a planned isolated-session map transition. Rules are
   numeric-slot rules; this harness deliberately does not claim session identity
   safety. Clean before a different person reuses a recipient slot. A production
   design would need its own connection lifetime policy. Map start discards old
   refs and rules; recreate manually after clients are active.

## Evidence and decision rule

`Transmit.stats()` counters are global: increasing `bitsCleared` shows some bits
were stripped, not which recipient saw a given layout. On a fully isolated run,
compare same-duration no-rule / owner-only / nobody deltas, keeping observer
conditions stable. Zero deltas despite valid baselines may indicate bypass or a
non-PVS path. Do not treat a successful `setVisibleTo` or server state write as
client evidence. A rejected native write reports `REFUSED` and stops that paint;
discard its partial observation and clean/retry. Schema shape and unit-test mocks
cannot answer the delivery experiment; the local tests cover failure reporting
and cleanup retention only.

Screen recordings establish **rendering** only. Before stating that spectators
did not **receive** an entity/state, additionally collect recipient-side entity
or decoded replication evidence keyed to the exact entity index/serial and fresh
markers, covering cold join and warm updates. The concrete instrumentation depends
on the available client/debug build; unparsed packet captures or demo files alone
are not proof. If that observation is unavailable, report delivery as **unproven**
even when the visual matrix passes. No confidential production API follows from
visual absence alone.

Record results using this template (all cells are pending today):

| Stage/build/slots | Recipient rule | State slots/marker | A screen | B screen | Counter delta | Recipient evidence | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Pending | Pending | Pending | Pending | Pending | Pending | Pending | Not run |

## Cleanup

Run `s2_hudprivacy clean`, confirm the status entity list is empty, and unload
`@demo/hud-privacy-probe` through the normal isolated-session plugin workflow.
If removal fails, `clean` reports the labels and retains their refs/rules for a
retry. Do not unload until the status entity list is empty: final unload attempts
cleanup but cannot veto the runtime releasing rules if entity removal still fails.
Never remove all `custom_hud_layout`
entities: other plugins or the map may own them. Remove only the staged probe
archive afterward and restore the recorded isolated-session configuration.
