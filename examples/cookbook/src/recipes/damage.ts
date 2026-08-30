import type { Recipe } from "../recipe.ts";
import type { DamageInfo } from "@s2script/sdk/damage";
import { command } from "@s2script/sdk/commands";

/**
 * SDKHook OnTakeDamage is the SDKHooks pre-hook: every point of incoming damage on the
 * hooked entity passes through it before the engine applies it. DamageInfo is a
 * block-scoped view of that one event — attacker, inflictor, and victim are readonly
 * EntityRef | null (resolve them, don't mutate them); damage and damageType are readable,
 * but only `damage` is actually writable — assigning it (including 0, to block the hit
 * outright) changes what the engine applies. damageType is the raw bit-flag mask, kept
 * numeric here rather than decoded, since which bits mean what is engine data, not
 * something this recipe should hardcode.
 *
 * The cookbook plugin SDKHooks player pawns and folds into this recipe. sm_damage only
 * toggles whether the handler actually *modifies* anything, so loading this recipe
 * doesn't quietly start halving damage on a live server.
 */
let halving = false;

export const damageRecipe: Recipe = {
  name: "damage",
  describe: "toggle a damage pre-hook that halves incoming damage (sm_damage)",
  onTakeDamage(info: DamageInfo) {
    const atk = info.attacker;
    const vic = info.victim;
    console.log("[cookbook] damage onPre: damage=" + info.damage + " type=" + info.damageType
      + " victim=" + (vic ? vic.index + "/" + vic.id : "none")
      + " attacker=" + (atk ? atk.index + "/" + atk.id : "none")
      + (halving ? " -> halved" : ""));
    if (halving) info.damage = info.damage / 2;
  },
  register() {
    command("sm_damage", (cmd) => {
      halving = !halving;
      cmd.reply(halving
        ? "damage hook now HALVING incoming damage — see server log"
        : "damage hook back to logging only");
    });
  },
};
