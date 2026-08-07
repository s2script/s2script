import { plugin } from "@s2script/sdk/plugin";
import { Chat } from "@s2script/sdk/chat";
import { Admin, ADMFLAG } from "@s2script/sdk/admin";
import { Player, Activity } from "@s2script/cs2";
import { HookResult } from "@s2script/sdk/events";
import { Translations } from "@s2script/sdk/translations";
import { phrases } from "./phrases";

function actorName(slot: number): string {
  if (slot < 0) return "Console";
  const p = Player.fromSlot(slot);
  return (p && p.playerName) ? p.playerName : "";
}

function doSay(actorSlot: number, msg: string): void {
  for (const p of Player.allConnected()) {
    const src = Activity.formatSource(actorSlot, p.slot);
    if (src.show) Chat.toSlot(p.slot, Translations.translate(p.slot, "Say All", src.name, msg));
  }
}

function doAdminChat(actorSlot: number, msg: string): void {
  const name = actorName(actorSlot);
  for (const p of Player.allConnected()) {
    const a = Admin.forSlot(p.slot);
    if (a && a.hasFlags(ADMFLAG.CHAT)) {
      Chat.toSlot(p.slot, Translations.translate(p.slot, "Say Admins", name, msg));
    }
  }
}

function doPsay(actorSlot: number, target: Player, msg: string): void {
  const name = actorName(actorSlot);
  const tn = target.playerName || "";
  Chat.toSlot(target.slot, Translations.translate(target.slot, "Say Private To", tn, name, msg));
  if (actorSlot >= 0 && actorSlot !== target.slot) {
    Chat.toSlot(actorSlot, Translations.translate(actorSlot, "Say Private Echo", tn, msg));
  }
}

// resolve exactly one target from a name token; returns null and replies on none/ambiguous
function resolveOne(pattern: string, callerSlot: number, reply: (m: string) => void): Player | null {
  const matches = Player.target(pattern, callerSlot);
  if (matches.length === 0) { reply(Translations.translate(callerSlot, "No matching players")); return null; }
  if (matches.length > 1) {
    reply(Translations.translate(callerSlot, "More than one client matched", pattern));
    return null;
  }
  return matches[0];
}

export default plugin((ctx) => {
  // Own set FIRST, common SECOND: translate takes the first hit across sets, so this order is what
  // lets a plugin override a shared phrase.
  Translations.load("basechat", phrases);
  Translations.load("common");   // shared file in translations/, SourceMod's LoadTranslations("common.phrases")

  // cmd.replyT(key) below: we hold a `cmd` context, so let it pick chat vs console for the reply.
  // Above and further down (resolveOne, the @@ trigger in onSay) there is only a raw slot — no
  // `cmd` — so those call Translations.translate(slot, key) directly instead.
  ctx.commands.registerAdmin("sm_say", ADMFLAG.CHAT, (cmd) => {
    const msg = cmd.argString.trim();
    if (!msg) { cmd.replyT("Usage Say"); return; }
    doSay(cmd.callerSlot, msg);
  });

  ctx.commands.registerAdmin("sm_chat", ADMFLAG.CHAT, (cmd) => {
    const msg = cmd.argString.trim();
    if (!msg) { cmd.replyT("Usage Chat"); return; }
    doAdminChat(cmd.callerSlot, msg);
  });

  ctx.commands.registerAdmin("sm_psay", ADMFLAG.CHAT, (cmd) => {
    const s = cmd.argString.trim();
    const sp = s.indexOf(" ");
    if (sp < 0) { cmd.replyT("Usage Psay"); return; }
    const targetPat = s.slice(0, sp), msg = s.slice(sp + 1).trim();
    if (!msg) { cmd.replyT("Usage Psay"); return; }
    const t = resolveOne(targetPat, cmd.callerSlot, (m) => cmd.reply(m));
    if (t) doPsay(cmd.callerSlot, t, msg);
  });

  // SourceMod @ chat triggers, over the raw-chat subscriber.
  ctx.clients.onSay((slot, text, teamonly) => {
    if (text[0] !== "@") return HookResult.Continue;
    const admin = Admin.forSlot(slot);
    if (!admin || !admin.hasFlags(ADMFLAG.CHAT)) return HookResult.Continue; // non-admin @ = normal chat
    if (text.startsWith("@@")) {
      const rest = text.slice(2).trim();
      const sp = rest.indexOf(" ");
      if (sp < 0) { Chat.toSlot(slot, Translations.translate(slot, "Usage Psay Trigger")); return HookResult.Handled; }
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
});
