import { Commands } from "@s2script/sdk/commands";
import { Chat } from "@s2script/sdk/chat";
import { Admin, ADMFLAG } from "@s2script/sdk/admin";
import { Player, ChatColors, Activity } from "@s2script/cs2";
import { HookResult } from "@s2script/sdk/events";

const GREEN = ChatColors.Green, WHITE = ChatColors.White;

function actorName(slot: number): string {
  if (slot < 0) return "Console";
  const p = Player.fromSlot(slot);
  return (p && p.playerName) ? p.playerName : "";
}

function doSay(actorSlot: number, msg: string): void {
  for (const p of Player.allConnected()) {
    const src = Activity.formatSource(actorSlot, p.slot);
    if (src.show) Chat.toSlot(p.slot, " " + GREEN + "(ALL) " + src.name + ": " + WHITE + msg);
  }
}

function doAdminChat(actorSlot: number, msg: string): void {
  const name = actorName(actorSlot);
  for (const p of Player.allConnected()) {
    const a = Admin.forSlot(p.slot);
    if (a && a.hasFlags(ADMFLAG.CHAT)) Chat.toSlot(p.slot, " " + GREEN + "(ADMINS) " + name + ": " + WHITE + msg);
  }
}

function doPsay(actorSlot: number, target: Player, msg: string): void {
  const name = actorName(actorSlot);
  const tn = target.playerName || "";
  // Recipient sees who it was directed to + who sent it; sender gets a confirmation echo.
  Chat.toSlot(target.slot, " " + GREEN + "(private to " + tn + ") " + name + ": " + WHITE + msg);
  if (actorSlot >= 0 && actorSlot !== target.slot) {
    Chat.toSlot(actorSlot, " " + GREEN + "(private to " + tn + ") " + WHITE + msg);
  }
}

// resolve exactly one target from a name token; returns null and replies on none/ambiguous
function resolveOne(pattern: string, callerSlot: number, reply: (m: string) => void): Player | null {
  const matches = Player.target(pattern, callerSlot);
  if (matches.length === 0) { reply("No matching players"); return null; }
  if (matches.length > 1) { reply("Multiple players match '" + pattern + "'"); return null; }
  return matches[0];
}

export function onLoad(): void {
  Commands.registerAdmin("sm_say", ADMFLAG.CHAT, (ctx) => {
    const msg = ctx.argString.trim();
    if (!msg) { ctx.reply("Usage: sm_say <message>"); return; }
    doSay(ctx.callerSlot, msg);
  });

  Commands.registerAdmin("sm_chat", ADMFLAG.CHAT, (ctx) => {
    const msg = ctx.argString.trim();
    if (!msg) { ctx.reply("Usage: sm_chat <message>"); return; }
    doAdminChat(ctx.callerSlot, msg);
  });

  Commands.registerAdmin("sm_psay", ADMFLAG.CHAT, (ctx) => {
    const s = ctx.argString.trim();
    const sp = s.indexOf(" ");
    if (sp < 0) { ctx.reply("Usage: sm_psay <target> <message>"); return; }
    const targetPat = s.slice(0, sp), msg = s.slice(sp + 1).trim();
    if (!msg) { ctx.reply("Usage: sm_psay <target> <message>"); return; }
    const t = resolveOne(targetPat, ctx.callerSlot, (m) => ctx.reply(m));
    if (t) doPsay(ctx.callerSlot, t, msg);
  });

  // SourceMod @ chat triggers, over the raw-chat subscriber.
  Chat.onMessage((slot, text, teamonly) => {
    if (text[0] !== "@") return HookResult.Continue;
    const admin = Admin.forSlot(slot);
    if (!admin || !admin.hasFlags(ADMFLAG.CHAT)) return HookResult.Continue; // non-admin @ = normal chat
    if (text.startsWith("@@")) {
      const rest = text.slice(2).trim();
      const sp = rest.indexOf(" ");
      if (sp < 0) { Chat.toSlot(slot, "Usage: @@<target> <message>"); return HookResult.Handled; }
      const t = resolveOne(rest.slice(0, sp), slot, (m) => Chat.toSlot(slot, m));
      if (t) doPsay(slot, t, rest.slice(sp + 1).trim());
      return HookResult.Handled;
    }
    const body = text.slice(1).trim();
    if (!body) return HookResult.Handled; // bare "@" with no message: consume, send nothing
    if (teamonly) doAdminChat(slot, body);
    else doSay(slot, body);
    return HookResult.Handled;
  });
}
