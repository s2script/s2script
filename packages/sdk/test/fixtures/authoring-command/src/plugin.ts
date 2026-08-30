import { hook } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";
import { SDKHook, SDKHookType, Entity } from "@s2script/sdk";
import type { CommandInvocation } from "@s2script/sdk/commands";
import type { DamageInfo, EntityRef } from "@s2script/sdk";
import type { HookResultValue } from "@s2script/sdk/events";
import { ADMFLAG } from "@s2script/sdk/admin";

export function OnPluginStart(): void {
  command.admin("sm_kick", ADMFLAG.KICK, kick);
  hook.on("round_start", () => {});
  for (const pawn of Entity.findByClass("player")) {
    SDKHook(pawn, SDKHookType.OnTakeDamage, onTakeDamage);
  }
}

function kick(cmd: CommandInvocation): HookResultValue | void {
  cmd.reply("kicked");
}

export function OnEntityCreated(entity: EntityRef | null, className: string): void {
  if (!entity || className !== "player") return;
  SDKHook(entity, SDKHookType.OnTakeDamage, onTakeDamage);
}

function onTakeDamage(info: DamageInfo) {
  info.damage = info.damage / 2;
}
