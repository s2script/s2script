// @s2script/cs2 — the CS2 vote rail presenter. This is a dedicated layout, not a hudkit modal.
(function () {
  var cs2 = globalThis.__s2pkg_cs2 || {};
  var CustomHudLayout = cs2.CustomHudLayout;
  var HudInput = (globalThis.__s2pkg_hudinput && globalThis.__s2pkg_hudinput.HudInput) ||
    cs2.HudInput;
  var votes = globalThis.__s2pkg_votes;
  var frame = globalThis.__s2pkg_frame;

  if (!CustomHudLayout || typeof CustomHudLayout.probe !== "function" ||
      !votes || !votes.Vote || typeof votes.Vote.registerTallyRenderer !== "function") return;

  // Probe first so a missing workshop addon leaves the normal chat vote path untouched.
  var probe;
  try { probe = CustomHudLayout.probe(); } catch (e) { return; }
  if (!probe) return;

  var DESCRIPTOR = {
    addons: ["3790153369"],
    resource: "panorama/layout/custom_game/s2_vote.xml",
    hideClass: "s2-hide",
    text: {
      s2_vote_q: "s2_vote_q",
      s2_vote_sub: "s2_vote_sub"
    },
    buttons: []
  };
  for (var i = 0; i < 9; i++) {
    DESCRIPTOR.text["s2_vote_o" + i + "_t"] = "s2_vote_o" + i + "_t";
    DESCRIPTOR.text["s2_vote_o" + i + "_c"] = "s2_vote_o" + i + "_c";
    DESCRIPTOR.buttons.push("s2_vote_o" + i);
  }

  var layout;
  try { layout = CustomHudLayout.create(DESCRIPTOR); } catch (e2) { return; }
  if (!layout || typeof layout.forSlot !== "function") return;

  var ROOT = "s2_vote";
  var COUNT_HIDE = "s2-vote-count-hide";
  var PICKED = "s2-vote-picked";
  var tallies = {}; // slot -> last VoteTally
  var waiting = {}; // slot -> waiting for Tab/pick
  var pollSub = null;

  function viewFor(slot) { return layout.forSlot(slot); }

  function optionAt(tally, index) {
    var option = tally.options && tally.options[index];
    if (typeof option === "string") return { label: option, count: tally.counts && tally.counts[index] || 0 };
    return {
      label: option && option.label != null ? option.label : "",
      count: option && option.count != null ? option.count :
        (tally.counts && tally.counts[index] != null ? tally.counts[index] : 0)
    };
  }

  function choiceOf(tally) { return tally.choice == null ? null : tally.choice; }

  function cursor(slot, on) {
    var view = viewFor(slot);
    if (view && typeof view.cursor === "function") view.cursor(on);
    else if (on && view && typeof view.show === "function") view.show(ROOT, { cursor: true });
  }

  function maybeActivate(slot) {
    if (!waiting[slot] || !HudInput || typeof HudInput.consumeActive !== "function") return;
    if (!HudInput.consumeActive(slot)) return;
    cursor(slot, true);
  }

  function ensurePoll() {
    if (pollSub || !frame || !frame.OnGameFrame ||
        typeof frame.OnGameFrame.subscribe !== "function") return;
    pollSub = frame.OnGameFrame.subscribe(function () {
      for (var slot in waiting) {
        if (Object.prototype.hasOwnProperty.call(waiting, slot)) maybeActivate(slot | 0);
      }
    });
  }

  function stopPollIfIdle() {
    for (var slot in waiting) {
      if (Object.prototype.hasOwnProperty.call(waiting, slot)) return;
    }
    if (pollSub && typeof pollSub.dispose === "function") pollSub.dispose();
    pollSub = null;
  }

  function paint(slot, tally) {
    var view = viewFor(slot);
    var choice = choiceOf(tally);
    var options = Array.isArray(tally.options) ? tally.options : [];

    view.setText("s2_vote_q", tally.question == null ? "" : tally.question);
    view.setText("s2_vote_sub", choice === null ? "Tab, or type 1–N" : "Voting…");

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
      view.setText(countId, option.count);
      view.setClass(buttonId, PICKED, choice !== null && i === choice);
      view.setClass(countId, COUNT_HIDE, choice === null);
    }

    view.show(ROOT);
    if (choice === null) {
      waiting[slot] = true;
      if (HudInput && typeof HudInput.arm === "function") HudInput.arm(slot);
      ensurePoll();
      maybeActivate(slot);
    } else {
      delete waiting[slot];
      if (HudInput && typeof HudInput.disarm === "function") HudInput.disarm(slot);
      cursor(slot, false);
      stopPollIfIdle();
    }
  }

  function hide(slot) {
    delete tallies[slot];
    delete waiting[slot];
    viewFor(slot).hide(ROOT);
    cursor(slot, false);
    if (HudInput && typeof HudInput.disarm === "function") HudInput.disarm(slot);
    stopPollIfIdle();
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
    hide: hide,
    clear: hide
  };
  votes.Vote.registerTallyRenderer(renderer);
  globalThis.__s2pkg_voterail = {
    descriptor: DESCRIPTOR,
    layout: layout,
    renderer: renderer,
    tick: maybeActivate
  };
})();
