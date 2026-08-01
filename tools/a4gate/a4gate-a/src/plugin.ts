// a4gate-a — live-gate fixture A for the A4 dispatch chain (#55-#60). NOT a shipped plugin.
//
// Drives the four checks in docs/superpowers/plans/2026-07-31-a4-live-gate.md. Pair with a4gate-b,
// which observes the same channels from a SECOND plugin context — the cross-plugin composition is the
// whole point of check 2.
//
// Every line is prefixed [A4GATE-A#<load>] so `docker logs | grep A4GATE` reads as a transcript and the
// load counter makes a hot reload visible.
import { plugin } from "@s2script/sdk/plugin";
import { HookResult } from "@s2script/sdk/events";
import { createEntity } from "@s2script/sdk/entity";

interface State { load: number; }

export default plugin((ctx) => {
  const load = ((ctx.previous as State | undefined)?.load ?? 0) + 1;
  const L = (m: string) => console.log(`[A4GATE-A#${load}] ${m}`);
  L("loaded");

  // ---------------------------------------------------------------- check 2
  // Two damage handlers in ONE context, so their relative order is registration order and therefore
  // deterministic (cross-plugin order is not). h1 cycles Handled -> Stop -> Continue across the three
  // synthetic dispatches the shim fires at frames 300/900/1800 under S2_DAMAGE_SELFTEST, so one boot
  // exercises all three collapse outcomes with no rcon race.
  let dmg = 0;
  ctx.entities.onDamage((info) => {
    dmg += 1;
    if (dmg === 1) {
      L(`dmg#1 h1 saw=${info.damage} -> zeroing, return Handled`);
      info.damage = 0;
      return HookResult.Handled;   // must NOT truncate: h2 and b must still run (this is the B2 fix)
    }
    if (dmg === 2) {
      L(`dmg#2 h1 saw=${info.damage} -> return Stop`);
      return HookResult.Stop;      // must truncate: h2 and b must NOT run
    }
    L(`dmg#${dmg} h1 saw=${info.damage} -> return Continue`);
    return HookResult.Continue;
  });
  ctx.entities.onDamage((info) => {
    L(`dmg#${dmg} h2 RAN saw=${info.damage}`);
  });

  // ---------------------------------------------------------------- check 1
  // The three lazy engine-op installs are all idempotent shim-side, so the failure mode worth gating is
  // "never installs" — i.e. the whole-store is_empty() sample landing AFTER subscribe_into, which would
  // leave the feature silently dead. So: does each of these fire at all.
  let pre = 0, post = 0, created = 0, runcmd = 0;
  ctx.events.onPre("player_spawn", () => { pre += 1; if (pre <= 3) L(`onPre  player_spawn #${pre}`); });
  ctx.events.on("player_spawn", () => { post += 1; if (post <= 3) L(`onPost player_spawn #${post}`); });
  ctx.entities.onCreate("*", (e, cls) => {
    created += 1;
    if (created <= 5) L(`onCreate #${created} ${cls} idx=${e?.index ?? -1}`);
  });
  ctx.clients.onRunCmd(() => { runcmd += 1; if (runcmd === 1) L("onRunCmd FIRED (first)"); });

  // ---------------------------------------------------------------- check 4
  // A view-backed payload built by a lifted build_args body. The relay is created from a command
  // handler, so its own onCreate is re-entrancy-skipped (expected) — but a DELAYED acceptInput is
  // queued by the engine and fires from the engine's I/O queue on a later frame, outside our isolate
  // borrow, so the output dispatch does reach JS.
  let outputs = 0;
  ctx.entities.onOutput("logic_relay", "OnTrigger", (ev) => {
    outputs += 1;
    L(`onOutput ${ev.output} caller=${ev.caller?.index ?? -1} activator=${ev.activator?.index ?? -1} value="${ev.value}" delay=${ev.delay}`);
  });

  ctx.commands.registerServer("a4_relay", () => {
    const relay = createEntity("logic_relay", { targetname: "a4gate_relay" });
    if (!relay) { L("a4_relay: createEntity FAILED"); return; }
    L(`a4_relay: created idx=${relay.index}, queuing delayed Trigger`);
    relay.acceptInput("Trigger", undefined, undefined, undefined, 0.5);
  });

  ctx.commands.registerServer("a4_report", () => {
    L(`REPORT dmg=${dmg} pre=${pre} post=${post} created=${created} runcmd=${runcmd} outputs=${outputs}`);
  });

  return {
    onUnload() { L(`unloading (dmg=${dmg} pre=${pre} created=${created})`); },
    state(): State { return { load }; },
  };
});
