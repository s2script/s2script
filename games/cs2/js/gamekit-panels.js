// @s2script/cs2 — GameHud panels: the pop-up vote and the drawer, on s2script_gamepanels.xml.
//
// INPUT RULE. Everything stays clickable. The one exception is a vote the player did not start
// that pops up mid-round: it must not grab the cursor or interrupt aiming, so voters can also type
// the option's number in chat, and Tab (HudInput) hands them the cursor only if they want to click.
// A drawer is opened by the player, so it takes the cursor at once and has a close button.
//
// CLAIMS. One pop-up vote runs at a time server-wide (`gamekit:vote`). A drawer is per player
// (`gamekit:drawer:<slot>`), so two plugins cannot open one over the other for the same player.
// Both are host-side claims, freed on plugin unload without the plugin's cleanup running.
//
// ES5 IIFE, concatenated after gamekit.js, whose helpers it reads from __s2pkg_gamekit_util.
(function () {
  var U = globalThis.__s2pkg_gamekit_util;
  var prevUi = globalThis.__s2pkg_game_ctx && globalThis.__s2pkg_game_ctx.ui;
  if (!U || typeof prevUi !== "function") return;

  var GK = U.GK;
  var SPEC = GK.spec;
  var HIDE = SPEC.hideClass;
  var OPTIONS = SPEC.vote.options;
  var TABS = SPEC.drawer.tabs;
  var ROWS = SPEC.drawer.rows;
  var VOTE_SECONDS = 20;
  var HANDLED = (globalThis.HookResult && globalThis.HookResult.Handled) || 2;
  var SEL = "gk-sel";
  var DISABLED = "s2-btn-disabled";

  function hudInput() {
    return (globalThis.__s2pkg_hudinput && globalThis.__s2pkg_hudinput.HudInput) ||
      (globalThis.__s2pkg_cs2 && globalThis.__s2pkg_cs2.HudInput) || null;
  }

  function ok(err) { return err === null || err === undefined; }

  function optionOf(raw) {
    if (raw && typeof raw === "object") {
      return { label: raw.label == null ? "" : String(raw.label), tone: raw.tone };
    }
    return { label: raw == null ? "" : String(raw), tone: undefined };
  }

  function makePanels(base) {
    var layout = null;
    var applied = {};
    var clicksBound = false;
    var vote = null;          // the live vote state, or null
    var drawers = {};         // slot -> drawer state
    var chatSub = false;
    var routes = {};          // button id -> fn(player)
    var chatHandlers = [];    // fn(slot, text) -> HANDLED | undefined
    var forgetHooks = [];
    var resetHooks = [];

    function P() {
      if (!layout) {
        layout = base.hud(GK.panels);
        bindClicks();
      }
      return layout;
    }

    function route(id, fn) {
      if (routes[id]) throw new Error("gamekit: button " + id + " already routed");
      routes[id] = fn;
    }

    function setText(slot, id, v) { return P().setText(slot, id, U.plainText(v)); }
    function setClass(slot, id, cls, on) { return P().setClass(slot, id, cls, on); }
    function show(slot, id, opts) { return P().show(slot, id, opts); }
    function hide(slot, id) { return P().hide(slot, id); }
    function setRaw(slot, id, v) {
      var h = P();
      return typeof h._setRaw === "function" ? h._setRaw(slot, id, v) : h.set(slot, id, v);
    }
    function cursor(slot, root, on) {
      var h = P();
      if (typeof h._cursorForPanel === "function") return h._cursorForPanel(slot, root, !!on);
      return null;
    }
    function swap(slot, id, family, cls) {
      var bySlot = applied[slot] || (applied[slot] = {});
      var byId = bySlot[id] || (bySlot[id] = {});
      var prev = byId[family];
      if (prev && prev !== cls) setClass(slot, id, prev, false);
      byId[family] = cls || null;
      return cls ? setClass(slot, id, cls, true) : null;
    }

    // ── pop-up vote ────────────────────────────────────────────────────────────────────────
    function counts(st) {
      var out = [];
      for (var i = 0; i < st.options.length; i++) out.push(0);
      for (var s in st.votes) {
        if (Object.prototype.hasOwnProperty.call(st.votes, s)) out[st.votes[s]]++;
      }
      return out;
    }

    function totalVotes(c) { var t = 0; for (var i = 0; i < c.length; i++) t += c[i]; return t; }

    function voteFooter(st, slot) {
      if (!st.voters[slot]) return "Watching";
      var mine = st.votes[slot];
      var range = st.options.length > 1 ? "1-" + st.options.length : "1";
      if (mine === undefined) return "Type " + range + " in chat, or press Tab to click";
      return "Voted " + U.plainText(st.options[mine].label) + ". Type another number to change it";
    }

    function paintVote(st, slot) {
      var c = counts(st);
      var total = totalVotes(c);
      var mine = st.votes[slot];
      var reveal = st.voters[slot] ? mine !== undefined : true;
      var first = null;
      function note(err) { if (!ok(err) && first === null) first = err; }
      note(setText(slot, "gk_vote_title", st.title));
      note(setText(slot, "gk_vote_right", st.remaining + "s"));
      note(P().setMeter(slot, "voteTimer", st.seconds > 0 ? (st.remaining / st.seconds) * 100 : 0));
      note(setText(slot, "gk_vote_q", st.question));
      for (var i = 0; i < OPTIONS; i++) {
        var row = "gk_vo" + i;
        var opt = st.options[i];
        if (!opt) { note(hide(slot, row)); continue; }
        note(setText(slot, row + "_n", String(i + 1)));
        note(swap(slot, row + "_t", "text", "gk-t-" + U.pickTone(opt.tone, opt.label, "default")));
        note(setText(slot, row + "_t", opt.label));
        note(setText(slot, row + "_c", reveal ? String(c[i]) : ""));
        note(P().setMeter(slot, "vo" + i, reveal && total > 0 ? (c[i] / total) * 100 : 0));
        note(setClass(slot, row, SEL, mine === i));
        note(P().setDisabled(slot, row, !st.voters[slot]));
        note(show(slot, row));
      }
      note(setText(slot, "gk_vote_ft", voteFooter(st, slot)));
      note(show(slot, "gk_vote"));
      return first;
    }

    function voteChatLines(st) {
      var lines = ["{orange}" + U.plainText(st.title) + "{grey}: " + st.question];
      for (var i = 0; i < st.options.length; i++) lines.push("{yellow}" + (i + 1) + "{grey}. " + st.options[i].label);
      lines.push("{blue}Type the number in chat to vote.");
      return lines;
    }

    function armVoter(st, slot) {
      var HI = hudInput();
      if (!HI || typeof HI.arm !== "function") return;
      HI.arm(slot, { onActivate: function () { if (vote === st) cursor(slot, "gk_vote", true); } });
    }

    function releaseVoter(slot) {
      var HI = hudInput();
      if (HI && typeof HI.disarm === "function") HI.disarm(slot);
      cursor(slot, "gk_vote", false);
    }

    function repaintVote(st) {
      for (var s in st.viewers) {
        if (!Object.prototype.hasOwnProperty.call(st.viewers, s)) continue;
        var slot = Number(s);
        var err = paintVote(st, slot);
        if (!ok(err) && !st.toldInChat[slot] && !/stale client/i.test(String(err))) {
          st.toldInChat[slot] = true;
          var lines = voteChatLines(st);
          for (var i = 0; i < lines.length; i++) U.chat(slot, lines[i]);
        }
      }
    }

    function finishVote(st, cancelled) {
      if (!st.live) return;
      st.live = false;
      if (vote === st) vote = null;
      for (var s in st.viewers) {
        if (!Object.prototype.hasOwnProperty.call(st.viewers, s)) continue;
        hide(Number(s), "gk_vote");
        if (st.voters[s]) releaseVoter(Number(s));
      }
      U.hostRelease("gamekit:vote");
      if (cancelled || typeof st.onEnd !== "function") return;
      var c = counts(st);
      var best = -1;
      var winner = null;
      for (var i = 0; i < c.length; i++) {
        if (c[i] > best) { best = c[i]; winner = i; }
        else if (c[i] === best) winner = null;
      }
      if (best <= 0) winner = null;
      var votes = {};
      for (var k in st.votes) if (Object.prototype.hasOwnProperty.call(st.votes, k)) votes[k] = st.votes[k];
      try { st.onEnd({ counts: c, winner: winner, votes: votes }); }
      catch (e) { U.log("vote onEnd threw: " + ((e && e.stack) || e)); }
    }

    function cast(slot, index) {
      var st = vote;
      if (!st || !st.live || !st.voters[slot]) return false;
      if (typeof index !== "number" || index < 0 || index >= st.options.length) return false;
      st.votes[slot] = index;
      releaseVoter(slot);
      if (typeof st.onVote === "function") {
        try { st.onVote(slot, index); } catch (e) { U.log("vote onVote threw: " + ((e && e.stack) || e)); }
      }
      if (st.live) repaintVote(st);
      return true;
    }

    function tick(st) {
      U.afterSeconds(1, function () {
        if (!st.live) return;
        st.remaining = Math.max(0, st.remaining - 1);
        if (st.remaining <= 0) { finishVote(st, false); return; }
        repaintVote(st);
        tick(st);
      });
    }

    function ensureChat() {
      if (chatSub || typeof globalThis.__s2_chat_on_message !== "function") return;
      chatSub = true;
      globalThis.__s2_chat_on_message(function (slot, text) {
        for (var i = 0; i < chatHandlers.length; i++) {
          var r;
          try { r = chatHandlers[i](slot, text); } catch (e) { U.log("chat handler threw: " + ((e && e.stack) || e)); }
          if (r === HANDLED) return HANDLED;
        }
        return undefined;
      });
    }

    chatHandlers.push(function (slot, text) {
      var st = vote;
      if (!st || !st.live || !st.voters[slot]) return undefined;
      var t = String(text).trim();
      if (!/^[1-9]$/.test(t)) return undefined;
      var index = parseInt(t, 10) - 1;
      if (index >= st.options.length) return undefined;
      cast(slot, index);
      return HANDLED;
    });

    function startVote(spec) {
      spec = spec || {};
      if (spec.question == null) throw new Error("GameHud.vote: spec.question is required");
      var raw = Array.isArray(spec.options) ? spec.options : [];
      if (raw.length < 2 || raw.length > OPTIONS) {
        throw new Error("GameHud.vote: between 2 and " + OPTIONS + " options are required (got " + raw.length + ")");
      }
      if (vote && vote.live) return null;
      if (U.hostClaim("gamekit:vote") < 0) return null;
      var seconds = spec.seconds > 0 ? Math.floor(Number(spec.seconds)) : VOTE_SECONDS;
      var voterSlots = U.slotsFor(spec.voters);
      var viewerSlots = spec.viewers === undefined ? voterSlots : U.slotsFor(spec.viewers);
      var st = {
        live: true,
        title: spec.title == null ? "Vote" : String(spec.title),
        question: String(spec.question),
        options: raw.map(optionOf),
        seconds: seconds,
        remaining: seconds,
        voters: {}, viewers: {}, votes: {}, toldInChat: {},
        onVote: spec.onVote, onEnd: spec.onEnd
      };
      var i;
      for (i = 0; i < voterSlots.length; i++) { st.voters[voterSlots[i]] = true; st.viewers[voterSlots[i]] = true; }
      for (i = 0; i < viewerSlots.length; i++) st.viewers[viewerSlots[i]] = true;
      vote = st;
      P();
      ensureChat();
      for (i = 0; i < voterSlots.length; i++) armVoter(st, voterSlots[i]);
      repaintVote(st);
      tick(st);
      return {
        isOpen: function () { return st.live; },
        counts: function () { return counts(st); },
        cast: function (player, index) { return st.live && vote === st ? cast(U.slotOf(player), index) : false; },
        end: function () { finishVote(st, false); },
        cancel: function () { finishVote(st, true); }
      };
    }

    // ── drawer ─────────────────────────────────────────────────────────────────────────────
    function drawerRows(st) {
      var tab = st.tabs[st.tab];
      var rows = tab && Array.isArray(tab.rows) ? tab.rows : st.rows;
      return rows.slice(0, ROWS);
    }

    function paintDrawer(st) {
      var slot = st.slot;
      var first = null;
      function note(err) { if (!ok(err) && first === null) first = err; }
      note(setText(slot, "gk_dr_title", st.title));
      note(setText(slot, "gk_dr_right", st.right));
      note(setText(slot, "gk_dr_close_t", "Close"));
      for (var t = 0; t < TABS; t++) {
        var tabId = "gk_dr_tab" + t;
        var tab = st.tabs[t];
        if (!tab) { note(hide(slot, tabId)); continue; }
        note(setText(slot, tabId + "_t", tab.label));
        note(setClass(slot, tabId, SEL, t === st.tab));
        note(show(slot, tabId));
      }
      var rows = drawerRows(st);
      for (var i = 0; i < ROWS; i++) {
        var r = "gk_dr_r" + i;
        var row = rows[i];
        if (!row) { note(hide(slot, r)); continue; }
        note(setRaw(slot, r + "_img", row.image || ""));
        note(setClass(slot, r + "_img", HIDE, !row.image));
        note(swap(slot, r + "_t", "text", "gk-t-" + U.pickTone(row.tone, row.title, "default")));
        note(setText(slot, r + "_t", row.title));
        note(setText(slot, r + "_s", row.sub == null ? "" : row.sub));
        note(setClass(slot, r + "_s", HIDE, row.sub == null || row.sub === ""));
        note(setText(slot, r + "_m", row.meta == null ? "" : row.meta));
        note(setClass(slot, r, SEL, !!row.selected));
        note(setClass(slot, r, DISABLED, !!row.disabled));
        note(P().setDisabled(slot, r, !!row.disabled));
        note(show(slot, r));
      }
      note(setText(slot, "gk_dr_ft", st.footer));
      note(setClass(slot, "gk_dr_ft", HIDE, !st.footer));
      note(show(slot, "gk_drawer", { cursor: true }));
      return first;
    }

    function drawerChat(st) {
      U.chat(st.slot, "{orange}" + st.title + (st.right ? " {grey}" + st.right : ""));
      var rows = drawerRows(st);
      for (var i = 0; i < rows.length; i++) {
        U.chat(st.slot, "{grey}- " + rows[i].title + (rows[i].meta ? " {yellow}" + rows[i].meta : ""));
      }
    }

    function applySpec(st, spec) {
      st.title = spec.title == null ? "" : String(spec.title);
      st.right = spec.right == null ? "" : String(spec.right);
      st.footer = spec.footer == null ? "" : String(spec.footer);
      st.tabs = Array.isArray(spec.tabs) ? spec.tabs.slice(0, TABS).map(function (t) {
        return { id: t.id == null ? t.label : t.id, label: t.label == null ? String(t.id) : String(t.label), rows: t.rows };
      }) : [];
      st.rows = Array.isArray(spec.rows) ? spec.rows : [];
      if (spec.tab !== undefined) {
        for (var i = 0; i < st.tabs.length; i++) if (st.tabs[i].id === spec.tab) st.tab = i;
      }
      if (st.tab >= st.tabs.length) st.tab = 0;
      st.onSelect = spec.onSelect;
      st.onTab = spec.onTab;
      st.onClose = spec.onClose;
    }

    function closeDrawer(st, fromPlayer) {
      if (!st.live) return;
      st.live = false;
      if (drawers[st.slot] === st) delete drawers[st.slot];
      hide(st.slot, "gk_drawer");
      U.hostRelease("gamekit:drawer:" + st.slot);
      if (typeof st.onClose === "function") {
        try { st.onClose(fromPlayer); } catch (e) { U.log("drawer onClose threw: " + ((e && e.stack) || e)); }
      }
    }

    function openDrawer(player, spec) {
      var slot = U.slotOf(player);
      if (slot < 0) throw new Error("GameHud.drawer: a player or slot is required");
      spec = spec || {};
      var existing = drawers[slot];
      if (existing && existing.live) { applySpec(existing, spec); repaint(existing); return existing.handle; }
      if (U.hostClaim("gamekit:drawer:" + slot) < 0) return null;
      var st = { slot: slot, live: true, tab: 0 };
      applySpec(st, spec);
      drawers[slot] = st;
      P();
      st.handle = {
        isOpen: function () { return st.live; },
        update: function (next) { if (st.live) { applySpec(st, next || {}); repaint(st); } },
        setTab: function (id) {
          if (!st.live) return;
          for (var i = 0; i < st.tabs.length; i++) if (st.tabs[i].id === id) st.tab = i;
          repaint(st);
        },
        close: function () { closeDrawer(st, false); }
      };
      repaint(st);
      return st.handle;
    }

    function repaint(st) {
      var err = paintDrawer(st);
      if (!ok(err) && !/stale client/i.test(String(err))) drawerChat(st);
    }

    function bindClicks() {
      if (clicksBound) return;
      clicksBound = true;
      var ids = GK.panels.buttons || [];
      for (var i = 0; i < ids.length; i++) {
        (function (id) {
          layout.onClick(id, function (player) {
            var fn = routes[id];
            if (!fn || !player) return;
            try { fn(player); } catch (e) { U.log("click " + id + " threw: " + ((e && e.stack) || e)); }
          });
        })(ids[i]);
      }
    }

    for (var oi = 0; oi < OPTIONS; oi++) {
      (function (index) {
        route("gk_vo" + index, function (player) { cast(player.slot, index); });
      })(oi);
    }
    for (var ti = 0; ti < TABS; ti++) {
      (function (index) {
        route("gk_dr_tab" + index, function (player) {
          var st = drawers[player.slot];
          if (!st || !st.live || !st.tabs[index]) return;
          st.tab = index;
          if (typeof st.onTab === "function") {
            try { st.onTab(st.tabs[index].id, st.handle); } catch (e) { U.log("drawer onTab threw: " + ((e && e.stack) || e)); }
          }
          if (st.live) repaint(st);
        });
      })(ti);
    }
    for (var ri = 0; ri < ROWS; ri++) {
      (function (index) {
        route("gk_dr_r" + index, function (player) {
          var st = drawers[player.slot];
          if (!st || !st.live) return;
          var row = drawerRows(st)[index];
          if (!row || row.disabled || typeof st.onSelect !== "function") return;
          try { st.onSelect(row, st.handle); } catch (e) { U.log("drawer onSelect threw: " + ((e && e.stack) || e)); }
        });
      })(ri);
    }
    route("gk_dr_close", function (player) {
      var st = drawers[player.slot];
      if (st) closeDrawer(st, true);
    });

    function forgetSlot(slot) {
      delete applied[slot];
      var st = drawers[slot];
      if (st) {
        st.live = false;
        delete drawers[slot];
        U.hostRelease("gamekit:drawer:" + slot);
      }
      if (vote) { delete vote.voters[slot]; delete vote.viewers[slot]; delete vote.votes[slot]; }
      for (var i = 0; i < forgetHooks.length; i++) forgetHooks[i](slot);
    }

    function resetForMap() {
      applied = {};
      if (vote) finishVote(vote, true);
      for (var s in drawers) if (Object.prototype.hasOwnProperty.call(drawers, s)) closeDrawer(drawers[s], false);
      for (var i = 0; i < resetHooks.length; i++) resetHooks[i]();
    }

    return {
      vote: startVote,
      drawer: openDrawer,
      _forgetSlot: forgetSlot,
      // The shared panels handle for the other panel surfaces (gamekit-sheet.js). One layout
      // entity per plugin context, one click route per button.
      _kit: {
        P: P, ok: ok, setText: setText, setClass: setClass, show: show, hide: hide, setRaw: setRaw,
        cursor: cursor, swap: swap, route: route, HANDLED: HANDLED,
        onChat: function (fn) { chatHandlers.push(fn); ensureChat(); },
        onForget: function (fn) { forgetHooks.push(fn); },
        onReset: function (fn) { resetHooks.push(fn); }
      },
      _resetForMap: resetForMap,
      _castFromChat: function (slot, text) {
        var st = vote;
        if (!st) return undefined;
        var t = String(text).trim();
        if (!/^[1-9]$/.test(t)) return undefined;
        return cast(slot, parseInt(t, 10) - 1) ? HANDLED : undefined;
      }
    };
  }

  var livePanels = null;

  globalThis.__s2pkg_game_ctx = Object.assign({}, globalThis.__s2pkg_game_ctx, {
    ui: function (reg, viaId) {
      var base = prevUi(reg, viaId);
      if (base && typeof base.hud === "function" && !base.gamePanels) {
        var panels = makePanels(base);
        base.gamePanels = panels;
        if (!livePanels) livePanels = panels;
        reg(viaId(function () {
          var Clients = U.clientsApi();
          if (Clients && typeof Clients.onDisconnect === "function") {
            return Clients.onDisconnect(function (c) { if (c && typeof c.slot === "number") panels._forgetSlot(c.slot); });
          }
        }));
        reg(viaId(function () {
          var Server = globalThis.__s2pkg_server && globalThis.__s2pkg_server.Server;
          if (Server && typeof Server.onMapStart === "function") return Server.onMapStart(function () { panels._resetForMap(); });
        }));
      }
      return base;
    }
  });

  function panelsOrThrow(name) {
    if (!livePanels) throw new Error("s2script: GameHud." + name + " requires plugin context; use it in OnPluginStart or a later callback");
    return livePanels;
  }

  var cs2 = globalThis.__s2pkg_cs2;
  if (cs2 && cs2.GameHud) {
    var merged = {};
    for (var k in cs2.GameHud) if (Object.prototype.hasOwnProperty.call(cs2.GameHud, k)) merged[k] = cs2.GameHud[k];
    merged.vote = function (spec) { return panelsOrThrow("vote").vote(spec); };
    merged.drawer = function (player, spec) { return panelsOrThrow("drawer").drawer(player, spec); };
    cs2.GameHud = Object.freeze(merged);
  }
})();
