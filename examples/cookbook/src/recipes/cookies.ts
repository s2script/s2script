import { Cookies, Clients, command, HookResult } from "@s2script/sdk";
const OFFLINE = "76561199888888888";
export const name = "cookies";
export const describe = "bounded online/offline cookie admission and empty-string cache round-trip (sm_cookies)";
export function OnPluginStart(): void {
  const boots = Cookies.register("demo_boots", { default: "0" });
  const empty = Cookies.register("empty_test", { default: "DEFAULT" });
  command("sm_cookies", (cmd) => {
    // Clientprefs is the sole database writer. Synthetic identities use setAuthId; a fabricated
    // Client cannot bypass connection-generation validation. This also works from server console.
    if (!Cookies.setAuthId(OFFLINE, boots, String(Date.now()))) {
      cmd.reply("[cookbook] offline preference rejected; retry when persistence capacity recovers.");
      return HookResult.Handled;
    }
    const client = Clients.fromSlot(cmd.callerSlot);
    if (client && client.steamId !== "0") {
      if (!Cookies.set(client, empty, "")) cmd.reply("[cookbook] preference rejected; please retry.");
      else cmd.reply(`[cookbook] empty=[${Cookies.get(client, empty)}] getTime=${Cookies.getTime(client, empty)}`);
    }
    cmd.reply("[cookbook] offline preference accepted for persistence (DB commit is asynchronous).");
    return HookResult.Handled;
  });
}
