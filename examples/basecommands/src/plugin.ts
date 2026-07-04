import { Commands } from "@s2script/commands";
import { Chat } from "@s2script/chat";

// Slice 6.1 live gate — the command spine. sm_say registers a server command, dispatches to this
// handler with a typed ctx (callerSlot/args/argString/reply), and calls Chat.toAll to broadcast.
// Ungated (admin gating is Slice 6.2). Invoked from the server console / rcon this slice (in-chat
// `!say` triggers come later). Slice 6.1c: Chat.toAll now DELIVERS to each real client's CHAT BOX —
// a CUserMessageSayText2 user message built via protobuf reflection over the game's message object
// (bots are skipped; they have no netchannel). Real-client visual delivery is unverifiable on the
// bots-only gate server, but the message + fields resolve live and the send path is proven.
export function onLoad(): void {
  Commands.register("sm_say", (ctx) => {
    const msg = ctx.argString.trim();
    if (!msg) { ctx.reply("Usage: sm_say <message>"); return; }
    Chat.toAll("[SM] " + msg);
    console.log("[basecommands] sm_say by slot=" + ctx.callerSlot + " msg=" + msg);
  });
  console.log("[basecommands] onLoad — sm_say registered");
}

export function onUnload(): void { console.log("[basecommands] onUnload"); }
