// Shared BaseBans service. Bans.add is void and cache-first; see api.d.ts for
// observable outcomes and trusted caller / advisory hook semantics.
import { command, topmenu, translations, ADMFLAG, Bans, Clients, Menu, MenuStyle, Translations, HookResult, publish } from "@s2script/sdk";
import type { Client } from "@s2script/sdk";
import { Player, pickPlayer } from "@s2script/cs2";
import type { TypedPublishHandle } from "@s2script/sdk/interfaces";
import type { Contract, BanRequest, BanResult, UnbanRequest } from "../api";

// Store this canonical sentinel; translate it for immediate and reconnect display.
const BAN_REASON_BY_ADMIN = "Banned by admin";

// The message a banned player sees (chat + console) — shared by the immediate sm_ban path and the
// reconnect enforcement so the wording is identical. `slot` is the BANNED PLAYER (the recipient of
// this message), translated in THEIR language — never the admin who issued the ban. No colour tags
// anywhere in here: kickWithReason delivers via Client.chat/Client.print, which never run through
// the colour-expanding chat/console funnels (see phrases.ts).
function banMessage(slot: number, reason: string, until: number): string {
  const now = Date.now() / 1000;
  const expiry = until === 0
    ? Translations.translate(slot, "Ban Expiry Permanent")
    : Translations.translate(slot, "Ban Expiry Minutes", Math.ceil((until - now) / 60));
  const reasonText = reason === BAN_REASON_BY_ADMIN
    ? Translations.translate(slot, "Ban Reason By Admin")
    : reason || Translations.translate(slot, "Ban Reason Default");
  return Translations.translate(slot, "Ban Message", reasonText, expiry);
}

interface Identity { steamId: string; userId: number; name: string }
function snapshot(p: Player): Identity {
  return { steamId: p.steamId, userId: p.userId, name: p.playerName ?? "" };
}
function resolve(identity: Identity): Player | null {
  const current = Player.fromUserId(identity.userId);
  return current && current.steamId === identity.steamId ? current : null;
}
function validSteamId(value: string): boolean {
  return typeof value === "string" && /^[1-9][0-9]*$/.test(value) &&
    (value.length < 20 || (value.length === 20 && value <= "18446744073709551615"));
}
function validMinutes(minutes: number): boolean {
  const seconds = minutes * 60;
  return Number.isSafeInteger(minutes) && minutes >= 0 && Number.isSafeInteger(seconds) &&
    Number.isSafeInteger(Math.floor(Date.now() / 1000) + seconds);
}
function validRequest(request: BanRequest): boolean {
  return !!request && validSteamId(request.steamId) && validMinutes(request.minutes) &&
    typeof request.reason === "string" &&
    (request.source === "command" || request.source === "menu" || request.source === "plugin") &&
    (request.actorSteamId === null || validSteamId(request.actorSteamId));
}

