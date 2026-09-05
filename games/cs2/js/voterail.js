// @s2script/cs2 — Vote rail presenter. Drives s2_vote* on s2script_lib.xml (hudkit.layout).
(function () {
  var cs2 = globalThis.__s2pkg_cs2 || {};
  var hudkit = cs2.hudkit;
  var HudInput = (globalThis.__s2pkg_hudinput && globalThis.__s2pkg_hudinput.HudInput) || cs2.HudInput;
  var Vote = globalThis.__s2pkg_votes && globalThis.__s2pkg_votes.Vote;
  var Clients = globalThis.__s2pkg_clients && globalThis.__s2pkg_clients.Clients;

  if (!hudkit || typeof hudkit.whenLive !== "function" || !Vote || typeof Vote.registerTallyRenderer !== "function") return;

  var ROOT = "s2_vote";
  var COUNT_HIDE = "s2-vote-count-hide";
  var PICKED = "s2-vote-picked";
  var tallies = {};
  var waiting = {};
  var live = null;   // the ctx-bound layout, set by whenLive below; every paint reads it lazily

  function layoutOf() {
    return live;
  }

  function viewFor(slot) {
    var layout = layoutOf();
    return layout && layout.forSlot(slot);
  }

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
    var layout = layoutOf();
    if (layout) layout._cursorForPanel(slot, ROOT, !!on);
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
    if (!view) return;
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
    var view = viewFor(slot);
    if (view) view.hide(ROOT);
    disarmWaiting(slot);
  }

  var renderer = {
    show: function (slot, tally) {
      tallies[slot] = tally;
      paint(slot, tally);
    },
    clear: hide,
    hide: hide
  };

  // Bind to the plugin's ctx-bound kit (P0-2). This file used to read `hudkit.layout` lazily
  // inside every paint, which pulled the layout out of hostKit()'s stand-in-registrar kit —
  // and bound the nine option clicks lazily at the FIRST paint, at runtime, where a click hook
  // that was not already installed would try to register through a sealed load window. whenLive
  // fires inside __s2_make_ctx with the load window open, so both the layout and the click
  // bindings go through the plugin's ledgered registrar. Registering the tally renderer only
  // here is deliberate: with no live rail, Vote's own "no tally renderer registered — chat-only"
  // warning is a better outcome than a registered renderer that silently paints nothing.
  hudkit.whenLive(function (kit) {
    var layout = kit && kit.layout;
    if (!layout || typeof layout.forSlot !== "function" || typeof layout.onClick !== "function") return;
    live = layout;

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

    Vote.registerTallyRenderer(renderer);

    if (Clients && typeof Clients.onDisconnect === "function") {
      Clients.onDisconnect(function (c) {
        if (c && typeof c.slot === "number" && tallies[c.slot]) hide(c.slot);
      });
    }

    globalThis.__s2pkg_voterail = { layoutOf: layoutOf, renderer: renderer };
  });
})();
