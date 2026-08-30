import {
  command,
  topmenu,
  HookResult,
  ADMFLAG,
  Admin,
  Server,
  Plugins,
  Menu,
  MenuStyle,
  SDKHook,
  SDKHookType,
  Entity,
} from "@s2script/sdk";
import type { Command, Client, DamageInfo, EntityRef } from "@s2script/sdk";

export function OnPluginStart(): void {
  command.admin("sm_kick", ADMFLAG.KICK, kick);
  topmenu.addItem("Server Commands", {
    id: "demo:map",
    name: "Change Map",
    flags: ADMFLAG.CHANGEMAP,
    onSelect: openMapMenu,
  });
  for (const pawn of Entity.findByClass("player")) {
    SDKHook(pawn, SDKHookType.OnTakeDamage, onTakeDamage);
  }
}

function kick(cmd: Command): typeof HookResult.Handled {
  cmd.reply("kicked");
  return HookResult.Handled;
}

export function OnEntityCreated(entity: EntityRef | null, className: string): void {
  if (!entity || className !== "player") return;
  SDKHook(entity, SDKHookType.OnTakeDamage, onTakeDamage);
}

function onTakeDamage(info: DamageInfo) {
  info.damage = info.damage / 2;
}

function openMapMenu(adminSlot: number): void {
  void Admin.forSlot(adminSlot);
  void Plugins.list();
  const m = new Menu("Change Map");
  m.style = MenuStyle.Center;
  m.freezePlayer = true;
  if (Server.isMapValid("de_dust2")) m.addItem("de_dust2", "de_dust2");
  m.display(adminSlot, 30);
}

export function OnClientPutInServer(_client: Client): void {}
