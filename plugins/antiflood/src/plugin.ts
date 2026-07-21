// @s2script/antiflood — the first non-command base plugin: a passive chat-flood moderator over the
// raw-chat subscriber (ctx.clients.onSay). A client spamming say/say_team is throttled by a pure
// leaky-bucket model; a flooded message is suppressed by returning HookResult.Handled, and the client
// gets a throttled "slow down" notice (SM parity). Config-driven (flood_time / max_tokens),
// live-reloadable via ctx.config.onChange.

import { plugin } from "@s2script/sdk/plugin";
import { Chat } from "@s2script/sdk/chat";
import { config } from "@s2script/sdk/config";
import { HookResult } from "@s2script/sdk/events";
import { ChatColors } from "@s2script/cs2";
import { floodStep } from "./flood";

interface SlotState { tokens: number; lastTime: number; lastNotify: number; }
const state = new Map<number, SlotState>();
const NOTIFY_INTERVAL = 2.0; // seconds — throttle the "slow down" notice so it isn't itself spammy

export default plugin((ctx) => {
  // Log tuning changes so an admin editing the config file sees them take effect (also opts this
  // plugin into the loader's live-reload watch, so getFloat/getInt below read fresh values).
  ctx.config.onChange(() => {
    console.log("[antiflood] config changed — flood_time=" + config.getFloat("flood_time") + " max_tokens=" + config.getInt("max_tokens"));
  });

  ctx.clients.onSay((slot, _text, _teamonly) => {
    const floodTime = config.getFloat("flood_time");
    if (floodTime <= 0) return HookResult.Continue; // disabled

    // Base SM antiflood throttles EVERYONE (admins included); admin-immunity is a separate opt-in
    // system, deferred as a follow-up. Time source: Date.now() (wall-clock ms -> seconds).
    const maxTokens = config.getInt("max_tokens");
    const now = Date.now() / 1000;
    const prev = state.get(slot) ?? { tokens: 0, lastTime: 0, lastNotify: 0 };
    const r = floodStep({ tokens: prev.tokens, lastTime: prev.lastTime }, now, floodTime, maxTokens);

    // On a blocked message, tell the client to slow down — but throttle the notice itself so a
    // sustained flood doesn't produce a wall of notices (they'd be the only lines the flooder sees).
    let lastNotify = prev.lastNotify;
    if (r.block && now - lastNotify >= NOTIFY_INTERVAL) {
      // Leading space so the red byte lands on the text (a leading color byte is swallowed).
      Chat.toSlot(slot, " " + ChatColors.Red + "[antiflood] You are sending messages too fast. Please slow down.");
      lastNotify = now;
    }
    state.set(slot, { tokens: r.tokens, lastTime: r.lastTime, lastNotify });
    return r.block ? HookResult.Handled : HookResult.Continue;
  });

  console.log("[antiflood] onLoad — chat flood protection active (flood_time=" + config.getFloat("flood_time") + " max_tokens=" + config.getInt("max_tokens") + ")");
});
