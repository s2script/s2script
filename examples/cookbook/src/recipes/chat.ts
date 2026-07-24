import type { Recipe } from "../recipe.ts";
import { Chat } from "@s2script/sdk/chat";
import { ChatColors } from "@s2script/cs2";

/**
 * Chat.toSlot / Chat.toAll print to player chat (SM's PrintToChat / PrintToChatAll). Colors are
 * SourceMod-style control bytes embedded IN the string, and — this is the detail that trips
 * people up — a control byte colors every character AFTER it, up to the next control byte or the
 * end of the string. Color is a property of a *run* of text, not of the message as a whole, so
 * one message can carry several colors just by interleaving more bytes in.
 *
 * You do NOT need to lead with a space. A Source 2 chat box would otherwise swallow a color byte
 * that sits at the very front of the message, so @s2script/chat auto-prepends an invisible
 * zero-width space to every line — just put a color byte first (`ChatColors.Green + "…"`) and it
 * lands. (Idempotent: if you DO lead with a space or a ZWSP yourself, it's passed through as-is.)
 */
export const chatRecipe: Recipe = {
  name: "chat",
  describe: "print to chat, plain and multi-color inline (sm_saycolor)",
  register(ctx) {
    ctx.commands.register("sm_saycolor", (cmd) => {
      if (cmd.callerSlot < 0) { cmd.reply("run in-game — chat has no console channel"); return; }

      // Plain — no control byte at all; renders in the client's default chat color.
      Chat.toSlot(cmd.callerSlot, "[cookbook] plain message, no color byte.");

      // Multi-color inline: Green lands on "[cookbook] " (the auto ZWSP means no leading space is
      // needed), then White takes over, then Red — one message, three color runs, no shared state.
      Chat.toSlot(
        cmd.callerSlot,
        ChatColors.Green + "[cookbook] " + ChatColors.White +
          "green stops here, white starts here, " + ChatColors.Red + "and red from here on.",
      );

      Chat.toAll("[cookbook] sm_saycolor ran — everyone sees this one.");
      cmd.reply("sent 3 chat lines — check chat");
    });
  },
};
