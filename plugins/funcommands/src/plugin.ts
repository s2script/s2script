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
import { Translations } from "@s2script/sdk/translations";
import { phrases } from "./phrases";

// MoveType_t (const.h)
const WALK = 2;
const NOCLIP = 7;
const NONE = 0;

// Resolve the target, apply `fn` to each live pawn, and reply with the count. With no target argument,
// defaults to the caller (self) — SM behavior — unless run from the console, which must name a target.
// Convention: filterImmunity=true for a punitive command (drops targets of higher immunity than the
// caller); filterImmunity=false for a reversal/benign command (no filter — e.g. un-freezing).
//
// usageKey/singularKey/pluralKey are typed `keyof typeof phrases`, not `string`, so a typo at one of
// the five call sites below is a typecheck error — gen-phrases.mjs's AST scanner can only validate a
// literal key argument to .replyT/.translate directly, and these arrive as arguments to this helper
// instead, so they're invisible to that scan (same limitation as basecomm's forTargets, Task 7).
function forEachPawn(
  cmd: CommandInvocation,
  usageKey: keyof typeof phrases,
  singularKey: keyof typeof phrases,
  pluralKey: keyof typeof phrases,
  fn: (p: Player, pw: Pawn) => void,
  filterImmunity: boolean,
): void {
  let pattern = cmd.arg(0);
  if (!pattern) {
    if (cmd.callerSlot < 0) { cmd.replyT(usageKey); return; } // console must name a target
    pattern = "@me"; // in-game with no arg → self
  }
  const targets = Player.target(pattern, cmd.callerSlot, filterImmunity);
  if (targets.length === 0) { cmd.replyT("No matching players"); return; }
  let n = 0;
  for (const p of targets) {
    const pw = p.pawn;
    if (pw) { fn(p, pw); n++; }
  }
  cmd.replyT(n === 1 ? singularKey : pluralKey, n);
}

export default plugin((ctx) => {
  // Own set FIRST, common SECOND: translate takes the first hit across sets, so this order is what
  // lets a plugin override a shared phrase.
  Translations.load("funcommands", phrases);
  Translations.load("common");

  // sm_gravity <target> [factor] — factor multiplies the player's gravity (1 = normal, <1 floaty, >1 heavy).
  ctx.commands.registerAdmin("sm_gravity", ADMFLAG.SLAY, (cmd) => {
    const factor = cmd.argFloat(1, 1.0);
    forEachPawn(cmd, "Usage Gravity", "Set Gravity For Player", "Set Gravity For Players", (_p, pw) => {
      pw.gravityScale = factor;
      pw.actualGravityScale = factor;
    }, true);
  });

  // sm_blind <target> [seconds] — full black-screen fade (CUserMessageFade) via the generic
  // @s2script/usermessages reflection path (Fade.blind). Replaces the flashbang-field approach.
  ctx.commands.registerAdmin("sm_blind", ADMFLAG.SLAY, (cmd) => {
    const secs = cmd.argFloat(1, 2);   // sm_blind <target> [seconds]: args[0]=target (forEachPawn), args[1]=seconds
    const durMs = (secs > 0 ? secs : 2) * 1000;
    forEachPawn(cmd, "Usage Blind", "Blinded Player", "Blinded Players", (p, _pw) => {
      Fade.blind(p.slot, durMs);
    }, true);
  });

  // sm_noclip <target> — toggle noclip (WALK <-> NOCLIP).
  ctx.commands.registerAdmin("sm_noclip", ADMFLAG.SLAY, (cmd) => {
    forEachPawn(cmd, "Usage Noclip", "Toggled Noclip For Player", "Toggled Noclip For Players", (_p, pw) => {
      pw.moveType = pw.moveType === NOCLIP ? WALK : NOCLIP;
    }, true);
  });

  // sm_freeze <target> [seconds] — freeze in place; auto-unfreeze after [seconds] (0 = until sm_unfreeze).
  ctx.commands.registerAdmin("sm_freeze", ADMFLAG.SLAY, (cmd) => {
    const secs = cmd.argFloat(1, 0);
    forEachPawn(cmd, "Usage Freeze", "Froze Player", "Froze Players", (p, pw) => {
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
    forEachPawn(cmd, "Usage Unfreeze", "Unfroze Player", "Unfroze Players", (_p, pw) => {
      pw.moveType = WALK;
    }, false);
  });

  console.log("[funcommands] onLoad — gravity/noclip/freeze/unfreeze/blind registered (burn/beacon deferred)");
});
