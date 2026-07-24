import type { Recipe } from "../recipe.ts";
import { Engine } from "@s2script/sdk/unsafe";
import { Entity } from "@s2script/sdk/entity";
import { Player } from "@s2script/cs2";

/**
 * `@s2script/sdk/unsafe` — call an engine function the core does not wrap, declared in this plugin's
 * OWN gamedata (`examples/cookbook/gamedata/plugin.gamedata.jsonc`) rather than in the framework's.
 *
 * The three things this recipe demonstrates, in order of importance:
 *
 * 1. **Guard once, at load.** `Engine.call(name)` returns a plain callable, or `null` when the
 *    descriptor failed a load-time gate (signature miss on this build, `prologue` rejection, no entry
 *    for this platform, or the plugin is not in the operator allow-list). Resolve it in `register`
 *    and null-check there — never per invocation.
 * 2. **A degraded call is a NAMED reason, not a mystery.** `Engine.status(name)` says why, so a
 *    plugin can tell the operator "this build moved the signature" instead of silently doing nothing.
 * 3. **Default-deny is real.** Declaring `permissions: ["engine:calls"]` in package.json is
 *    necessary but NOT sufficient — until an operator adds this plugin's id to
 *    `addons/s2script/configs/permissions.json`, every declared call resolves to `null`. Running
 *    `cb_unsafe` on a fresh server is expected to report unavailable; that IS the demo.
 *
 *   cb_unsafe          report the descriptor's status (available, or why not)
 *   cb_unsafe_burn     set the caller (or <target>) on fire for 10s via the declared call
 */
export const unsafeRecipe: Recipe = {
  name: "unsafe",
  describe: "plugin-declared engine calls (cb_unsafe / cb_unsafe_burn)",
  register(ctx) {
    // Resolved ONCE, at load. `ignite` is declared in this plugin's gamedata; `s2s build` generated
    // its type into .s2script/gamedata.d.ts, so the argument list below is typechecked at build time.
    const ignite = Engine.call("ignite");
    console.log(`[cookbook] unsafe: ignite -> ${Engine.status("ignite")}`);
    // The negative fixture: expected to be degraded with a named reason (spec criterion 4).
    console.log(`[cookbook] unsafe: dropActiveWeaponRejected -> ${Engine.status("dropActiveWeaponRejected")}`);

    ctx.commands.register("cb_unsafe", (cmd) => {
      const status = Engine.status("ignite");
      cmd.reply(
        ignite
          ? `[cookbook] unsafe: ignite is ${status} — try cb_unsafe_burn`
          : `[cookbook] unsafe: ignite unavailable (${status}). If this says "not permitted", add ` +
            `"@example/cookbook" under "engine:calls" in addons/s2script/configs/permissions.json.`
      );
    });

    ctx.commands.register("cb_unsafe_burn", (cmd) => {
      if (!ignite) {
        cmd.reply(`[cookbook] unsafe: ignite unavailable (${Engine.status("ignite")})`);
        return;
      }
      const targets = cmd.argCount > 0 ? Player.target(cmd.arg(0), cmd.callerSlot) : [];
      const chosen = targets.length > 0 ? targets : Player.allConnected().filter((p) => p.slot === cmd.callerSlot);
      if (chosen.length === 0) {
        cmd.reply("[cookbook] usage: cb_unsafe_burn [target] (run from a client to burn yourself)");
        return;
      }
      // Count flames BEFORE, so the reply proves the call had an EFFECT rather than merely that it
      // was dispatched. That distinction is the whole point: a declared call that resolves but passes
      // a wrong argument (a mis-declared float, or nFlags without bit 0x4) returns cleanly and does
      // nothing — "ignited N pawns" would be a lie. Ignite spawns a CEntityFlame per victim, so a
      // rising flame count is server-side evidence the engine actually acted.
      const before = Entity.findByClass("entityflame").length;
      let called = 0;
      for (const p of chosen) {
        const pawn = p.pawn;
        if (!pawn?.isValid) continue;
        // nFlags MUST be 4 — with bit 0x4 clear the engine consults its "ignitable?" virtual, which
        // is false for every pawn, and the call no-ops silently. See the gamedata comment.
        ignite(pawn.ref, 10.0, 4, null, 0.0);
        called++;
      }
      const after = Entity.findByClass("entityflame").length;
      cmd.reply(
        `[cookbook] unsafe: called ignite on ${called} pawn(s); entityflame ${before} -> ${after}` +
          (after > before ? " (EFFECT CONFIRMED)" : " (NO EFFECT — resolved but did nothing)")
      );
    });
  },
};