let service: TypedPublishHandle<Contract>;
// undefined selects a live API target; null deliberately means record-only (sm_addban).
function recordBan(input: BanRequest, target?: Identity | null): BanResult {
  if (!validRequest(input)) return { recorded: false, result: HookResult.Continue };
  const request: BanRequest = { steamId: input.steamId, minutes: input.minutes, reason: input.reason,
    source: input.source, actorSteamId: input.actorSteamId };
  if (target === undefined) {
    const live = Player.allConnected().find(p => p.steamId === request.steamId);
    target = live ? snapshot(live) : null;
  }
  const result = service.dispatch("OnBanRequested", request);
  if (result >= HookResult.Handled) return { recorded: false, result };
  // A disconnected snapshot must not acquire a later connection, even to the same SteamID.
  if (target && !resolve(target)) target = null;
  // Wall time may advance during callbacks; validate expiry again before touching the store.
  if (!validMinutes(request.minutes)) return { recorded: false, result };
  const before = Math.floor(Date.now() / 1000);
  Bans.add(request.steamId, request.minutes, request.reason);
  const after = Math.floor(Date.now() / 1000);
  const record = Bans.get(request.steamId);
  const seconds = request.minutes * 60;
  if (!record || record.reason !== request.reason || !Number.isSafeInteger(record.until) ||
      (request.minutes === 0 ? record.until !== 0 :
        record.until < before + seconds || record.until > after + seconds)) {
    return { recorded: false, result };
  }
  service.emit("OnBanRecorded", { request, until: record.until });
  const current = target ? resolve(target) : null;
  if (current) {
    const slot = current.slot;
    const client = Clients.fromSlot(slot);
    if (client) client.kickWithReason(banMessage(slot, request.reason, record.until));
    else current.kick(Translations.translate(slot, "Kick Ban Reason Fallback",
      request.reason || Translations.translate(slot, "Ban Reason Default")));
  }
  return { recorded: true, result };
}
function ban(request: BanRequest): BanResult { return recordBan(request); }
function unban(request: UnbanRequest): boolean {
  if (!request || !validSteamId(request.steamId)) return false;
  const steamId = request.steamId;
  const removed = Bans.remove(steamId);
  if (removed) service.emit("OnBanRemoved", { steamId });
  return removed;
}
function snapshotActor(slot: number): Identity | null {
  // Actor identity follows the connection; Player.fromSlot requires a live pawn.
  const actor = slot < 0 ? null : Player.allConnected().find(player => player.slot === slot);
  return actor ? snapshot(actor) : null;
}
function resolveActor(slot: number, actor: Identity): Player | null {
  const current = resolve(actor);
  return current && current.slot === slot ? current : null;
}
function canReply(slot: number, actor: Identity | null): boolean {
  // Command.replyT captures a raw slot; guard it after arbitrary interop callbacks.
  return slot < 0 || (actor !== null && resolveActor(slot, actor) !== null);
}
function failurePhrase(result: BanResult): "Ban Intercepted" | "Ban Record Failed" {
  return result.result >= HookResult.Handled ? "Ban Intercepted" : "Ban Record Failed";
}

