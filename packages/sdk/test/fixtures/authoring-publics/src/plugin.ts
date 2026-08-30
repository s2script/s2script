import { topmenu, translations } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";
import { SDKHook, SDKHookType, Entity } from "@s2script/sdk";
import type { Command } from "@s2script/sdk/commands";
import type { DamageInfo, EntityRef } from "@s2script/sdk";
import type { HookResultValue } from "@s2script/sdk/events";
import { ADMFLAG } from "@s2script/sdk/admin";

export function OnPluginStart(): void {
  // ES2024 lib proof — Object.groupBy is in ES2024; the typecheck gate must accept it.
  void Object.groupBy(["kicked", "halved"], (s) => s.length);
  translations.load("common");
  command.admin("sm_kick", ADMFLAG.KICK, kick);
  topmenu.addCategory("Server Commands");
  for (const pawn of Entity.findByClass("player")) {
    SDKHook(pawn, SDKHookType.OnTakeDamage, onTakeDamage);
  }
}

function kick(cmd: Command): HookResultValue | void {
  cmd.reply("kicked");
}

export function OnEntityCreated(entity: EntityRef | null, className: string): void {
  if (!entity || className !== "player") return;
  SDKHook(entity, SDKHookType.OnTakeDamage, onTakeDamage);
}

function onTakeDamage(info: DamageInfo) {
  info.damage = info.damage / 2;
}
