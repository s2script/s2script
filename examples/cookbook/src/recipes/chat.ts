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
 * The first character position is reserved for the sender's name color, so a color byte with
 * nothing before it is swallowed and never lands on visible text — lead with a literal space
 * instead. (plugins/antiflood and plugins/basechat both do this; see their `" " + ChatColors....`
 * calls.)
 */
export const chatRecipe: Recipe = {
  name: "chat",
  describe: "print to chat, plain and multi-color inline (sm_saycolor)",
  register(ctx) {
    ctx.commands.register("sm_saycolor", (cmd) => {
      if (cmd.callerSlot < 0) { cmd.reply("run in-game — chat has no console channel"); return; }

      // Plain — no control byte at all; renders in the client's default chat color.
      Chat.toSlot(cmd.callerSlot, "[cookbook] plain message, no color byte.");

      // Multi-color inline: leading space (swallowed) so Green actually lands on "[cookbook] ",
      // then White takes over, then Red — one message, three color runs, no shared/global state.
      Chat.toSlot(
        cmd.callerSlot,
        " " + ChatColors.Green + "[cookbook] " + ChatColors.White +
          "green stops here, white starts here, " + ChatColors.Red + "and red from here on.",
      );

      Chat.toAll("[cookbook] sm_saycolor ran — everyone sees this one.");
      cmd.reply("sent 3 chat lines — check chat");
    });
  },
};
