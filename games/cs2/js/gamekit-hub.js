// @s2script/cs2 — GameHud hub surfaces: the intro and the spectate hub, on s2script_gamepanels.xml.
//
// Two separate ideas on one panel set. The INTRO is what a player sees on joining: the mode's
// brand, a short menu (play, spectate, rules...) and optionally a profile card. The SPECTATE HUB
// is for a player watching: a smaller menu (pick a team, back to the intro...), a player picker
// and a camera shot list. Both are per player and share one claim (`gamekit:hub:<slot>`), so
// opening one replaces the other.
//
// CAMERA. Shots drive the player's own observer camera, through the same commands a spectator
// types: a follow shot is `spec_mode` (in-eye or chase) plus `spec_player <entity index>`; a fixed
// shot is roaming mode plus `spec_goto x y z pitch yaw`. They are run with `fakeCommand`, so they
// only move players who are observing (dead or spectating). A shot list can advance on a timer.
//
// The intro always takes the cursor. The spectate hub takes it by default; with `cursor: false`
// it leaves the mouse to the camera and Tab hands it over, like the pop-up vote.
//
// ES5 IIFE, concatenated after gamekit-sheet.js; uses the shared panels kit from gamekit-panels.js.
(function () {
  var U = globalThis.__s2pkg_gamekit_util;
  var prevUi = globalThis.__s2pkg_game_ctx && globalThis.__s2pkg_game_ctx.ui;
  if (!U || typeof prevUi !== "function") return;

  var GK = U.GK;
  var SPEC = GK.spec;
  var HIDE = SPEC.hideClass;
  var HUB = SPEC.hub;
  var SEL = "gk-sel";
  var DISABLED = "s2-btn-disabled";
  var MODES = { eye: 2, chase: 3, roaming: 4 };

  function str(v) { return v == null ? "" : String(v); }
  function has(o, k) { return Object.prototype.hasOwnProperty.call(o, k); }
  function guard(what, fn) {
    try { return fn(); } catch (e) { U.log(what + " threw: " + ((e && e.stack) || e)); return undefined; }
  }
  function num(v) { var n = Number(v); return isFinite(n) ? n : 0; }

  /** Run a spectator command as the player. False when there is no client or the engine refused. */
  function asPlayer(slot, cmd) {
    var Clients = U.clientsApi();
    var c = Clients && typeof Clients.fromSlot === "function" ? Clients.fromSlot(slot) : null;
    if (!c || typeof c.fakeCommand !== "function") return false;
    return c.fakeCommand(cmd) !== false;
  }

  function followCommands(target, mode) {
    var m = MODES[mode] || MODES.eye;
    return ["spec_mode " + m, "spec_player " + (target + 1)];
  }

  function fixedCommands(shot) {
    var p = shot.position || {};
    var a = shot.angles || {};
    return ["spec_mode " + MODES.roaming,
      "spec_goto " + num(p.x) + " " + num(p.y) + " " + num(p.z) + " " + num(a.pitch) + " " + num(a.yaw)];
  }

  function makeHub(kit) {
    var hubs = {};   // slot -> state

    function note(acc, err) { if (!kit.ok(err) && acc.first === null) acc.first = err; }

    function paint(st) {
      var acc = { first: null };
      var slot = st.slot;
      var i;
      var id;
      note(acc, kit.setText(slot, "gk_hub_eye", st.eyebrow));
      note(acc, kit.setClass(slot, "gk_hub_eye", HIDE, st.eyebrow === ""));
      note(acc, kit.setText(slot, "gk_hub_title", st.title));
      note(acc, kit.setText(slot, "gk_hub_sub", st.sub));
      note(acc, kit.setClass(slot, "gk_hub_sub", HIDE, st.sub === ""));
      note(acc, kit.setText(slot, "gk_hub_list_title", st.listTitle));
      note(acc, kit.setText(slot, "gk_hub_list_right", st.listRight));
      note(acc, kit.setText(slot, "gk_hub_list_close_t", st.closeLabel));
      note(acc, kit.setClass(slot, "gk_hub_list_close", HIDE, !st.closeable));
      for (i = 0; i < HUB.items; i++) {
        id = "gk_hub_i" + i;
        var it = st.items[i];
        if (!it) { note(acc, kit.hide(slot, id)); continue; }
        note(acc, kit.swap(slot, id + "_t", "text", "gk-t-" + U.pickTone(it.tone, it.label, "default")));
        note(acc, kit.setText(slot, id + "_t", it.label));
        note(acc, kit.setText(slot, id + "_m", it.meta == null ? "" : it.meta));
        note(acc, kit.setClass(slot, id, SEL, !!it.selected));
        note(acc, kit.setClass(slot, id, DISABLED, !!it.disabled));
        note(acc, kit.P().setDisabled(slot, id, !!it.disabled));
        note(acc, kit.show(slot, id));
      }
      note(acc, kit.setText(slot, "gk_hub_ft", st.footer));
      note(acc, kit.setClass(slot, "gk_hub_ft", HIDE, st.footer === ""));
      // Profile card.
      var card = st.card;
      note(acc, kit.setClass(slot, "gk_hub_card", HIDE, !card));
      if (card) {
        note(acc, kit.setText(slot, "gk_hub_card_h", card.heading == null ? "" : card.heading));
        note(acc, kit.setText(slot, "gk_hub_card_n", card.name == null ? "" : card.name));
        note(acc, kit.setText(slot, "gk_hub_card_s", card.sub == null ? "" : card.sub));
        var fields = Array.isArray(card.fields) ? card.fields : [];
        for (i = 0; i < HUB.card; i++) {
          id = "gk_hub_kv" + i;
          var f = fields[i];
          if (!f) { note(acc, kit.hide(slot, id)); continue; }
          note(acc, kit.setText(slot, id + "_k", f.key));
          note(acc, kit.setText(slot, id + "_v", f.value));
          note(acc, kit.show(slot, id));
        }
      }
      // Player picker (spectate only).
      var players = st.players;
      note(acc, kit.setClass(slot, "gk_hub_pick", HIDE, players.length === 0));
      if (players.length > 0) {
        for (i = 0; i < HUB.picker; i++) {
          id = "gk_hub_p" + i;
          var p = players[i];
          if (!p) { note(acc, kit.hide(slot, id)); continue; }
          note(acc, kit.swap(slot, id, "edge", "gk-e-" + U.pickTone(p.tone, p.name, "grey")));
          note(acc, kit.setText(slot, id + "_t", p.name));
          note(acc, kit.setText(slot, id + "_s", p.sub == null ? "" : p.sub));
          note(acc, kit.setClass(slot, id, SEL, p.slot === st.following));
          note(acc, kit.show(slot, id));
        }
      }
      // Shot label.
      var shot = st.shotIndex >= 0 ? st.shots[st.shotIndex] : null;
      var shotTitle = shot ? str(shot.label) : (st.following >= 0 ? st.followLabel : "");
      note(acc, kit.setClass(slot, "gk_hub_shot", HIDE, shotTitle === ""));
      if (shotTitle !== "") {
        note(acc, kit.setText(slot, "gk_hub_shot_t", shotTitle));
        note(acc, kit.setText(slot, "gk_hub_shot_w", shot && shot.where != null ? shot.where : ""));
      }
      note(acc, kit.show(slot, "gk_hub", { cursor: st.cursor || st.tabbed }));
      return acc.first;
    }

    function chatOut(st) {
      U.chat(st.slot, "{orange}" + U.plainText(st.title) + (st.sub ? " {grey}" + st.sub : ""));
      for (var i = 0; i < st.items.length; i++) {
        if (!st.items[i].disabled) U.chat(st.slot, "{grey}- " + st.items[i].label);
      }
    }

    function repaint(st) {
      var err = paint(st);
      if (!kit.ok(err) && !/stale client/i.test(String(err)) && !st.toldInChat) {
        st.toldInChat = true;
        chatOut(st);
      }
    }

    function nameFor(st, target) {
      for (var i = 0; i < st.players.length; i++) if (st.players[i].slot === target) return U.plainText(st.players[i].name);
      return "";
    }

    function runAll(slot, cmds) {
      var okAll = true;
      for (var i = 0; i < cmds.length; i++) if (!asPlayer(slot, cmds[i])) okAll = false;
      return okAll;
    }

    function follow(st, target, mode) {
      if (typeof target !== "number" || target < 0) return false;
      st.following = target;
      st.shotIndex = -1;
      var n = nameFor(st, target);
      st.followLabel = n ? "Following " + n : "Following";
      var done = runAll(st.slot, followCommands(target, mode));
      if (st.live) repaint(st);
      return done;
    }

    function setShot(st, which) {
      var at = -1;
      if (typeof which === "number") at = which;
      else for (var i = 0; i < st.shots.length; i++) if (st.shots[i].id === which) at = i;
      var shot = st.shots[at];
      if (!shot) return false;
      st.shotIndex = at;
      var done;
      if (shot.kind === "follow") {
        st.following = typeof shot.target === "number" ? shot.target : -1;
        done = st.following >= 0 ? runAll(st.slot, followCommands(st.following, shot.mode)) : false;
      } else {
        st.following = -1;
        done = runAll(st.slot, fixedCommands(shot));
      }
      if (typeof st.onShot === "function") guard("hub onShot", function () { st.onShot(shot, st.handle); });
      if (st.live) repaint(st);
      return done;
    }

    // The shot timer runs while `auto` is on: on open with `shotSeconds`, and again after an
    // explicit `shot()` / `nextShot()`. Following a player stops it. A refresh never restarts it,
    // so frequent updates do not hold the countdown back; only a changed `shotSeconds` does.
    function stopShots(st) { st.shotEpoch++; st.shotTimer = false; }
    function scheduleShots(st) {
      var epoch = ++st.shotEpoch;
      st.shotTimer = false;
      if (!st.auto || !(st.shotSeconds > 0) || st.shots.length < 2) return;
      st.shotTimer = true;
      U.afterSeconds(st.shotSeconds, function () {
        if (!st.live || st.shotEpoch !== epoch) return;
        setShot(st, (st.shotIndex + 1) % st.shots.length);
        scheduleShots(st);
      });
    }

    // Tab and the cursor. With `cursor: false` the hub arms Tab through the panels' shared arm
    // stack, so a pop-up vote arming over it hands Tab back when it ends.
    function syncInput(st, hadCursor) {
      var wantArm = st.live && !st.cursor;
      if (wantArm && !st.armed) {
        kit.arm(st.slot, "hub", function () {
          if (hubs[st.slot] === st && st.live) { st.tabbed = true; kit.cursor(st.slot, "gk_hub", true); }
        });
        st.armed = true;
      } else if (!wantArm && st.armed) {
        kit.disarm(st.slot, "hub");
        st.armed = false;
      }
      if (hadCursor && !st.cursor) { st.tabbed = false; kit.cursor(st.slot, "gk_hub", false); }
    }

    function defaults(st, kind) {
      st.kind = kind;
      st.eyebrow = ""; st.title = ""; st.sub = ""; st.listRight = ""; st.footer = "";
      st.listTitle = kind === "intro" ? "Menu" : "Spectating";
      st.closeable = true; st.closeLabel = "Close";
      st.items = []; st.card = null; st.players = []; st.shots = []; st.shotSeconds = 0;
      st.cursor = true;
      st.onSelect = st.onPick = st.onShot = st.onClose = undefined;
    }

    // A spec field left out keeps its current value; switching between intro and spectate starts
    // from the defaults.
    function applySpec(st, spec) {
      var kind = st.kind;
      if (spec.eyebrow !== undefined) st.eyebrow = str(spec.eyebrow);
      if (spec.title !== undefined) st.title = str(spec.title);
      if (spec.sub !== undefined) st.sub = str(spec.sub);
      if (spec.listTitle !== undefined) st.listTitle = str(spec.listTitle);
      if (spec.listRight !== undefined) st.listRight = str(spec.listRight);
      if (spec.footer !== undefined) st.footer = str(spec.footer);
      if (spec.closeable !== undefined) st.closeable = spec.closeable !== false;
      if (spec.closeLabel !== undefined) st.closeLabel = str(spec.closeLabel);
      if (spec.items !== undefined) {
        st.items = Array.isArray(spec.items) ? spec.items.slice(0, HUB.items).map(function (it) {
          return it && typeof it === "object" ? it : { id: it, label: str(it) };
        }) : [];
      }
      if (spec.card !== undefined) st.card = spec.card && typeof spec.card === "object" ? spec.card : null;
      if (kind === "spectate") {
        if (spec.players !== undefined) {
          st.players = Array.isArray(spec.players)
            ? spec.players.slice(0, HUB.picker).filter(function (p) { return p && typeof p.slot === "number"; })
            : [];
        }
        if (spec.shots !== undefined) {
          st.shots = Array.isArray(spec.shots) ? spec.shots.slice() : [];
          if (st.shotIndex >= st.shots.length) st.shotIndex = -1;
        }
        if (spec.shotSeconds !== undefined) st.shotSeconds = spec.shotSeconds > 0 ? Number(spec.shotSeconds) : 0;
        if (spec.cursor !== undefined) st.cursor = spec.cursor !== false;
      }
      if (spec.onSelect !== undefined) st.onSelect = spec.onSelect;
      if (spec.onPick !== undefined) st.onPick = spec.onPick;
      if (spec.onShot !== undefined) st.onShot = spec.onShot;
      if (spec.onClose !== undefined) st.onClose = spec.onClose;
    }

    /** Apply a spec to an open hub and restart the shot timer only if its period changed. */
    function refresh(st, spec) {
      // Only a spec-owned cursor that the spec now turns off is released; a cursor the player took
      // with Tab stays with them through refreshes.
      var hadCursor = st.cursor;
      var period = st.shotSeconds;
      applySpec(st, spec);
      syncInput(st, hadCursor);
      repaint(st);
      // A stopped timer stays stopped unless auto is on (scheduleShots checks it).
      if (st.kind === "spectate" && (st.shotSeconds !== period || !st.shotTimer)) scheduleShots(st);
    }

    function close(st, fromPlayer) {
      if (!st.live) return;
      st.live = false;
      stopShots(st);
      syncInput(st, false);
      if (hubs[st.slot] === st) delete hubs[st.slot];
      kit.hide(st.slot, "gk_hub");
      U.hostRelease("gamekit:hub:" + st.slot);
      if (typeof st.onClose === "function") guard("hub onClose", function () { st.onClose(fromPlayer); });
    }

    function open(kind, player, spec) {
      var name = kind === "intro" ? "intro" : "spectate";
      var slot = U.slotOf(player);
      if (slot < 0) throw new Error("GameHud." + name + ": a player or slot is required");
      spec = spec || {};
      var st = hubs[slot];
      if (st && st.live && st.kind === kind) {
        // Same plugin, same screen: a refresh.
        refresh(st, spec);
        return st.handle;
      }
      var hadCursor = false;
      if (st && st.live) {
        // Same plugin, the other screen: switch in place without a close event.
        hadCursor = st.cursor || st.tabbed;
        stopShots(st);
        st.following = -1; st.shotIndex = -1; st.toldInChat = false;
      } else {
        if (U.hostClaim("gamekit:hub:" + slot) < 0) return null;
        st = { slot: slot, live: true, tabbed: false, armed: false, following: -1, followLabel: "", shotIndex: -1, shotEpoch: 0, shotTimer: false, auto: false, toldInChat: false };
        hubs[slot] = st;
        st.handle = makeHandle(st);
      }
      defaults(st, kind);
      applySpec(st, spec);
      kit.P();
      syncInput(st, hadCursor);
      st.auto = kind === "spectate" && st.shotSeconds > 0;
      if (kind === "spectate" && st.shots.length > 0) {
        var first = 0;
        if (spec.shot !== undefined) {
          for (var i = 0; i < st.shots.length; i++) if (st.shots[i].id === spec.shot || i === spec.shot) first = i;
        }
        setShot(st, first);
      } else {
        repaint(st);
      }
      if (kind === "spectate") scheduleShots(st);
      return st.handle;
    }

    function makeHandle(st) {
      return {
        isOpen: function () { return st.live && hubs[st.slot] === st; },
        kind: function () { return st.kind; },
        update: function (next) { if (st.live) refresh(st, next || {}); },
        follow: function (target, mode) {
          if (!st.live) return false;
          st.auto = false;
          stopShots(st);
          return follow(st, U.slotOf(target), mode);
        },
        shot: function (which) {
          if (!st.live) return false;
          var r = setShot(st, which);
          st.auto = st.shotSeconds > 0;
          scheduleShots(st);
          return r;
        },
        nextShot: function () {
          if (!st.live || st.shots.length === 0) return false;
          var r = setShot(st, (st.shotIndex + 1) % st.shots.length);
          st.auto = st.shotSeconds > 0;
          scheduleShots(st);
          return r;
        },
        close: function () { close(st, false); }
      };
    }

    function live(player) {
      var st = hubs[player.slot];
      return st && st.live ? st : null;
    }

    var i;
    for (i = 0; i < HUB.items; i++) {
      (function (n) {
        kit.route("gk_hub_i" + n, function (player) {
          var st = live(player);
          var it = st && st.items[n];
          if (!it || it.disabled || typeof st.onSelect !== "function") return;
          guard("hub onSelect", function () { st.onSelect(it, st.handle); });
        });
      })(i);
    }
    for (i = 0; i < HUB.picker; i++) {
      (function (n) {
        kit.route("gk_hub_p" + n, function (player) {
          var st = live(player);
          var p = st && st.players[n];
          if (!p) return;
          st.auto = false;
          stopShots(st);
          follow(st, p.slot, p.mode);
          if (typeof st.onPick === "function") guard("hub onPick", function () { st.onPick(p, st.handle); });
        });
      })(i);
    }
    kit.route("gk_hub_list_close", function (player) {
      var st = live(player);
      if (st && st.closeable) close(st, true);
    });

    kit.onForget(function (slot) {
      var st = hubs[slot];
      if (st) close(st, false);
    });
    kit.onReset(function () {
      for (var s in hubs) if (has(hubs, s)) close(hubs[s], false);
    });

    return {
      intro: function (player, spec) { return open("intro", player, spec); },
      spectate: function (player, spec) { return open("spectate", player, spec); }
    };
  }

  var liveHub = null;

  globalThis.__s2pkg_game_ctx = Object.assign({}, globalThis.__s2pkg_game_ctx, {
    ui: function (reg, viaId) {
      var base = prevUi(reg, viaId);
      var panels = base && base.gamePanels;
      if (panels && panels._kit && !panels.hub) {
        panels.hub = makeHub(panels._kit);
        if (!liveHub) liveHub = panels.hub;
      }
      return base;
    }
  });

  function hubOrThrow(name) {
    if (!liveHub) throw new Error("s2script: GameHud." + name + " requires plugin context; use it in OnPluginStart or a later callback");
    return liveHub;
  }

  var cs2 = globalThis.__s2pkg_cs2;
  if (cs2 && cs2.GameHud) {
    var merged = {};
    for (var k in cs2.GameHud) if (has(cs2.GameHud, k)) merged[k] = cs2.GameHud[k];
    merged.intro = function (player, spec) { return hubOrThrow("intro").intro(player, spec); };
    merged.spectate = function (player, spec) { return hubOrThrow("spectate").spectate(player, spec); };
    cs2.GameHud = Object.freeze(merged);
  }
})();
