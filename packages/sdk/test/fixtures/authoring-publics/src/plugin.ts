import { hook, topmenu, translations } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";
import type { Command } from "@s2script/sdk/commands";
import type { DamageInfo } from "@s2script/sdk/damage";
import type { HookResultValue } from "@s2script/sdk/events";
import { ADMFLAG } from "@s2script/sdk/admin";

export function OnPluginStart(): void {
  // ES2024 lib proof — Object.groupBy is in ES2024; the typecheck gate must accept it.
  void Object.groupBy(["kicked", "halved"], (s) => s.length);
  translations.load("common");
  command.admin("sm_kick", ADMFLAG.KICK, kick);
  hook.entity.onDamage(halve);
  topmenu.addCategory("Server Commands");
}

function kick(cmd: Command): HookResultValue | void {
  cmd.reply("kicked");
}

function halve(info: DamageInfo): void {
  info.damage = info.damage / 2;
}
