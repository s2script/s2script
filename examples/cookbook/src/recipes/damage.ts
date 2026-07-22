import type { Recipe } from "../recipe.ts";
import type { DamageInfo } from "@s2script/sdk/damage";

/**
 * ctx.entities.onDamage is the SDKHooks-equivalent pre-hook (SM's OnTakeDamage):
 * every point of incoming damage passes through it before the engine applies
 * it. DamageInfo is a block-scoped view of that one event — attacker,
 * inflictor, and victim are readonly EntityRef | null (resolve them, don't
 * mutate them); damage and damageType are readable, but only `damage` is
 * actually writable — assigning it (including 0, to block the hit outright)
 * changes what the engine applies. damageType is the raw bit-flag mask, kept
 * numeric here rather than decoded, since which bits mean what is engine
 * data, not something this recipe should hardcode.
 *
 * The hook is a subscription, so it's registered unconditionally at
 * register() time, as it must be. cb_damage only toggles whether the handler
 * actually *modifies* anything, so loading this recipe doesn't quietly start
 * halving damage on a live server.
 */
export const damageRecipe: Recipe = {
  name: "damage",
  describe: "toggle a damage pre-hook that halves incoming damage (cb_damage)",
  register(ctx) {
    let halving = false;

    ctx.entities.onDamage((info: DamageInfo) => {
      const atk = info.attacker;
      const vic = info.victim;
      console.log("[cookbook] damage onPre: damage=" + info.damage + " type=" + info.damageType
        + " victim=" + (vic ? vic.index + "/" + vic.id : "none")
        + " attacker=" + (atk ? atk.index + "/" + atk.id : "none")
        + (halving ? " -> halved" : ""));
      if (halving) info.damage = info.damage / 2;
    });

    ctx.commands.register("cb_damage", (cmd) => {
      halving = !halving;
      cmd.reply(halving
        ? "damage hook now HALVING incoming damage — see server log"
        : "damage hook back to logging only");
    });
  },
};
