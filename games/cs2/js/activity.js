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
    return { show: show, name: useReal ? actorReal : actorLabel };
  }

  var api = { computeActivitySource: computeActivitySource, SHOW_ACTIVITY_DEFAULT: SHOW_ACTIVITY_DEFAULT };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  if (typeof globalThis !== "undefined") globalThis.__s2_activity = api;
})();
