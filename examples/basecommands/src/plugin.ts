import { Commands } from "@s2script/commands";
import { Chat } from "@s2script/chat";
import { Admin, ADMFLAG } from "@s2script/admin";

// Slice 6.2 live gate — admin-gated commands. sm_say is now registered via Commands.registerAdmin with
// ADMFLAG.CHAT: the server console / rcon is root (always allowed); an in-game player needs the CHAT flag
// (from admins.json or a runtime Admin.add), else it replies "You do not have access." Chat.toAll delivers
// the SayText2 chat message (6.1c). Admin cache = host-global (file admins.json ⊕ runtime), from @s2script/admin.
export function onLoad(): void {
  Commands.registerAdmin("sm_say", ADMFLAG.CHAT, (ctx) => {
    const msg = ctx.argString.trim();
    if (!msg) { ctx.reply("Usage: sm_say <message>"); return; }
    Chat.toAll("[SM] " + msg);
    console.log("[basecommands] sm_say by slot=" + ctx.callerSlot + " msg=" + msg);
  });

  // 6.2 live-gate diagnostic: prove the admin cache works live (rcon-verifiable, no human client needed).
  Admin.add("76561199000000009", ADMFLAG.KICK | ADMFLAG.CHAT);   // runtime tier
  const t = Admin.get("76561199000000009");
  console.log("[basecommands] admin diag: runtime-add hasKick=" + (t ? String(t.hasFlags(ADMFLAG.KICK)) : "null")
    + " hasBan=" + (t ? String(t.hasFlags(ADMFLAG.BAN)) : "null"));
  console.log("[basecommands] admin diag: slot0=" + (Admin.forSlot(0) ? "admin" : "not-admin (bot/steamid=0)"));
  console.log("[basecommands] onLoad — sm_say registered (registerAdmin ADMFLAG.CHAT)");
}

export function onUnload(): void { console.log("[basecommands] onUnload"); }
