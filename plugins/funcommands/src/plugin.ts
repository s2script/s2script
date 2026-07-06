// @s2script/funcommands — SourceMod funcommands: fun admin effects.
//
//   v1 ships gravity / noclip / freeze — all schema-field writes (no game-function RE):
//     - sm_gravity  -> pawn.gravityScale + actualGravityScale (generated setters)
//     - sm_noclip   -> pawn.moveType toggled WALK<->NOCLIP (needs the uint8 write kind)
//     - sm_freeze   -> pawn.moveType = NONE, auto-restored to WALK after [seconds]
//   With no target argument, each command targets the CALLER (self) — SM behavior.
//   DEFERRED: sm_blind (needs a black-screen CUserMessageFade via a new client_fade op — the flashbang
//   fields produced no visible effect), sm_burn (an ignite game-function, no framework sig to port),
//   sm_beacon (a particle/temp-entity subsystem). All three are documented follow-ups.

import { Commands, CommandContext } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Player, Pawn } from "@s2script/cs2";
import { delay } from "@s2script/timers";

// MoveType_t (const.h)
const WALK = 2;
const NOCLIP = 7;
const NONE = 0;

// Resolve the target, apply `fn` to each live pawn, and reply with the count. With no target argument,
// defaults to the caller (self) — SM behavior — unless run from the console, which must name a target.
function forEachPawn(ctx: CommandContext, usage: string, verb: string, fn: (p: Player, pw: Pawn) => void): void {
  let pattern = ctx.arg(0);
  if (!pattern) {
    if (ctx.callerSlot < 0) { ctx.reply("[SM] Usage: " + usage); return; } // console must name a target
    pattern = "@me"; // in-game with no arg → self
  }
  const targets = Player.target(pattern, ctx.callerSlot);
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

  // sm_blind is DEFERRED to the next cut — it needs a proper black screen fade (a CUserMessageFade via the
  // SayText2 reflection path), not the flashbang fields (which produced no visible effect). Coming as a
  // follow-up with a new client_fade op.

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

  console.log("[funcommands] onLoad — gravity/noclip/freeze/unfreeze registered (blind/burn/beacon deferred)");
}

export function onUnload(): void {
  console.log("[funcommands] onUnload");
}
