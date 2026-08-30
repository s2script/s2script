import { command } from "@s2script/sdk/commands";
import type { CommandInvocation } from "@s2script/sdk/commands";
import type { DamageInfo } from "@s2script/sdk/damage";
import type { HookResultValue } from "@s2script/sdk/events";
import { ADMFLAG } from "@s2script/sdk/admin";

export function OnPluginStart(): void {
  command.admin("sm_kick", ADMFLAG.KICK, kick);
}

function kick(cmd: CommandInvocation): HookResultValue | void {
  cmd.reply("kicked");
}

export function OnTakeDamage(info: DamageInfo): void {
  info.damage = info.damage / 2;
}
