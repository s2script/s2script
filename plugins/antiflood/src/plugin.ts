// @s2script/antiflood — the first non-command base plugin: a passive chat-flood moderator over the
// raw-chat subscriber (Chat.onMessage). A client spamming say/say_team is throttled by a pure
// token-decay model; a flooded message is suppressed by returning HookResult.Handled. Admins are
// exempt. Config-driven (flood_time / max_tokens), live-reloadable via config.onChange.

import { Chat } from "@s2script/chat";
import { Admin, ADMFLAG } from "@s2script/admin";
import { config } from "@s2script/config";
import { HookResult } from "@s2script/events";
import { floodStep, FloodState } from "./flood";

const state = new Map<number, FloodState>();

export function onLoad(): void {
  // Log tuning changes so an admin editing the config file sees them take effect (also opts this
  // plugin into the loader's live-reload watch, so getFloat/getInt below read fresh values).
  config.onChange(() => {
    console.log("[antiflood] config changed — flood_time=" + config.getFloat("flood_time") + " max_tokens=" + config.getInt("max_tokens"));
  });

  Chat.onMessage((slot, _text, _teamonly) => {
    const floodTime = config.getFloat("flood_time");
    if (floodTime <= 0) return HookResult.Continue; // disabled

    const admin = Admin.forSlot(slot);
    if (admin && admin.hasFlags(ADMFLAG.CHAT)) return HookResult.Continue; // admins aren't throttled

    const maxTokens = config.getInt("max_tokens");
    const prev = state.get(slot) ?? { tokens: 0, lastTime: 0 };
    const r = floodStep(prev, Date.now() / 1000, floodTime, maxTokens);
    state.set(slot, { tokens: r.tokens, lastTime: r.lastTime });
    return r.block ? HookResult.Handled : HookResult.Continue;
  });

  console.log("[antiflood] onLoad — chat flood protection active (flood_time=" + config.getFloat("flood_time") + " max_tokens=" + config.getInt("max_tokens") + ")");
}

export function onUnload(): void { console.log("[antiflood] onUnload"); }
