import { Commands } from "@s2script/commands";
import { Chat } from "@s2script/chat";

// Slice 6.1 live gate — the command spine. sm_say registers a server command, dispatches to this
// handler with a typed ctx (callerSlot/args/argString/reply), and calls Chat.toAll to broadcast.
// Ungated (admin gating is Slice 6.2). Invoked from the server console / rcon this slice (in-chat
// `!say` triggers come later). NOTE: the actual chat SEND is deferred to 6.1b — the shim's client_print
// is a degrade-safe stub until then (the concrete SayText2 protobuf isn't vendored), so Chat.toAll
// currently no-ops per slot; this plugin needs NO change once 6.1b lands the send.
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
