// Live-gate fixture for SDKHook(OnTakeDamage/OnTakeDamagePost) on the game-package damage adapter.
// Not shipped. Prefix [DMGPROBE]. Drive with `dmg_hook`, `dmg_report`, `dmg_scale <factor>`.
import { HookResult } from "@s2script/sdk/events";
import { SDKHook, SDKHookType } from "@s2script/sdk";
import { command } from "@s2script/sdk/commands";
import { Player } from "@s2script/cs2";
import { after } from "@s2script/sdk/timers";
import { createEntity } from "@s2script/sdk/entity";

let pre = 0;
let post = 0;
let scale = 1;
let last = "";

export function OnPluginStart(): void {
  const L = (m: string) => console.log(`[DMGPROBE] ${m}`);
  L("loaded");

  command.server("dmg_hook", () => {
    for (const p of Player.all()) {
      const pawn = p.pawn;
      if (!pawn) continue;
      const a = SDKHook(pawn.ref, SDKHookType.OnTakeDamage, (info) => {
        pre += 1;
        last = `hookedOn=${pawn.ref.index} victim=${info.victim ? info.victim.index : "null"} damage=${info.damage} type=${info.damageType} attacker=${info.attacker ? info.attacker.index : "null"}`;
        if (scale !== 1) { info.damage = info.damage * scale; return HookResult.Changed; }
        return HookResult.Continue;
      });
      const b = SDKHook(pawn.ref, SDKHookType.OnTakeDamagePost, () => { post += 1; });
      L(`hooked slot=${p.slot} pre=${a} post=${b} health=${pawn.health}`);
    }
  });
  // Fall damage goes through the engine's damage function; slap-style health writes do not.
  command.server("dmg_fall", () => {
    for (const p of Player.all()) {
      const pawn = p.pawn;
      const o = pawn?.origin;
      if (!pawn || !o) continue;
      L(`drop slot=${p.slot} z=${o.z} ok=${pawn.ref.teleport([o.x, o.y, o.z + 600], null, [0, 0, -1500])} zNow=${pawn.origin?.z}`);
    }
    after(2000, () => { for (const p of Player.all()) L(`  after slot=${p.slot} z=${p.pawn?.origin?.z} health=${p.pawn?.health} pre=${pre}`); });
  });
  // point_hurt aimed at the pawn via !activator: full damage through the engine damage path.
  command.server("dmg_hurt", (cmd) => {
    const amount = Number(cmd.arg(0) || "10");
    const p = Player.all()[0];
    const o = p?.pawn?.origin;
    if (!p || !o) { L("hurt: no pawn"); return; }
    const hurt = createEntity("point_hurt", { origin: `${o.x} ${o.y} ${o.z + 16}`, DamageTarget: "!activator", Damage: amount, DamageType: 0 });
    L(`hurt slot=${p.slot} amount=${amount} created=${hurt !== null} fired=${hurt ? hurt.acceptInput("Hurt", "", p.pawn!.ref) : false}`);
    after(1000, () => { L(`  after slot=${p.slot} health=${p.pawn?.health} pre=${pre} post=${post} last: ${last}`); hurt?.remove(); });
  });
  command.server("dmg_scale", (cmd) => { scale = Number(cmd.arg(0)); L(`scale=${scale}`); });
  command.server("dmg_report", () => {
    L(`REPORT pre=${pre} post=${post} last: ${last}`);
    for (const p of Player.all()) L(`  slot=${p.slot} pawnIndex=${p.pawn ? p.pawn.ref.index : "-"} health=${p.pawn ? p.pawn.health : "no pawn"}`);
  });
}
