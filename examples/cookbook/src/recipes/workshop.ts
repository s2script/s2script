import { Steam, Workshop, WorkshopItemState, command, HookResult } from "@s2script/sdk";

/**
 * Steam Workshop on the server — check, download and inspect workshop items.
 *
 *   sm_ws_status          the server's Steam session (logged on, SteamID, public IP)
 *   sm_ws_get <id>        download (or update) workshop item <id> and report the result
 *   sm_ws_info <id>       the item's state flags, download progress and install folder
 *
 * Downloads go through the server's own Steam session, the same one `host_workshop_map` uses, so
 * a map fetched here changes level without waiting for a download.
 */
export const name = "workshop";
export const describe = "Steam Workshop downloads (sm_ws_status / _get / _info)";

function flags(state: number): string {
  const names = Object.entries(WorkshopItemState).filter(([, bit]) => bit !== 0 && (state & bit) !== 0);
  return names.length ? names.map(([n]) => n).join("+") : "None";
}

export function OnPluginStart(): void {
  Workshop.onDownloadResult((e) => console.log(`[cookbook] workshop ${e.id}: EResult ${e.result}`));

  command("sm_ws_status", (cmd) => {
    if (!Steam.isAvailable()) {
      cmd.reply("[cookbook] Steam is not available on this server");
      return HookResult.Handled;
    }
    cmd.reply(`[cookbook] logged on: ${Steam.isLoggedOn()}, secure: ${Steam.isSecure()}, ` +
      `id: ${Steam.steamId() ?? "-"}, ip: ${Steam.publicIp() ?? "-"}`);
    return HookResult.Handled;
  });

  command("sm_ws_get", (cmd) => {
    const id = cmd.arg(0);
    cmd.reply(`[cookbook] downloading ${id}...`);
    Workshop.download(id, { highPriority: true }).then((r) =>
      cmd.reply(r.ok ? `[cookbook] ${r.id} is ready` : `[cookbook] ${r.id} failed (EResult ${r.result})`));
    return HookResult.Handled;
  });

  command("sm_ws_info", (cmd) => {
    const id = cmd.arg(0);
    const state = Workshop.state(id);
    if (state === null) {
      cmd.reply("[cookbook] usage: sm_ws_info <id> — Steam unavailable or bad id");
      return HookResult.Handled;
    }
    const progress = Workshop.downloadProgress(id);
    const install = Workshop.installInfo(id);
    cmd.reply(`[cookbook] ${id}: ${flags(state)}` +
      (progress ? `, ${progress.bytesDownloaded}/${progress.bytesTotal} bytes` : "") +
      (install ? `, at ${install.folder}` : ""));
    return HookResult.Handled;
  });
}
