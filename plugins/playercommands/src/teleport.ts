import { ADMFLAG, HookResult, command, nextFrame } from "@s2script/sdk";
import type { Client, EntityRef } from "@s2script/sdk";
import { Player } from "@s2script/cs2";
import { connectedPlayer, livePawn, replyFor, sameRef } from "./actions";

type Location = { origin: number[]; angles: number[] };
const saved = new Map<number, { owner: Player; location: Location }>();
const disconnected = new Map<number, number>();
let generation = 0;

function triple(value: { x: number; y: number; z: number } | null): number[] | null {
  if (!value) return null;
  const copy = [value.x, value.y, value.z];
  return copy.every(Number.isFinite) ? copy : null;
}

function locationOf(player: Player): Location | null {
  const pawn = livePawn(player);
  if (!pawn) return null;
  const origin = triple(pawn.origin), angles = triple(pawn.eyeAngles);
  return origin && angles ? { origin, angles } : null;
}

export function OnMapStart(): void { generation++; saved.clear(); disconnected.clear(); }
export function OnMapEnd(): void { OnMapStart(); }
export function OnPluginEnd(): void { OnMapStart(); }
export function OnClientDisconnect(client: Client): void {
  saved.delete(client.slot);
  disconnected.set(client.slot, (disconnected.get(client.slot) ?? 0) + 1);
}

type OneUp = { player: Player; spawned: EntityRef; origin: number[]; connectionGeneration: number };

export function registerTeleportCommands(): void {
  command.admin("sm_saveloc", ADMFLAG.CHEATS, cmd => {
    const actor = connectedPlayer(cmd.callerSlot);
    if (!actor) { cmd.replyT("Save Location Player Only"); return HookResult.Handled; }
    const location = locationOf(actor);
    if (!location) { cmd.replyT("Save Location Unavailable"); return HookResult.Handled; }
    saved.set(actor.slot, { owner: actor, location });
    cmd.replyT("Location Saved");
    return HookResult.Handled;
  });

  for (const name of ["sm_tp", "sm_teleport"]) command.admin(name, ADMFLAG.SLAY, cmd => {
    if (!cmd.arg(0)) { cmd.replyT("Usage Teleport"); return HookResult.Handled; }
    const selected = Player.target(cmd.arg(0), cmd.callerSlot, true);
    if (!selected.length) { cmd.replyT("No matching players"); return HookResult.Handled; }
    let destination: Location | null = null;
    if (cmd.arg(1)) {
      // A destination is read-only; immunity only filters the players being moved.
      const destinations = Player.target(cmd.arg(1), cmd.callerSlot, false);
      if (destinations.length !== 1) { cmd.replyT("Invalid Teleport Destination"); return HookResult.Handled; }
      destination = locationOf(destinations[0]);
    } else {
      const actor = connectedPlayer(cmd.callerSlot), entry = saved.get(cmd.callerSlot);
      if (actor && entry?.owner.ref.isValid() && sameRef(actor.ref, entry.owner.ref)) destination = entry.location;
      if (!destination) { cmd.replyT("No Saved Location"); return HookResult.Handled; }
    }
    if (!destination) { cmd.replyT("Invalid Teleport Destination"); return HookResult.Handled; }
    const reply = replyFor(cmd), mapGeneration = generation;
    let count = 0;
    for (const player of selected) {
      if (generation !== mapGeneration) break;
      const pawn = livePawn(player);
      // Copies per target keep native/reentrant callbacks from changing the next target's transform.
      if (pawn?.ref.teleport([...destination.origin], [...destination.angles], [0, 0, 0])) count++;
    }
    reply("Teleported Players", count);
    return HookResult.Handled;
  });

  for (const name of ["sm_1up", "sm_oneup"]) command.admin(name, ADMFLAG.SLAY, cmd => {
    if (!cmd.arg(0)) { cmd.replyT("Usage Oneup"); return HookResult.Handled; }
    const selected = Player.target(cmd.arg(0), cmd.callerSlot, true);
    if (!selected.length) { cmd.replyT("No matching players"); return HookResult.Handled; }
    const reply = replyFor(cmd), mapGeneration = generation;
    const pending: OneUp[] = [];
    for (const player of selected) {
      if (generation !== mapGeneration) break;
      if (!player.ref.isValid() || player.pawnIsAlive !== false) continue;
      const dead = player.pawn;
      if (!dead?.isValid || dead.health === null || dead.health > 0 || !dead.hasDeathInfo) continue;
      const origin = triple(dead.deathInfoOrigin);
      if (!origin) continue;
      const connectionGeneration = disconnected.get(player.slot) ?? 0;
      if (!player.respawn() || generation !== mapGeneration) continue;
      // Keep only a ref to the new pawn. Resolve the current body again on the next frame.
      const spawned = player.pawn?.ref;
      if (spawned) pending.push({ player, spawned, origin, connectionGeneration });
    }
    if (!pending.length) { reply("Oneupped Players", 0); return HookResult.Handled; }
    void nextFrame().then(() => {
      if (generation !== mapGeneration) return;
      let count = 0;
      for (const job of pending) {
        if (generation !== mapGeneration) break;
        if ((disconnected.get(job.player.slot) ?? 0) !== job.connectionGeneration) continue;
        const pawn = livePawn(job.player);
        if (pawn && sameRef(pawn.ref, job.spawned) && pawn.ref.teleport([...job.origin], null, [0, 0, 0])) count++;
      }
      reply("Oneupped Players", count);
    }).catch(() => {
      if (generation === mapGeneration) reply("Oneup Scheduling Failed");
    });
    return HookResult.Handled;
  });
}