export function OnPluginStart(): void {
  translations.load("basebans", "common");
  service = publish("@s2script/basebans", { ban, unban });

  command.admin("sm_ban", ADMFLAG.BAN, cmd => {
    const target = cmd.arg(0), minutes = Number(cmd.arg(1)), reason = cmd.argsFrom(2);
    if (!target || !/^\d+$/.test(cmd.arg(1)) || !validMinutes(minutes)) {
      cmd.replyT("Usage Ban"); return HookResult.Handled;
    }
    const targets = Player.target(target, cmd.callerSlot, true);
    if (targets.length === 0) { cmd.replyT("No matching players"); return HookResult.Handled; }
    if (targets.length > 1) { cmd.replyT("Ban Ambiguous Target", target); return HookResult.Handled; }
    const identity = snapshot(targets[0]);
    if (!validSteamId(identity.steamId)) {
      cmd.replyT("Cannot Ban No Steamid", identity.name); return HookResult.Handled;
    }
    const actor = snapshotActor(cmd.callerSlot);
    const request: BanRequest = { steamId: identity.steamId, minutes, reason, source: "command", actorSteamId: cmd.callerSlot < 0 ? null : actor?.steamId ?? "0" };
    // Resolve translated success arguments while the command's actor identity is still current.
    const duration = minutes > 0
      ? Translations.translate(cmd.callerSlot, minutes === 1 ? "Ban Duration Minute" : "Ban Duration Minutes", minutes)
      : Translations.translate(cmd.callerSlot, "Ban Duration Permanently");
    const suffix = reason ? Translations.translate(cmd.callerSlot, "Ban Reason Suffix", reason) : "";
    const outcome = recordBan(request, identity);
    if (!canReply(cmd.callerSlot, actor)) return HookResult.Handled;
    if (outcome.recorded) cmd.replyT("Ban Success", identity.name, duration, suffix);
    else cmd.replyT(failurePhrase(outcome));
    return HookResult.Handled;
  });

  command.admin("sm_unban", ADMFLAG.UNBAN, cmd => {
    const steamId = cmd.arg(0);
    if (!validSteamId(steamId)) { cmd.replyT("Usage Unban"); return HookResult.Handled; }
    const actor = snapshotActor(cmd.callerSlot);
    const removed = unban({ steamId });
    if (canReply(cmd.callerSlot, actor)) cmd.replyT(removed ? "Unban Success" : "Unban Not Banned", steamId);
    return HookResult.Handled;
  });

  command.admin("sm_addban", ADMFLAG.BAN, cmd => {
    const steamId = cmd.arg(0), minutes = Number(cmd.arg(1)), reason = cmd.argsFrom(2);
    if (!validSteamId(steamId) || !/^\d+$/.test(cmd.arg(1)) || !validMinutes(minutes)) {
      cmd.replyT("Usage Addban"); return HookResult.Handled;
    }
    const actor = snapshotActor(cmd.callerSlot);
    const request: BanRequest = { steamId, minutes, reason, source: "command", actorSteamId: cmd.callerSlot < 0 ? null : actor?.steamId ?? "0" };
    const duration = minutes > 0 ? Translations.translate(cmd.callerSlot, "Addban Duration Minutes", minutes)
      : Translations.translate(cmd.callerSlot, "Addban Duration Permanent");
    const suffix = reason ? Translations.translate(cmd.callerSlot, "Addban Reason Suffix", reason) : "";
    const outcome = recordBan(request, null);
    if (!canReply(cmd.callerSlot, actor)) return HookResult.Handled;
    if (outcome.recorded) cmd.replyT("Addban Success", steamId, duration, suffix);
    else cmd.replyT(failurePhrase(outcome));
    return HookResult.Handled;
  });

  topmenu.addTab({ id: "basebans", title: "Bans" });
  topmenu.addItem("basebans", { id: "basebans:kick", name: Translations.translate(-1, "Kick Item"), flags: ADMFLAG.KICK,
    onSelect: adminSlot => pickPlayer(adminSlot, t => t.kick(Translations.translate(t.slot, "Kick By Admin"))) });
  topmenu.addItem("basebans", { id: "basebans:ban", name: Translations.translate(-1, "Ban Item"), flags: ADMFLAG.BAN,
    onSelect: adminSlot => {
      const adminIdentity = snapshotActor(adminSlot);
      if (!adminIdentity || !validSteamId(adminIdentity.steamId)) return;
      pickPlayer(adminSlot, target => {
        const identity = snapshot(target);
        const admin = resolveActor(adminSlot, adminIdentity);
        if (!admin) return;
        const slot = admin.slot;
        if (!validSteamId(identity.steamId)) {
          Clients.fromSlot(slot)?.chat(Translations.translate(slot, "Cannot Ban Bot", identity.name)); return;
        }
        const menu = new Menu(Translations.translate(slot, "Ban Menu Title", identity.name));
        menu.style = MenuStyle.Center; menu.freezePlayer = true;
        for (const minutes of [0, 5, 30, 60]) menu.addItem(String(minutes), minutes === 0
          ? Translations.translate(slot, "Ban Menu Permanent") : Translations.translate(slot, "Ban Menu Minutes", minutes));
        menu.onSelect(event => {
          if (!resolveActor(adminSlot, adminIdentity)) return;
          const minutes = Number(event.info);
          if (![0, 5, 30, 60].includes(minutes)) return;
          const outcome = recordBan({ steamId: identity.steamId, minutes, reason: BAN_REASON_BY_ADMIN,
            source: "menu", actorSteamId: adminIdentity.steamId }, identity);
          // Re-resolve the admin too: a listener can disconnect or replace either participant.
          const currentAdmin = resolveActor(adminSlot, adminIdentity);
          if (!outcome.recorded && currentAdmin) Clients.fromSlot(currentAdmin.slot)?.chat(
            Translations.translate(currentAdmin.slot, failurePhrase(outcome)));
        });
        menu.display(slot, 30);
      });
    } });
}

// Reconnect enforcement is a separate query/kick path; it never records or notifies.
export function OnClientConnected(c: Client): void {
  if (c.isBot) return;                                   // bots have steamId "0" — never banned
  const b = Bans.get(c.steamId);
  if (!b) return;
  const now = Date.now() / 1000;
  if (b.until !== 0 && b.until <= now) return;           // expired — let them in
  c.kickWithReason(banMessage(c.slot, b.reason, b.until));
}
