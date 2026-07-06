// @s2script/funcommands — SourceMod funcommands: fun admin effects.
//
//   v1 ships gravity / blind / noclip / freeze — all schema-field writes (no game-function RE):
//     - sm_gravity  -> pawn.gravityScale + actualGravityScale (generated setters)
//     - sm_blind    -> pawn.flashDuration + flashMaxAlpha (the CS2 flashbang white-out)
//     - sm_noclip   -> pawn.moveType toggled WALK<->NOCLIP (needs the uint8 write kind)
//     - sm_freeze   -> pawn.moveType = NONE, auto-restored to WALK after [seconds]
//   sm_burn (an ignite game-function with no framework sig to port) and sm_beacon (a particle/temp-entity
//   subsystem) are DEFERRED — both are real from-scratch engine RE.

import { Commands, CommandContext } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Player, Pawn } from "@s2script/cs2";
import { delay } from "@s2script/timers";

// MoveType_t (const.h)
const WALK = 2;
const NOCLIP = 7;
const NONE = 0;

// Resolve the target, apply `fn` to each live pawn, and reply with the count.
function forEachPawn(ctx: CommandContext, usage: string, verb: string, fn: (p: Player, pw: Pawn) => void): void {
  if (!ctx.arg(0)) { ctx.reply("[SM] Usage: " + usage); return; }
  const targets = Player.target(ctx.arg(0), ctx.callerSlot);
  if (targets.length === 0) { ctx.reply("[SM] No matching players."); return; }
  let n = 0;
  for (const p of targets) {
    const pw = p.pawn;
    if (pw) { fn(p, pw); n++; }
  }
  ctx.reply("[SM] " + verb + " " + n + " player" + (n === 1 ? "" : "s") + ".");
}

export function onLoad(): void {
  // sm_gravity <target> [factor] — factor multiplies the player's gravity (1 = normal, <1 floaty, >1 heavy).
  Commands.registerAdmin("sm_gravity", ADMFLAG.SLAY, (ctx) => {
    const factor = ctx.argFloat(1, 1.0);
    forEachPawn(ctx, "sm_gravity <target> [factor]", "Set gravity for", (_p, pw) => {
      pw.gravityScale = factor;
      pw.actualGravityScale = factor;
    });
  });

  // sm_blind <target> [seconds] — flashbang-style white-out for [seconds] (0 = un-blind).
  Commands.registerAdmin("sm_blind", ADMFLAG.SLAY, (ctx) => {
    const secs = ctx.argFloat(1, 20);
    forEachPawn(ctx, "sm_blind <target> [seconds]", secs <= 0 ? "Un-blinded" : "Blinded", (_p, pw) => {
      pw.flashDuration = secs <= 0 ? 0 : secs;
      pw.flashMaxAlpha = secs <= 0 ? 0 : 255;
    });
  });

  // sm_noclip <target> — toggle noclip (WALK <-> NOCLIP).
  Commands.registerAdmin("sm_noclip", ADMFLAG.SLAY, (ctx) => {
    forEachPawn(ctx, "sm_noclip <target>", "Toggled noclip for", (_p, pw) => {
      pw.moveType = pw.moveType === NOCLIP ? WALK : NOCLIP;
    });
  });

  // sm_freeze <target> [seconds] — freeze in place; auto-unfreeze after [seconds] (0 = until sm_unfreeze).
  Commands.registerAdmin("sm_freeze", ADMFLAG.SLAY, (ctx) => {
    const secs = ctx.argFloat(1, 0);
    forEachPawn(ctx, "sm_freeze <target> [seconds]", "Froze", (p, pw) => {
      pw.moveType = NONE;
      if (secs > 0) {
        const slot = p.slot;
        delay(secs * 1000).then(() => {
          const q = Player.fromSlot(slot); // re-resolve — the slot may have been reused
          if (q && q.pawn) q.pawn.moveType = WALK;
        });
      }
    });
  });

  // sm_unfreeze <target> — restore movement.
  Commands.registerAdmin("sm_unfreeze", ADMFLAG.SLAY, (ctx) => {
    forEachPawn(ctx, "sm_unfreeze <target>", "Unfroze", (_p, pw) => {
      pw.moveType = WALK;
    });
  });

  console.log("[funcommands] onLoad — gravity/blind/noclip/freeze/unfreeze registered (burn/beacon deferred)");
}

export function onUnload(): void {
  console.log("[funcommands] onUnload");
}
