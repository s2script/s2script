// Show-activity decision (SourceMod FormatActivitySource port). PURE — the
// caller resolves names/flags; this only decides {show, name} from the bitmask.
// Dual-mode: sets globalThis.__s2_activity at runtime (V8, concatenated into the
// CS2 bundle) AND module.exports under node:test.
(function () {
  var SHOW_ACTIVITY_DEFAULT = 13; // 1|4|8 (kNonAdmins | kAdmins | kAdminsNames)
  var kNonAdmins = 1, kNonAdminsNames = 2, kAdmins = 4, kAdminsNames = 8, kRootNames = 16;

  function computeActivitySource(flags, actorLabel, actorReal, recipientIsAdmin, recipientIsRoot, recipientIsActor) {
    var show = false, useReal = false;
    if (!recipientIsAdmin) {
      if (flags & (kNonAdmins | kNonAdminsNames)) show = true;
      if ((flags & kNonAdminsNames) || recipientIsActor) useReal = true;
    } else {
      if ((flags & (kAdmins | kAdminsNames)) || ((flags & kRootNames) && recipientIsRoot)) show = true;
      if ((flags & kAdminsNames) || ((flags & kRootNames) && recipientIsRoot) || recipientIsActor) useReal = true;
    }
    // name is the generic label whenever show is false (SM parity: the original set the real
    // name only inside the show branch, so a caller must read name only when show === true).
    return { show: show, name: (show && useReal) ? actorReal : actorLabel };
  }

  // sm_show_activity's string value -> the bitmask. Anything unset/non-numeric/out of range is the
  // SourceMod default (13), so a missing cvar behaves exactly like a stock SM server.
  function parseShowActivity(value) {
    if (typeof value !== "string" || !/^\s*\d+\s*$/.test(value)) return SHOW_ACTIVITY_DEFAULT;
    var n = parseInt(value, 10);
    return (n >= 0 && n <= 31) ? n : SHOW_ACTIVITY_DEFAULT;
  }

  // ShowActivity2's routing decision (SourceMod core ShowActivity2 port). PURE.
  //   actor:      { slot, label, real, replySource }  (slot < 0 = server console)
  //   recipients: [{ slot, isAdmin, isRoot }]          (connected humans only)
  // Returns { actorConsole, chat: [{ slot, name }] }:
  //   - actorConsole: the actor gets ONE copy on their reply channel because they ran it from a
  //     console (the server console, or their own developer console) — SM prints it there.
  //   - chat: every recipient the sm_show_activity flags allow, under the name they may see. An actor
  //     who used chat (a !trigger or a menu) always sees their own line, under their real name; an
  //     actor who already got the console copy is not sent a second one in chat.
  function planActivity(flags, actor, recipients) {
    var actorConsole = actor.slot < 0 || actor.replySource !== "chat";
    var chat = [];
    for (var i = 0; i < recipients.length; i++) {
      var r = recipients[i];
      if (r.slot === actor.slot) {
        if (!actorConsole) chat.push({ slot: r.slot, name: actor.real });
        continue;
      }
      var src = computeActivitySource(flags, actor.label, actor.real, !!r.isAdmin, !!r.isRoot, false);
      if (src.show) chat.push({ slot: r.slot, name: src.name });
    }
    return { actorConsole: actorConsole, chat: chat };
  }

  // SourceMod's %L: "Name<userid><steamid><>" — the identity LogAction lines carry, so a log line
  // still names the right person after a rename or a reconnect. The console is "Console<0><Console><Console>".
  function describeIdentity(id) {
    if (!id) return "Console<0><Console><Console>";
    var name = String(id.name == null ? "" : id.name).replace(/[\x00-\x1F"<>]/g, "");
    return name + "<" + (id.userId | 0) + "><" + (id.isBot ? "BOT" : String(id.steamId || "0")) + "><>";
  }

  var api = {
    computeActivitySource: computeActivitySource, SHOW_ACTIVITY_DEFAULT: SHOW_ACTIVITY_DEFAULT,
    parseShowActivity: parseShowActivity, planActivity: planActivity, describeIdentity: describeIdentity,
  };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  if (typeof globalThis !== "undefined") globalThis.__s2_activity = api;
})();
