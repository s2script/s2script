/** Throwaway live experiment. No automatic entities, state writes, or client commands. */
import { Clients, command, createEntity, HookResult, Transmit } from "@s2script/sdk";
import type { EntityRef } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

// Test-local bridge to the SAME already-bound natives used by games/cs2/js/ui.js.
// This deliberately bypasses that library's resource -> one entity cache so we can
// create independent recipients' entities without proposing a public private-HUD API.
// No offsets, pointers, signatures, or raw-memory writes are introduced here.
// Core returns undefined for a successful void call, null for rejected invocation.
type ClassCall = (entity: EntityRef, slot: number, panel: string, name: string, status: number) => void | null;
type TextCall = (entity: EntityRef, slot: number, panel: string, name: string, text: string) => void | null;
interface Calls {
  call(name: "setHasClassForPlayer"): ClassCall | null;
  call(name: "setDialogVariableStringForPlayer"): TextCall | null;
  status(name: string): string;
}
const bridge = globalThis as typeof globalThis & { __s2pkg_cs2_calls?: Calls };
const RESOURCE = "panorama/layout/custom_game/s2script_lib.xml";
const owned = new Map<string, EntityRef>();

function cleanup(): string[] {
  const failed: string[] = [];
  for (const [label, entity] of owned) {
    // Remove before dropping its visibility rule; never deliberately reveal on cleanup.
    if (entity.isValid() && !entity.remove()) {
      failed.push(label);
      continue;
    }
    owned.delete(label);
  }
  // Keep failed refs AND their rules available for a later cleanup attempt.
  if (failed.length === 0) Transmit.resetAll();
  return failed;
}

export function OnPluginStart(): void {
  command.server("s2_hudprivacy", (cmd) => {
    const op = cmd.arg(0);
    const label = cmd.arg(1);
    if (op === "status") {
      cmd.reply(JSON.stringify({
        clients: Clients.all().map((c) => ({ slot: c.slot, name: c.name, signon: c.signonState,
          bot: c.isBot, team: Player.fromSlot(c.slot)?.teamNum })),
        entities: [...owned].map(([key, e]) => ({ key, index: e.index, id: e.id, valid: e.isValid() })),
        transmit: Transmit.stats(),
        classCall: bridge.__s2pkg_cs2_calls?.status("setHasClassForPlayer") ?? "bridge missing",
        textCall: bridge.__s2pkg_cs2_calls?.status("setDialogVariableStringForPlayer") ?? "bridge missing",
      }));
      return HookResult.Handled;
    }
    if (op === "clean") {
      const failed = cleanup();
      cmd.reply(failed.length ? `FAILED to remove ${failed.join(",")}; refs/rules retained, retry clean before unload`
        : "probe entities removed and probe rules reset");
      return HookResult.Handled;
    }
    const fail = (reason: string) => { cmd.reply(`REFUSED: ${reason}`); return HookResult.Handled; };
    if (!/^[AB]$/.test(label)) return fail("label must be A or B");
    if (op === "create") {
      if (owned.get(label)?.isValid()) return fail("label already exists; clean before recreating");
      if (!Clients.all().some((c) => !c.isBot && c.signonState === 6)) return fail("requires an active human client");
      if (!bridge.__s2pkg_cs2_calls?.call("setHasClassForPlayer") ||
          !bridge.__s2pkg_cs2_calls.call("setDialogVariableStringForPlayer")) return fail("HUD calls unavailable");
      const entity = createEntity("custom_hud_layout", {
        targetname: `s2_privacy_probe_${label}`, layout: RESOURCE, origin: "0 0 0",
      });
      if (!entity) return fail("createEntity failed");
      // Start hidden from all. No text has been written. This is not a confidentiality
      // guarantee for a spawn/filter race; cold-join observation is a separate gate.
      if (!Transmit.setVisibleTo(entity, [])) {
        if (!entity.remove()) {
          owned.set(label, entity);
          return fail("transmit unavailable AND entity removal failed; empty entity retained for cleanup");
        }
        return fail("transmit unavailable; removed entity");
      }
      owned.set(label, entity);
      cmd.reply(`created ${label}: index=${entity.index} id=${entity.id}; audience=none`);
      return HookResult.Handled;
    }
    const entity = owned.get(label);
    if (!entity?.isValid()) return fail("no live probe entity; create first");
    if (op === "audience") {
      const audience = cmd.arg(2);
      let ok: boolean;
      if (audience === "all") ok = Transmit.reset(entity);
      else if (audience === "none") ok = Transmit.setVisibleTo(entity, []);
      else {
        if (!/^\d+$/.test(audience) || Number(audience) > 63) return fail("audience: all | none | active human slot 0..63");
        const slot = Number(audience);
        const client = Clients.fromSlot(slot);
        if (!client || client.isBot || client.signonState !== 6) return fail("audience requires active human slot");
        ok = Transmit.setVisibleTo(entity, [slot]);
      }
      cmd.reply(`${ok ? "applied" : "FAILED"} ${label} audience=${audience}; ${JSON.stringify(Transmit.stats())}`);
      return HookResult.Handled;
    }
    if (op === "paint") {
      const target = cmd.arg(2);
      if (target !== "all" && (!/^\d+$/.test(target) || Number(target) > 63)) return fail("paint target: all | slot 0..63");
      const marker = cmd.arg(3);
      if (!/^[A-Za-z0-9_-]{1,32}$/.test(marker)) return fail("use a non-secret marker, 1..32 letters/digits/_/-");
      const setClass = bridge.__s2pkg_cs2_calls?.call("setHasClassForPlayer");
      const setText = bridge.__s2pkg_cs2_calls?.call("setDialogVariableStringForPlayer");
      if (!setClass || !setText) return fail("HUD calls unavailable");
      const slots = target === "all" ? Array.from({ length: 64 }, (_, i) => i) : [Number(target)];
      // A and B use different corners; duplicate resource instances are observable.
      const panel = label === "A" ? "s2_b0" : "s2_b1";
      for (const slot of slots) {
        if (setText(entity, slot, `${panel}_title`, `${panel}_title`, `PROBE ${label}`) === null)
          return fail(`rejected title write: ${label} slot=${slot}; partial paint, discard this observation`);
        if (setText(entity, slot, `${panel}_text`, `${panel}_text`, marker) === null)
          return fail(`rejected marker write: ${label} slot=${slot}; partial paint, discard this observation`);
        if (setClass(entity, slot, panel, "s2-hide", 0) === null)
          return fail(`rejected reveal write: ${label} slot=${slot}; partial paint, discard this observation`);
      }
      cmd.reply(`wrote ${label} state=${target} marker=${marker}; server write is NOT a client receipt result`);
      return HookResult.Handled;
    }
    return fail("status | clean | create A/B | audience A/B all/none/slot | paint A/B all/slot marker");
  });
}

export function OnPluginEnd(): void {
  const failed = cleanup();
  // The runtime releases plugin rules after unload; this callback cannot veto it.
  if (failed.length) console.log(`FAILED final cleanup: ${failed.join(",")}; unload will release rules on remaining entities`);
}
// Map teardown invalidates these handles; do not create or touch entities during map start.
export function OnMapStart(): void { owned.clear(); Transmit.resetAll(); }
