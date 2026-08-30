// @s2script/antiflood — the first non-command base plugin: a passive chat-flood moderator over the
// raw-chat subscriber (OnClientSayCommand). A client spamming say/say_team is throttled by a pure
// leaky-bucket model; a flooded message is suppressed by returning HookResult.Handled, and the client
// gets a throttled "slow down" notice (SM parity). Config-driven (flood_time / max_tokens),
// live-reloadable via config.onChange.

import { translations, config, Chat, HookResult, Translations } from "@s2script/sdk";
import { floodStep } from "./flood";

interface SlotState { tokens: number; lastTime: number; lastNotify: number; }
const state = new Map<number, SlotState>();
const NOTIFY_INTERVAL = 2.0; // seconds — throttle the "slow down" notice so it isn't itself spammy

export function OnPluginStart(): void {
  translations.load("antiflood", "common");

  // Arm the config-file watcher so flood_time / max_tokens pick up live edits.
  config.onChange(() => {
    /* getters read rematerialized values on the next message */
  });
}

export function OnClientSayCommand(slot: number, _text: string, _teamonly: boolean): typeof HookResult.Continue | typeof HookResult.Handled {
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
      Chat.toSlot(slot, Translations.translate(slot, "Flood Warning"));
      lastNotify = now;
    }
    state.set(slot, { tokens: r.tokens, lastTime: r.lastTime, lastNotify });
    return r.block ? HookResult.Handled : HookResult.Continue;
}
