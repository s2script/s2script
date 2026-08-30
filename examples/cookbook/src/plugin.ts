// cookbook — one file per API, all registered under a single plugin.
//
// Browse src/recipes/ for the API you want; each file is a copy-pasteable
// plugin (`export function OnPluginStart` plus any other named publics it
// needs). This entry fans those publics — a plugin may export each one once.
// Run `sm_list` on a server to see everything registered.
//
// This is a DEMO plugin: it registers a lot of commands and is not part of the
// shipped release. Copy a recipe into your own plugin rather than loading this.
import { RECIPES } from "./recipes/index.ts";
import { command, HookResult } from "@s2script/sdk";
import type { Client, EntityRef, HookResultValue, PrecacheContext, UserCmdView } from "@s2script/sdk";

export function OnPluginStart(): void {
  for (const recipe of RECIPES) {
    recipe.OnPluginStart();
  }

  command("sm_list", (cmd) => {
    cmd.reply(`${RECIPES.length} recipes:`);
    for (const r of RECIPES) cmd.reply(`  sm_${r.name} — ${r.describe}`);
    return HookResult.Handled;
  });

  console.log(`[cookbook] loaded ${RECIPES.length} recipes — run sm_list`);
}

export function OnGameFrame(): void {
  for (const r of RECIPES) r.OnGameFrame?.();
}

export function OnMapStart(map: string): void {
  for (const r of RECIPES) r.OnMapStart?.(map);
}

export function OnPrecache(pc: PrecacheContext): void {
  for (const r of RECIPES) r.OnPrecache?.(pc);
}

export function OnEntityCreated(entity: EntityRef | null, className: string): void {
  for (const r of RECIPES) r.OnEntityCreated?.(entity, className);
}

export function OnPlayerRunCmd(cmd: UserCmdView, info: { slot: number }): HookResultValue | void {
  let acc: number | undefined;
  for (const r of RECIPES) {
    const v = r.OnPlayerRunCmd?.(cmd, info);
    if (typeof v === "number") acc = Math.max(acc ?? 0, v);
  }
  return acc as HookResultValue | undefined;
}

export function OnClientConnected(c: Client): void | Promise<void> {
  for (const r of RECIPES) void r.OnClientConnected?.(c);
}

export function OnClientPutInServer(c: Client): void | Promise<void> {
  for (const r of RECIPES) void r.OnClientPutInServer?.(c);
}

export function OnClientActive(c: Client): void | Promise<void> {
  for (const r of RECIPES) void r.OnClientActive?.(c);
}

export function OnClientPostAdminCheck(c: Client): void | Promise<void> {
  for (const r of RECIPES) void r.OnClientPostAdminCheck?.(c);
}

export function OnClientDisconnect(c: Client): void {
  for (const r of RECIPES) r.OnClientDisconnect?.(c);
}

export function OnClientSettingsChanged(c: Client): void {
  for (const r of RECIPES) r.OnClientSettingsChanged?.(c);
}

export function OnClientVoice(c: Client): void {
  for (const r of RECIPES) r.OnClientVoice?.(c);
}
