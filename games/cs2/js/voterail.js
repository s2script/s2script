// @s2script/cs2 — Vote rail presenter. Drives s2_vote* on s2script_lib.xml (hudkit.layout).
(function () {
  var cs2 = globalThis.__s2pkg_cs2 || {};
  var hudkit = cs2.hudkit;
  var layout = hudkit && hudkit.layout;
  var HudInput = (globalThis.__s2pkg_hudinput && globalThis.__s2pkg_hudinput.HudInput) || cs2.HudInput;
  var Vote = globalThis.__s2pkg_votes && globalThis.__s2pkg_votes.Vote;
  var Clients = globalThis.__s2pkg_clients && globalThis.__s2pkg_clients.Clients;

  if (!layout || typeof layout.forSlot !== "function" || typeof layout.onClick !== "function" ||
      !Vote || typeof Vote.registerTallyRenderer !== "function") return;

  var ROOT = "s2_vote";
  var COUNT_HIDE = "s2-vote-count-hide";
  var PICKED = "s2-vote-picked";
  var tallies = {};
  var waiting = {};

  function viewFor(slot) { return layout.forSlot(slot); }

  function optionAt(tally, index) {
    var option = tally.options && tally.options[index];
    if (typeof option === "string") {
      return { label: option, count: (tally.counts && tally.counts[index]) || 0 };
    }
    return {
      label: option && option.label != null ? String(option.label) : "",
      count: option && option.count != null ? option.count : ((tally.counts && tally.counts[index]) || 0)
    };
  }

  function choiceOf(tally) {
    return tally.choice == null ? null : tally.choice;
  }

  function cursor(slot, on) {
    var view = viewFor(slot);
    if (view && typeof view.cursor === "function") view.cursor(!!on);
  }

  function armWaiting(slot) {
    if (waiting[slot]) return;
    waiting[slot] = true;
    if (HudInput && typeof HudInput.arm === "function") {
      HudInput.arm(slot, {
        onActivate: function () { cursor(slot, true); }
      });
    }
  }

  function disarmWaiting(slot) {
    if (waiting[slot]) delete waiting[slot];
    if (HudInput && typeof HudInput.disarm === "function") HudInput.disarm(slot);
    cursor(slot, false);
  }

  function paint(slot, tally) {
    var view = viewFor(slot);
    var choice = choiceOf(tally);
    var options = Array.isArray(tally.options) ? tally.options : [];

    view.setText("s2_vote_q", tally.question == null ? "" : String(tally.question));
    view.setText("s2_vote_sub", choice === null
      ? "Tab, or type 1–N"
      : ((tally.secondsLeft == null ? "" : tally.secondsLeft + "s") +
         (tally.total == null ? "" : " · " + tally.total + " voted")));

    for (var i = 0; i < 9; i++) {
      var buttonId = "s2_vote_o" + i;
      var titleId = buttonId + "_t";
      var countId = buttonId + "_c";
      if (i >= options.length) {
        view.hide(buttonId);
        view.setClass(buttonId, PICKED, false);
        view.setClass(countId, COUNT_HIDE, true);
        continue;
      }
      var option = optionAt(tally, i);
      view.show(buttonId);
      view.setText(titleId, option.label);
      view.setText(countId, String(option.count));
      view.setClass(buttonId, PICKED, choice !== null && i === choice);
      view.setClass(countId, COUNT_HIDE, choice === null);
    }

    view.show(ROOT);
    if (choice === null) armWaiting(slot);
    else disarmWaiting(slot);
  }

  function hide(slot) {
    delete tallies[slot];
    viewFor(slot).hide(ROOT);
    disarmWaiting(slot);
  }

  for (var clickIndex = 0; clickIndex < 9; clickIndex++) {
    (function (index) {
      layout.onClick("s2_vote_o" + index, function (player) {
        var slot = player && player.slot;
        var tally = tallies[slot];
        if (slot == null || !tally || choiceOf(tally) !== null) return;
        var cast = globalThis.__s2_vote_cast;
        if (typeof cast === "function") cast(slot, index);
      });
    })(clickIndex);
  }

  var renderer = {
    show: function (slot, tally) {
      tallies[slot] = tally;
      paint(slot, tally);
    },
    clear: hide,
    hide: hide
  };
  Vote.registerTallyRenderer(renderer);

  if (Clients && typeof Clients.onDisconnect === "function") {
    Clients.onDisconnect(function (c) {
      if (c && typeof c.slot === "number" && tallies[c.slot]) hide(c.slot);
    });
  }

  globalThis.__s2pkg_voterail = { layout: layout, renderer: renderer };
})();
