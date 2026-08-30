// cookbook — one file per API, all registered under a single plugin.
//
// Browse src/recipes/ for the API you want; each file is self-contained and
// readable on its own. Run `sm_list` on a server to see everything registered.
//
// This is a DEMO plugin: it registers a lot of commands and is not part of the
// shipped release. Copy a recipe into your own plugin rather than loading this.
import { RECIPES } from "./recipes/index.ts";
import { command } from "@s2script/sdk/commands";
import { SDKHook, SDKHookType, Entity } from "@s2script/sdk";
import type { Client } from "@s2script/sdk/clients";
import type { DamageInfo, EntityRef } from "@s2script/sdk";
import type { HookResultValue } from "@s2script/sdk/events";
import type { PrecacheContext } from "@s2script/sdk/sound";
import type { UserCmdView } from "@s2script/sdk/usercmd";

export function OnPluginStart(): void {
  for (const recipe of RECIPES) {
    recipe.register();
  }

  command("sm_list", (cmd) => {
    cmd.reply(`${RECIPES.length} recipes:`);
    for (const r of RECIPES) cmd.reply(`  sm_${r.name} — ${r.describe}`);
  });

  console.log(`[cookbook] loaded ${RECIPES.length} recipes — run sm_list`);

  for (const pawn of Entity.findByClass("player")) {
    SDKHook(pawn, SDKHookType.OnTakeDamage, onTakeDamage);
  }
}

export function OnGameFrame(): void {
  for (const r of RECIPES) r.onGameFrame?.();
}

export function OnMapStart(map: string): void {
  for (const r of RECIPES) r.onMapStart?.(map);
}

export function OnPrecache(pc: PrecacheContext): void {
  for (const r of RECIPES) r.onPrecache?.(pc);
}

export function OnEntityCreated(entity: EntityRef | null, className: string): void {
  if (!entity || className !== "player") return;
  SDKHook(entity, SDKHookType.OnTakeDamage, onTakeDamage);
}

function onTakeDamage(info: DamageInfo) {
  let acc: number | undefined;
  for (const r of RECIPES) {
    const v = r.onTakeDamage?.(info);
    if (typeof v === "number") acc = Math.max(acc ?? 0, v);
  }
  return acc as HookResultValue | undefined;
}

export function OnPlayerRunCmd(cmd: UserCmdView, info: { slot: number }): HookResultValue | void {
  let acc: number | undefined;
  for (const r of RECIPES) {
    const v = r.onPlayerRunCmd?.(cmd, info);
    if (typeof v === "number") acc = Math.max(acc ?? 0, v);
  }
  return acc as HookResultValue | undefined;
}

export function OnClientConnected(c: Client): void | Promise<void> {
  for (const r of RECIPES) void r.onClientConnected?.(c);
}

export function OnClientPutInServer(c: Client): void | Promise<void> {
  for (const r of RECIPES) void r.onClientPutInServer?.(c);
}

export function OnClientActive(c: Client): void | Promise<void> {
  for (const r of RECIPES) void r.onClientActive?.(c);
}

export function OnClientPostAdminCheck(c: Client): void | Promise<void> {
  for (const r of RECIPES) void r.onClientPostAdminCheck?.(c);
}

export function OnClientDisconnect(c: Client): void {
  for (const r of RECIPES) r.onClientDisconnect?.(c);
}

export function OnClientSettingsChanged(c: Client): void {
  for (const r of RECIPES) r.onClientSettingsChanged?.(c);
}

export function OnClientVoice(c: Client): void {
  for (const r of RECIPES) r.onClientVoice?.(c);
}
