// @s2script/basetriggers — SourceMod basetriggers: answer chat phrases (timeleft / thetime / currentmap /
// nextmap). Typing a trigger word in chat broadcasts the answer to everyone; the player's word still shows
// (SM-style — the handler returns Continue, never suppresses).
//
//   nextmap is DEFERRED (no authoritative engine "next map" value) — answered from the `nextlevel` cvar
//   (set only when an admin/vote forced a specific next map), else "Pending". Proper next-map rotation is
//   the domain of a future @s2script/nextmap plugin.
//   timeleft uses mp_timelimit (cvar) + Server.gameTime (map time / curtime), which includes warmup/freeze
//   so it can differ from the HUD by the warmup — approximate, fine for an info trigger.

import { plugin } from "@s2script/sdk/plugin";
import { Chat } from "@s2script/sdk/chat";
import { Server } from "@s2script/sdk/server";
import { HookResult } from "@s2script/sdk/events";
import { nextFrame } from "@s2script/sdk/timers";

function timeLeft(): string {
  const timelimit = parseFloat(Server.getCvar("mp_timelimit")) || 0; // minutes; 0 = no limit
  if (timelimit <= 0) return "Time left: no time limit";
  const left = Math.max(0, Math.round(timelimit * 60 - Server.gameTime));
  if (left <= 0) return "Time left: last round";
  const m = Math.floor(left / 60);
  const s = left % 60;
  return "Time left: " + m + ":" + (s < 10 ? "0" : "") + s;
}

function theTime(): string {
  return "Current time: " + new Date().toLocaleTimeString();
}

function currentMap(): string {
  return "Current map: " + (Server.mapName || "unknown");
}

function nextMap(): string {
  const next = Server.getCvar("nextlevel");
  return "Next map: " + (next ? next : "Pending");
}

export default plugin((ctx) => {
  ctx.clients.onSay((_slot, text, _teamonly) => {
    const t = text.trim().toLowerCase();
    let answer: string | null = null;
    if (t === "timeleft") answer = timeLeft();
    else if (t === "thetime") answer = theTime();
    else if (t === "currentmap" || t === "map") answer = currentMap();
    else if (t === "nextmap") answer = nextMap();
    if (answer !== null) {
      const a = answer;
      // ctx.clients.onSay is a PRE-hook (runs before the say is broadcast), so a synchronous reply would
      // appear BEFORE the player's trigger word. Defer one frame so the word broadcasts first, then the answer.
      nextFrame().then(() => Chat.toAll(a));
    }
    return HookResult.Continue; // never suppress — the player's trigger word still shows (SM behavior)
  });

  console.log("[basetriggers] onLoad — timeleft/thetime/currentmap/nextmap");
});
