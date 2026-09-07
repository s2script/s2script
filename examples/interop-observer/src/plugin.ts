import { Clients, HookResult, watchOptional } from "@s2script/sdk";

export function OnPluginStart(): void {
  watchOptional("@s2script/basebans", (basebans, scope) => {
    scope.own(basebans.on("OnBanRequested", request => {
      console.log(`[interop-observer] ban request ${request.steamId} source=${request.source} actor=${request.actorSteamId}`);
      // Advisory observation: exceptions contribute Continue; this is not authorization.
      return HookResult.Continue;
    }));
    scope.own(basebans.on("OnBanRecorded", ({ request, until }) => {
      console.log(`[interop-observer] ban cache record ${request.steamId} until=${until}`);
    }));
    scope.own(basebans.on("OnBanRemoved", ({ steamId }) => {
      console.log(`[interop-observer] ban cache removed ${steamId}`);
    }));
  });
  watchOptional("@s2script/basecomm", (basecomm, scope) => {
    scope.own(basecomm.on("OnClientMuteChanged", ({ steamId, state }) => {
      console.log(`[interop-observer] mute ${steamId}=${state}`);
    }));
    scope.own(basecomm.on("OnClientGagChanged", ({ steamId, state }) => {
      console.log(`[interop-observer] gag ${steamId}=${state}`);
    }));

    // Notifications are changes, not history. Query connected identities after subscribing on
    // every attachment so a replacement provider's current policy is visible after reload.
    for (const client of Clients.all()) {
      const steamId = client.steamId;
      if (steamId === "0") continue;
      console.log(
        `[interop-observer] current ${steamId} muted=${basecomm.isMuted(steamId)} gagged=${basecomm.isGagged(steamId)}`,
      );
    }
  });
}
