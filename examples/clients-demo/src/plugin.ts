// @demo/clients-demo — exercises the @s2script/clients lifecycle events (sub-project 1 live gate).
//
// Subscribes to all six notify events and logs each with the Client handle's live fields. The logs
// confirm: the events fire, the firing order (esp. where fullyConnect lands), Client.steamId/name/
// userId read correctly, isBot for bots, and whether onDisconnect's live-ops are still populated.

import { Clients } from "@s2script/clients";

export function onLoad(): void {
  Clients.onConnect((c) =>
    console.log(`[clients-demo] connect slot=${c.slot} name=${c.name} steamId=${c.steamId} userId=${c.userId} isBot=${c.isBot}`));
  Clients.onPutInServer((c) =>
    console.log(`[clients-demo] putInServer slot=${c.slot} name=${c.name}`));
  Clients.onActive((c) =>
    console.log(`[clients-demo] active slot=${c.slot} name=${c.name}`));
  Clients.onFullyConnect((c) =>
    console.log(`[clients-demo] fullyConnect slot=${c.slot} name=${c.name}`));
  Clients.onDisconnect((c) =>
    console.log(`[clients-demo] disconnect slot=${c.slot} name=${c.name} steamId=${c.steamId}`));
  Clients.onSettingsChanged((c) =>
    console.log(`[clients-demo] settingsChanged slot=${c.slot} name=${c.name}`));

  console.log(`[clients-demo] onLoad — all()=${Clients.all().length} clients`);
}

export function onUnload(): void {
  console.log("[clients-demo] onUnload");
}
