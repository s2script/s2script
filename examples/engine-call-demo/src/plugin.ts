/**
 * engine-call-demo — calling an engine function the framework does not wrap.
 *
 * See README.md for how the pieces fit together. The short version: this plugin ships its own
 * gamedata (gamedata/engine-call.gamedata.jsonc), `s2s build` generated types from it into
 * .s2script/gamedata.d.ts, and `Engine.call()` hands back a plain callable — or null when the
 * descriptor failed a load-time gate.
 *
 *   sm_ec_status        report every declared call and why it is (or is not) available
 *   sm_ec_burn <target> set a player on fire via the declared CBaseModelEntity::Ignite
 */
import { plugin } from "@s2script/sdk/plugin";
import { Engine } from "@s2script/sdk/unsafe";
import { Entity } from "@s2script/sdk/entity";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Player } from "@s2script/cs2";

/** How long a burn lasts, in seconds. Passed as the call's `flFlameLifetime` float. */
const BURN_SECONDS = 10.0;

/**
 * `nFlags` MUST include bit 0x4.
 *
 * With that bit clear, Ignite consults a separate "is this thing ignitable?" virtual, which returns
 * false for every player pawn — so the call returns having done absolutely nothing. It is a silent
 * no-op, not an error, which is why the demo below measures an effect instead of trusting the call.
 */
const IGNITE_FLAGS = 4;

export default plugin((ctx) => {
  // ── Resolve ONCE, at load ────────────────────────────────────────────────────────────────────
  // `Engine.call` returns a plain callable, or null if the descriptor failed a load-time gate:
  // the signature did not match this build, a `prologue` validator rejected the target, there is no
  // entry for this platform, or an operator has not allow-listed this plugin. Guard here, not at
  // every call site.
  //
  // `ignite` is typed: `s2s build` generated its signature from the gamedata, so the argument list
  // below is checked by the same tsc gate that checks the rest of the plugin. Rename the call in the
  // gamedata, or change its args, and this file stops compiling.
  const ignite = Engine.call("ignite");

  // Declared to FAIL on purpose — see the long comment in the gamedata. Keeping it here is the
  // point: one bad descriptor must not take the others down with it.
  const rejected = Engine.call("dropActiveWeaponRejected");

  console.log(`[engine-call-demo] ignite                   -> ${Engine.status("ignite")}`);
  console.log(`[engine-call-demo] dropActiveWeaponRejected -> ${Engine.status("dropActiveWeaponRejected")}`);
  if (rejected) {
    // If this ever fires, the prologue guard has stopped discriminating and the negative fixture is
    // no longer testing anything — that is a framework regression worth shouting about.
    console.error("[engine-call-demo] UNEXPECTED: the negative fixture resolved; the prologue guard is not working");
  }

  ctx.commands.registerAdmin("sm_ec_status", ADMFLAG.GENERIC, (cmd) => {
    cmd.reply(`[engine-call-demo] ignite: ${Engine.status("ignite")}`);
    cmd.reply(`[engine-call-demo] dropActiveWeaponRejected (expected to fail): ${Engine.status("dropActiveWeaponRejected")}`);
    if (!ignite) {
      cmd.reply(
        '[engine-call-demo] to enable: add "@demo/engine-call" under "engine:calls" in ' +
          "addons/s2script/configs/permissions.json, then reload."
      );
    }
  });

  ctx.commands.registerAdmin("sm_ec_burn", ADMFLAG.SLAY, (cmd) => {
    if (!ignite) {
      cmd.reply(`[engine-call-demo] sm_ec_burn unavailable: ${Engine.status("ignite")}`);
      return;
    }
    const targets = Player.target(cmd.arg(0) ?? "@me", cmd.callerSlot);
    if (targets.length === 0) {
      cmd.reply("[engine-call-demo] usage: sm_ec_burn <target>  (no match)");
      return;
    }

    // Count flames BEFORE and AFTER, rather than reporting how many pawns we called on.
    //
    // This is the part worth copying. A declared call that resolves but receives a wrong argument
    // returns cleanly and does nothing — a mis-declared float, or nFlags missing bit 0x4, both look
    // like success from the caller's side. Ignite spawns one CEntityFlame per victim, so a rising
    // count is independent evidence that the ENGINE acted, not just that we made a call.
    const before = Entity.findByClass("entityflame").length;
    let called = 0;
    for (const target of targets) {
      const pawn = target.pawn;
      if (!pawn?.isValid) continue;
      ignite(pawn.ref, BURN_SECONDS, IGNITE_FLAGS, null, 0.0);
      called++;
    }
    const after = Entity.findByClass("entityflame").length;

    cmd.reply(
      `[engine-call-demo] called ignite on ${called} pawn(s); entityflame ${before} -> ${after} ` +
        (after > before ? "(EFFECT CONFIRMED)" : "(NO EFFECT — the call resolved but did nothing)")
    );
  });
});
