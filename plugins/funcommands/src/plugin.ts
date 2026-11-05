// @s2script/funcommands — SourceMod funcommands: fun admin effects.
//
//   v1 ships gravity / noclip / freeze — all schema-field writes (no game-function RE):
//     - sm_gravity  -> pawn.gravityScale + actualGravityScale (generated setters)
//     - sm_noclip   -> pawn.moveType toggled WALK<->NOCLIP (needs the uint8 write kind)
//     - sm_freeze   -> pawn.moveType = NONE, auto-restored to WALK after [seconds]
//     - sm_blind    -> Fade.blind (CUserMessageFade black-screen fade via @s2script/usermessages)
//   The original five commands default to the caller; the additional commands require a target.
//   Also ships god / speed / size / unblind / drug / beacon through typed SDK facilities.
//   DEFERRED: sm_burn (an ignite game-function, no framework sig to port).

import { command, translations, ADMFLAG, HookResult } from "@s2script/sdk";
import type { Client, Command, PhraseKey } from "@s2script/sdk";
import { Events, Player, Pawn, Fade } from "@s2script/cs2";
import { FunEffects } from "./effects";

// MoveType_t (const.h)
const WALK = 2;
const NOCLIP = 7;
const effects = new FunEffects();

// Number() alone accepts whitespace and hexadecimal. Accept complete finite decimal tokens only.
function numberArg(cmd: Command, index: number, min: number, max: number, fallback?: number): number | null {
  const token = cmd.arg(index);
  if (cmd.args.length <= index) return fallback ?? null;
  if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(token)) return null;
  const value = Number(token);
  return Number.isFinite(value) && value >= min && value <= max ? value : null;
}

// Resolve the target, apply `fn` to each live pawn, and reply with the count. With no target argument,
// defaults to the caller (self) — SM behavior — unless run from the console, which must name a target.
// Convention: filterImmunity=true for a punitive command (drops targets of higher immunity than the
// caller); filterImmunity=false for a reversal/benign command (no filter — e.g. un-freezing).
//
// usageKey/singularKey/pluralKey are typed `PhraseKey`, not `string`, so a typo at one of
// the five call sites below is a typecheck error — gen-phrases.mjs's AST scanner can only validate a
// literal key argument to .replyT/.translate directly, and these arrive as arguments to this helper
// instead, so they're invisible to that scan (same limitation as basecomm's forTargets, Task 7).
function forEachPawn(
  cmd: Command,
  usageKey: PhraseKey,
  singularKey: PhraseKey,
  pluralKey: PhraseKey,
  fn: (p: Player, pw: Pawn) => void | boolean,
  filterImmunity: boolean,
  defaultSelf = true,
): void {
  let pattern = cmd.arg(0);
  if (!pattern) {
    if (!defaultSelf || cmd.callerSlot < 0) { cmd.replyT(usageKey); return; } // console must name a target
    pattern = "@me"; // in-game with no arg → self
  }
  const targets = Player.target(pattern, cmd.callerSlot, filterImmunity);
  if (targets.length === 0) { cmd.replyT("No matching players"); return; }
  let n = 0;
  for (const p of targets) {
    const pw = p.pawn;
    if (pw && pw.isValid && fn(p, pw) !== false) n++;
  }
  cmd.replyT(n === 1 ? singularKey : pluralKey, n);
}

