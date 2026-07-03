import { Events } from "@s2script/cs2";
import { Player } from "@s2script/cs2";

// Slice 5D.1 — game events. Subscribe to a couple of CS2 events via the typed Events.on overlay;
// resolve players with Player.fromSlot(ev.getPlayerSlot(...)). The GameEvent accessor is live during
// the synchronous handler (raw engine event never crosses to JS).
export function onLoad(): void {
  console.log("[demo] onLoad (game events)");

  Events.on("player_spawn", (ev) => {
    const p = Player.fromSlot(ev.getPlayerSlot("userid"));
    console.log("[demo] player_spawn slot=" + ev.getPlayerSlot("userid")
      + " name=" + (p ? p.playerName : "?"));
  });

  Events.on("player_death", (ev) => {
    const victim = Player.fromSlot(ev.getPlayerSlot("userid"));
    const attacker = Player.fromSlot(ev.getPlayerSlot("attacker"));
    console.log("[demo] player_death victim=" + (victim ? victim.playerName : "?")
      + " attacker=" + (attacker ? attacker.playerName : "?")
      + " weapon=" + ev.getString("weapon")
      + " headshot=" + ev.getBool("headshot"));
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
