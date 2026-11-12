import { HookResult, command } from "@s2script/sdk";
import type { UserCmdPostInfo, UserCmdView } from "@s2script/sdk";
import { Pawn } from "@s2script/cs2";

type Mode = "off" | "jump" | "side" | "block" | "trace";
const IN_JUMP = 2n; // buttons is a bigint
const modeBySlot = new Map<number, Mode>();
let logN = 0;
let verbose = false;
// "trace": the pawn's origin as "pre" saw it, per slot, and the post-phase per-tick command count.
const kin = new Float64Array(Pawn.KINEMATICS_SLOTS);
const preOrigin = new Map<number, [number, number, number]>();
const lastTick = new Map<number, number>();
const perTick = new Map<number, number>();
let traceN = 0;

/**
 * OnPlayerRunCmd reads a player's live per-tick input (7 fields) before
 * the engine processes it, and can modify or block it by returning a
 * HookResult. Gated behind sm_usercmd so it doesn't fight normal movement
 * until a player opts in:
 *
 *   sm_usercmd off      — read only (default)
 *   sm_usercmd jump     — force forwardMove=0 + IN_JUMP (modify proof)
 *   sm_usercmd side     — zero forwardMove, route it to sideMove (the sideways surf style — proves sign+effect)
 *   sm_usercmd block    — return HookResult.Handled (neutralize the whole input)
 *   sm_usercmd trace    — "pre" vs "post": logs the pawn's origin before and after a command ran,
 *                         the server tick, the command number and how many commands ran this tick
 *                         (1-in-64 commands). While moving, the post origin differs from the pre one.
 *   sm_usercmd verbose  — toggle the read-proof log line below (off by default — this hook fires
 *                         every tick for every player, so even throttled to 1-in-64 it's ~1
 *                         line/sec/player; loading the cookbook must not spam the console on its
 *                         own, same reasoning as recipes/damage.ts defaulting its effect off)
 */
export const name = "usercmd";
export const describe = "read/modify/block a player's per-tick input (sm_usercmd off|jump|side|block|trace|verbose)";

export function OnPlayerRunCmd(cmd: UserCmdView, info: { slot: number }) {
    const slot = info.slot;
    if (verbose && (logN++ & 0x3f) === 0) {
      const schemaBtn = Pawn.forSlot(slot)?.buttons ?? -1;
      const va = cmd.viewAngles;
      console.log(
        `[cookbook] usercmd slot=${slot} fwd=${cmd.forwardMove} side=${cmd.sideMove} up=${cmd.upMove}` +
          ` imp=${cmd.impulse} btn=${cmd.buttons} schemaBtn=${schemaBtn}` +
          ` pitch=${va.x.toFixed(0)} yaw=${va.y.toFixed(0)}`,
      );
    }
    const mode = modeBySlot.get(slot) ?? "off";
    if (mode === "trace") {
      const pawn = Pawn.forSlot(slot);
      if (pawn && pawn.readKinematics(kin)) preOrigin.set(slot, [kin[0], kin[1], kin[2]]);
    } else if (mode === "jump") {
      cmd.forwardMove = 0;
      cmd.buttons = cmd.buttons | IN_JUMP;
    } else if (mode === "side") {
      const fwd = cmd.forwardMove;
      cmd.forwardMove = 0;
      cmd.sideMove = fwd;
    } else if (mode === "block") {
      return HookResult.Handled;
    }
    return HookResult.Continue;
  }

// "post": after this command's movement ran. Read-only; the return value is ignored.
export function OnPlayerRunCmdPost(cmd: UserCmdView, info: UserCmdPostInfo): void {
  const slot = info.slot;
  if (modeBySlot.get(slot) !== "trace") return;
  const n = lastTick.get(slot) === info.tick ? (perTick.get(slot) ?? 0) + 1 : 1;
  lastTick.set(slot, info.tick);
  perTick.set(slot, n);
  if ((traceN++ & 0x3f) !== 0) return;
  const pawn = Pawn.forSlot(slot);
  const before = preOrigin.get(slot);
  if (!pawn || !before || !pawn.readKinematics(kin)) return;
  const moved = Math.hypot(kin[0] - before[0], kin[1] - before[1], kin[2] - before[2]);
  console.log(
    `[cookbook] usercmd post slot=${slot} tick=${info.tick} cmd#=${info.commandNumber} cmdsThisTick=${n}` +
      ` pre=(${before[0].toFixed(1)},${before[1].toFixed(1)}) post=(${kin[0].toFixed(1)},${kin[1].toFixed(1)})` +
      ` moved=${moved.toFixed(2)} pressed=${cmd.pressedButtons} changed=${cmd.changedButtons}`,
  );
}

export function OnPluginStart(): void {
  command("sm_usercmd", (cmd) => {
    const arg = cmd.argsFrom(0).trim();
    if (arg === "verbose") {
      verbose = !verbose;
      cmd.reply(`usercmd verbose logging = ${verbose ? "on" : "off"}`);
      return HookResult.Handled;
    }
    if (cmd.callerSlot < 0) { cmd.reply("run in-game"); return HookResult.Handled; }
    const mode: Mode = arg === "jump" || arg === "side" || arg === "block" || arg === "trace" ? arg : "off";
    modeBySlot.set(cmd.callerSlot, mode);
    cmd.reply(`usercmd mode = ${mode}`);
    return HookResult.Handled;
  });
}
