// @s2script/funcommands — SourceMod funcommands: fun admin effects.
//
//   v1 ships gravity / noclip / freeze — all schema-field writes (no game-function RE):
//     - sm_gravity  -> pawn.gravityScale + actualGravityScale (generated setters)
//     - sm_noclip   -> pawn.moveType toggled WALK<->NOCLIP (needs the uint8 write kind)
//     - sm_freeze   -> pawn.moveType = NONE, auto-restored to WALK after [seconds]
//     - sm_blind    -> Fade.blind (CUserMessageFade black-screen fade via @s2script/usermessages)
//   With no target argument, each command targets the CALLER (self) — SM behavior.
//   DEFERRED: sm_burn (an ignite game-function, no framework sig to port),
//   sm_beacon (a particle/temp-entity subsystem). Both are documented follow-ups.

import { plugin } from "@s2script/sdk/plugin";
import { CommandInvocation } from "@s2script/sdk/commands";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Player, Pawn, Fade } from "@s2script/cs2";
import { delay } from "@s2script/sdk/timers";

// MoveType_t (const.h)
const WALK = 2;
const NOCLIP = 7;
const NONE = 0;

// Resolve the target, apply `fn` to each live pawn, and reply with the count. With no target argument,
// defaults to the caller (self) — SM behavior — unless run from the console, which must name a target.
// Convention: filterImmunity=true for a punitive command (drops targets of higher immunity than the
// caller); filterImmunity=false for a reversal/benign command (no filter — e.g. un-freezing).
function forEachPawn(cmd: CommandInvocation, usage: string, verb: string, fn: (p: Player, pw: Pawn) => void, filterImmunity: boolean): void {
  let pattern = cmd.arg(0);
  if (!pattern) {
    if (cmd.callerSlot < 0) { cmd.reply("[SM] Usage: " + usage); return; } // console must name a target
    pattern = "@me"; // in-game with no arg → self
  }
  const targets = Player.target(pattern, cmd.callerSlot, filterImmunity);
  if (targets.length === 0) { cmd.reply("[SM] No matching players."); return; }
  let n = 0;
  for (const p of targets) {
    const pw = p.pawn;
    if (pw) { fn(p, pw); n++; }
  }
  cmd.reply("[SM] " + verb + " " + n + " player" + (n === 1 ? "" : "s") + ".");
}

export default plugin((ctx) => {
  // sm_gravity <target> [factor] — factor multiplies the player's gravity (1 = normal, <1 floaty, >1 heavy).
  ctx.commands.registerAdmin("sm_gravity", ADMFLAG.SLAY, (cmd) => {
    const factor = cmd.argFloat(1, 1.0);
    forEachPawn(cmd, "sm_gravity <target> [factor]", "Set gravity for", (_p, pw) => {
      pw.gravityScale = factor;
      pw.actualGravityScale = factor;
    }, true);
  });

  // sm_blind <target> [seconds] — full black-screen fade (CUserMessageFade) via the generic
  // @s2script/usermessages reflection path (Fade.blind). Replaces the flashbang-field approach.
  ctx.commands.registerAdmin("sm_blind", ADMFLAG.SLAY, (cmd) => {
    const secs = cmd.argFloat(1, 2);   // sm_blind <target> [seconds]: args[0]=target (forEachPawn), args[1]=seconds
    const durMs = (secs > 0 ? secs : 2) * 1000;
    forEachPawn(cmd, "sm_blind <target> [seconds]", "Blinded", (p, _pw) => {
      Fade.blind(p.slot, durMs);
    }, true);
  });

  // sm_noclip <target> — toggle noclip (WALK <-> NOCLIP).
  ctx.commands.registerAdmin("sm_noclip", ADMFLAG.SLAY, (cmd) => {
    forEachPawn(cmd, "sm_noclip <target>", "Toggled noclip for", (_p, pw) => {
      pw.moveType = pw.moveType === NOCLIP ? WALK : NOCLIP;
    }, true);
  });

  // sm_freeze <target> [seconds] — freeze in place; auto-unfreeze after [seconds] (0 = until sm_unfreeze).
  ctx.commands.registerAdmin("sm_freeze", ADMFLAG.SLAY, (cmd) => {
    const secs = cmd.argFloat(1, 0);
    forEachPawn(cmd, "sm_freeze <target> [seconds]", "Froze", (p, pw) => {
      pw.moveType = NONE;
      if (secs > 0) {
        const slot = p.slot;
        delay(secs * 1000).then(() => {
          const q = Player.fromSlot(slot); // re-resolve — the slot may have been reused
          if (q && q.pawn) q.pawn.moveType = WALK;
        });
      }
    }, true);
  });

  // sm_unfreeze <target> — restore movement.
  ctx.commands.registerAdmin("sm_unfreeze", ADMFLAG.SLAY, (cmd) => {
    forEachPawn(cmd, "sm_unfreeze <target>", "Unfroze", (_p, pw) => {
      pw.moveType = WALK;
    }, false);
  });

  console.log("[funcommands] onLoad — gravity/noclip/freeze/unfreeze/blind registered (burn/beacon deferred)");
});
