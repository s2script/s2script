import {
  command,
  hook,
  HookResult,
  ADMFLAG,
  Admin,
  Server,
  Plugins,
  Menu,
  MenuStyle,
} from "@s2script/sdk";
import type { Command, Client, DamageInfo } from "@s2script/sdk";

export function OnPluginStart(): void {
  command.admin("sm_kick", ADMFLAG.KICK, kick);
  hook.entity.onDamage(halve);
  hook.topmenu.addItem("Server Commands", {
    id: "demo:map",
    name: "Change Map",
    flags: ADMFLAG.CHANGEMAP,
    onSelect: openMapMenu,
  });
}

function kick(cmd: Command): typeof HookResult.Handled {
  cmd.reply("kicked");
  return HookResult.Handled;
}

function halve(info: DamageInfo): typeof HookResult.Changed {
  info.damage = info.damage / 2;
  return HookResult.Changed;
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
