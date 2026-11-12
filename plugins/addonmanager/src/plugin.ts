// @s2script/addonmanager — workshop addons on the server and on every client, without a separate
// Metamod plugin. The settings are MultiAddonManager's, so a server moving over keeps its lists.
//
// `addons_extra` addons are downloaded through the server's Steam session, mounted on the server,
// carried by every map load and sent to every client; `addons_client_extra` addons go to clients
// only. A client downloads one addon per connection, so it reconnects once per addon it lacks the
// first time it joins (s2script's handshake drives that; see @s2script/sdk/addons).
//
// With both lists empty this plugin never touches the addon state, so a server that still runs
// MultiAddonManager is unaffected.

import { command, config, hook, Server, ADMFLAG, HookResult, Steam, Workshop, Addons } from "@s2script/sdk";
import type { CommandInvocation } from "@s2script/sdk";

const ID = /^[0-9]{1,20}$/;
/** NETWORK_DISCONNECT_LOOPSHUTDOWN: the reason a client reconnecting for an addon leaves with. */
const LOOP_SHUTDOWN = 54;
const TAG = "[addonmanager]";

function parseIds(text: string): string[] {
  const out: string[] = [];
  for (const id of text.split(/[\s,]+/)) if (ID.test(id) && !/^0+$/.test(id) && !out.includes(id)) out.push(id);
  return out;
}

/** The working lists: the config's, changed at run time by the commands until the config changes. */
let extra: string[] = [];
let clientExtra: string[] = [];
/** Whether this plugin has taken over the addon state. Once it has, it keeps it until unload. */
let managing = false;
/** Downloads that change what the server loads: when the last one lands, the map reloads. */
const important = new Set<string>();
const downloading = new Set<string>();
/** Downloads asked for before Steam logged on, retried once it does. */
const beforeLogon = new Map<string, boolean>();

function reloadMap(): void {
  const own = Addons.currentMap()[0];
  // A workshop map has to come back through host_workshop_map, or the server drops its addon; a
  // stock or community map (its "addon" is an official folder name) reloads with changelevel.
  if (own && ID.test(own)) Server.command(`host_workshop_map ${own}`);
  else if (Server.mapName) Server.command(`changelevel ${Server.mapName}`);
}

function download(id: string, isImportant: boolean): void {
  if (downloading.has(id)) { if (isImportant) important.add(id); return; }
  if (!Steam.isLoggedOn()) { beforeLogon.set(id, (beforeLogon.get(id) ?? false) || isImportant); return; }
  downloading.add(id);
  if (isImportant) important.add(id);
  console.log(`${TAG} downloading ${id}`);
  void Workshop.download(id).then(r => {
    downloading.delete(id);
    console.log(`${TAG} ${id}: ` + (r.ok ? "downloaded" : `download failed (EResult ${r.result})`));
    if (!important.delete(id)) return;
    if (r.ok) mountAll();
    if (important.size === 0 && r.ok) {
      console.log(`${TAG} addon downloads finished, reloading the map`);
      reloadMap();
    }
  });
}

/** Mount every listed addon that is installed and download the rest. Unmounts addons taken off the list. */
function mountAll(): boolean {
  for (const id of Addons.mounted()) if (!extra.includes(id)) Addons.unmount(id);
  const own = Addons.currentMap();
  let all = true;
  for (const id of extra) {
    if (own.includes(id)) continue;   // the map loads it itself
    if (!Workshop.isInstalled(id)) { all = false; download(id, true); continue; }
    if (config.getBool("addons_mount_download")) download(id, false);
    const info = Workshop.installInfo(id);
    const r = info ? Addons.mount(id, info.folder) : "missing";
    if (r !== "mounted" && r !== "already") {
      all = false;
      console.log(`${TAG} ${id}: not mounted (${r})`);
    }
  }
  return all;
}

/** Hand the lists and timing to s2script. Does nothing until there is something to manage. */
function apply(): void {
  if (!managing && extra.length === 0 && clientExtra.length === 0) return;
  managing = true;
  Addons.configure({
    extraAddonTimeout: config.getFloat("addons_extra_timeout"),
    connectTimeout: config.getFloat("addons_connection_timeout"),
    cacheClients: config.getBool("addons_cache_clients"),
    cacheSeconds: config.getFloat("addons_cache_duration"),
  });
  Addons.setExtraAddons(extra);
  Addons.setClientAddons(clientExtra);
  mountAll();
}