export function OnPluginStart(): void {
  translations.load("funcommands", "common");
  effects.start();
  Events.on("player_death", ev => effects.removeSlot(ev.getPlayerSlot("userid")));

  command.admin("sm_god", ADMFLAG.CHEATS, cmd => {
    const mode = numberArg(cmd, 1, -1, 1, 0);
    if (mode === null || !Number.isInteger(mode)) { cmd.replyT("Invalid God Mode"); return HookResult.Handled; }
    forEachPawn(cmd, "Usage God", "Set God For Player", "Set God For Players", (_p, pw) => {
      if (pw.takesDamage === null) return false;
      const takesDamage = mode === 0 ? !pw.takesDamage : mode === -1;
      pw.takesDamage = takesDamage;
      return pw.takesDamage === takesDamage;
    }, true, false);
    return HookResult.Handled;
  });

  command.admin("sm_speed", ADMFLAG.SLAY, cmd => {
    if (!cmd.arg(0)) { cmd.replyT("Usage Speed"); return HookResult.Handled; }
    const factor = numberArg(cmd, 1, .01, 10);
    if (factor === null) { cmd.replyT("Invalid Speed"); return HookResult.Handled; }
    forEachPawn(cmd, "Usage Speed", "Set Speed For Player", "Set Speed For Players", (p, pw) => effects.setSpeed(p, pw, factor), true, false);
    return HookResult.Handled;
  });

  for (const name of ["sm_size", "sm_scale"]) command.admin(name, ADMFLAG.SLAY, cmd => {
    const scale = numberArg(cmd, 1, .2, 4, 1);
    if (scale === null) { cmd.replyT("Invalid Size"); return HookResult.Handled; }
    forEachPawn(cmd, "Usage Size", "Set Size For Player", "Set Size For Players", (_p, pw) => pw.setModelScale(scale), true, false);
    return HookResult.Handled;
  });

  command.admin("sm_unblind", ADMFLAG.SLAY, cmd => {
    forEachPawn(cmd, "Usage Unblind", "Unblinded Player", "Unblinded Players", p => Fade.to(p.slot, { duration: 0, holdTime: 0, color: 0, flags: 0x10 }), true, false);
    return HookResult.Handled;
  });

  command.admin("sm_drug", ADMFLAG.SLAY, cmd => {
    const seconds = numberArg(cmd, 1, 0, Number.MAX_SAFE_INTEGER / 1000, 0);
    if (seconds === null) { cmd.replyT("Invalid Drug Duration"); return HookResult.Handled; }
    forEachPawn(cmd, "Usage Drug", "Drugged Player", "Drugged Players", (p, pw) => effects.drug(p, pw, seconds), true, false);
    return HookResult.Handled;
  });

  command.admin("sm_undrug", ADMFLAG.SLAY, cmd => {
    forEachPawn(cmd, "Usage Undrug", "Undrugged Player", "Undrugged Players", (p, pw) => effects.undrug(p, pw), true, false);
    return HookResult.Handled;
  });

  command.admin("sm_beacon", ADMFLAG.SLAY, cmd => {
    forEachPawn(cmd, "Usage Beacon", "Toggled Beacon For Player", "Toggled Beacon For Players", (p, pw) => effects.beacon(p, pw), true, false);
    return HookResult.Handled;
  });

  // sm_gravity <target> [factor] — factor multiplies the player's gravity (1 = normal, <1 floaty, >1 heavy).
  command.admin("sm_gravity", ADMFLAG.SLAY, (cmd) => {
    const factor = cmd.argFloat(1, 1.0);
    forEachPawn(cmd, "Usage Gravity", "Set Gravity For Player", "Set Gravity For Players", (_p, pw) => {
      pw.gravityScale = factor;
      pw.actualGravityScale = factor;
    }, true);
    return HookResult.Handled;
  });

  // sm_blind <target> [seconds] — full black-screen fade (CUserMessageFade) via the generic
  // @s2script/usermessages reflection path (Fade.blind). Replaces the flashbang-field approach.
  command.admin("sm_blind", ADMFLAG.SLAY, (cmd) => {
    const secs = cmd.argFloat(1, 2);   // sm_blind <target> [seconds]: args[0]=target (forEachPawn), args[1]=seconds
    const durMs = (secs > 0 ? secs : 2) * 1000;
    forEachPawn(cmd, "Usage Blind", "Blinded Player", "Blinded Players", (p, _pw) => {
      Fade.blind(p.slot, durMs);
    }, true);
    return HookResult.Handled;
  });

  // sm_noclip <target> — toggle noclip (WALK <-> NOCLIP).
  command.admin("sm_noclip", ADMFLAG.SLAY, (cmd) => {
    forEachPawn(cmd, "Usage Noclip", "Toggled Noclip For Player", "Toggled Noclip For Players", (_p, pw) => {
      pw.moveType = pw.moveType === NOCLIP ? WALK : NOCLIP;
    }, true);
    return HookResult.Handled;
  });

  // sm_freeze <target> [seconds] — freeze in place; auto-unfreeze after [seconds] (0 = until sm_unfreeze).
  command.admin("sm_freeze", ADMFLAG.SLAY, (cmd) => {
    const secs = cmd.argFloat(1, 0);
    forEachPawn(cmd, "Usage Freeze", "Froze Player", "Froze Players", (p, pw) => effects.freeze(p, pw, secs), true);
    return HookResult.Handled;
  });

  // sm_unfreeze <target> — restore movement.
  command.admin("sm_unfreeze", ADMFLAG.SLAY, (cmd) => {
    forEachPawn(cmd, "Usage Unfreeze", "Unfroze Player", "Unfroze Players", (p, pw) => effects.unfreeze(p, pw), false);
    return HookResult.Handled;
  });
}

export function OnGameFrame(): void { effects.frame(); }
export function OnClientDisconnect(client: Client): void { effects.disconnect(client); }
export function OnMapStart(): void { effects.clear(); }
export function OnMapEnd(): void { effects.clear(); }
export function OnPluginEnd(): void { effects.clear(true); }