function loadConfig(): void {
  extra = parseIds(config.getString("addons_extra"));
  clientExtra = parseIds(config.getString("addons_client_extra"));
  apply();
}

function describe(): string[] {
  const s = Addons.status();
  const on = (b: boolean) => (b ? "on" : "OFF");
  return [
    `${TAG} mount ${on(s.mount)}, map addons ${on(s.mapAddons)}, client handshake ${on(s.clientHandshake)}`,
    `${TAG} map's own: ${Addons.currentMap().join(",") || "(stock map)"}`,
    `${TAG} extra: ${extra.join(",") || "(none)"}; mounted: ${Addons.mounted().join(",") || "(none)"}`,
    `${TAG} client-only: ${clientExtra.join(",") || "(none)"}`,
    ...(downloading.size ? [`${TAG} downloading: ${[...downloading].join(",")}`] : []),
  ];
}

function idArg(cmd: CommandInvocation, usage: string): string | null {
  const id = cmd.arg(0);
  if (ID.test(id) && !/^0+$/.test(id)) return id;
  cmd.reply(`${TAG} usage: ${usage}`);
  return null;
}

export function OnPluginStart(): void {
  config.onChange(() => loadConfig());
  Steam.onConnected(() => {
    const retry = [...beforeLogon];
    beforeLogon.clear();
    for (const [id, isImportant] of retry) download(id, isImportant);
  });
  hook.onPre("player_disconnect", ev =>
    managing && config.getBool("addons_block_disconnect_messages") && ev.getInt("reason") === LOOP_SHUTDOWN
      ? HookResult.Handled : HookResult.Continue);

  const admin = (name: string, run: (cmd: CommandInvocation) => void) =>
    command.admin(name, ADMFLAG.ROOT, cmd => { run(cmd); return HookResult.Handled; });

  admin("sm_addons", cmd => { for (const line of describe()) cmd.reply(line); });
  admin("sm_addons_add", cmd => {
    const id = idArg(cmd, "sm_addons_add <workshop id>");
    if (!id) return;
    if (extra.includes(id)) { cmd.reply(`${TAG} ${id} is already listed`); return; }
    extra.push(id);
    apply();
    cmd.reply(`${TAG} added ${id}; it reaches clients from the next map load`);
  });
  admin("sm_addons_remove", cmd => {
    const id = idArg(cmd, "sm_addons_remove <workshop id>");
    if (!id) return;
    if (!extra.includes(id)) { cmd.reply(`${TAG} ${id} is not listed`); return; }
    extra = extra.filter(x => x !== id);
    apply();
    cmd.reply(`${TAG} removed ${id}; clients stop loading it from the next map load`);
  });
  admin("sm_addons_add_client", cmd => {
    const id = idArg(cmd, "sm_addons_add_client <workshop id>");
    if (!id) return;
    if (!clientExtra.includes(id)) clientExtra.push(id);
    apply();
    cmd.reply(`${TAG} clients load ${id} from their next connect`);
  });
  admin("sm_addons_remove_client", cmd => {
    const id = idArg(cmd, "sm_addons_remove_client <workshop id>");
    if (!id) return;
    clientExtra = clientExtra.filter(x => x !== id);
    apply();
    cmd.reply(`${TAG} removed client addon ${id}`);
  });
  admin("sm_addons_download", cmd => {
    const id = idArg(cmd, "sm_addons_download <workshop id>");
    if (!id) return;
    download(id, false);
    cmd.reply(`${TAG} download of ${id} requested`);
  });
  admin("sm_addons_clear_cache", cmd => {
    if (!managing) { cmd.reply(`${TAG} no addons are configured`); return; }
    Addons.clearClientCache();
    cmd.reply(`${TAG} every client downloads its addons again on its next connect`);
  });

  loadConfig();
}

export function OnMapStart(): void {
  // Pick up addons that finished installing since, and ask for updates when configured.
  if (managing) mountAll();
}
