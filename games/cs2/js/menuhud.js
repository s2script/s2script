// @s2script/cs2 — Menu renderer backed by the shared hudkit modal.
//
// Menu is the generic model (open/update/close). This file is the CS2 HUD, cursor, and freeze.
(function () {
  var hudkit = globalThis.__s2pkg_cs2 && globalThis.__s2pkg_cs2.hudkit;
  var menuPkg = globalThis.__s2pkg_menu;
  var Menu = menuPkg && menuPkg.Menu;
  var MenuStyle = menuPkg && menuPkg.MenuStyle;
  var HudInput = (globalThis.__s2pkg_hudinput && globalThis.__s2pkg_hudinput.HudInput) ||
    (globalThis.__s2pkg_cs2 && globalThis.__s2pkg_cs2.HudInput);
  var Clients = globalThis.__s2pkg_clients && globalThis.__s2pkg_clients.Clients;
  var MOVETYPE_NONE = 0;
  var sessions = {};
  var frozenMoveType = {};

  function log(message) {
    if (globalThis.console) console.log("[s2script/menuhud] " + message);
  }

  function playerApi() {
    return globalThis.__s2pkg_cs2 && globalThis.__s2pkg_cs2.Player;
  }

  function freeze(slot, session) {
    if (!session.menu.freezePlayer || frozenMoveType[slot] !== undefined) return;
    var Player = playerApi();
    var player = Player && typeof Player.fromSlot === "function" ? Player.fromSlot(slot) : null;
    var pawn = player && player.pawn;
    if (!pawn) return;
    var moveType = pawn.moveType;
    if (moveType === null || moveType === MOVETYPE_NONE) return;
    frozenMoveType[slot] = moveType;
    pawn.moveType = MOVETYPE_NONE;
  }

  function unfreeze(slot) {
    if (frozenMoveType[slot] === undefined) return;
    var moveType = frozenMoveType[slot];
    delete frozenMoveType[slot];
    var Player = playerApi();
    var player = Player && typeof Player.fromSlot === "function" ? Player.fromSlot(slot) : null;
    var pawn = player && player.pawn;
    if (pawn) pawn.moveType = moveType;
  }

  function activationOf(session) {
    return session && session.menu && session.menu.activation === "tab" ? "tab" : "immediate";
  }

  function itemRows(slot) {
    var session = sessions[slot];
    if (!session || typeof session.view !== "function") return [];
    var lines = session.view().lines || [];
    var rows = [];
    for (var i = 0; i < lines.length; i++) {
      var line = lines[i];
      if (line.control) continue;
      rows.push({
        a: line.text == null ? "" : String(line.text),
        disabled: !line.selectable,
        key: line.key,
        index: line.index
      });
    }
    return rows;
  }

  function footerButtons(slot) {
    var session = sessions[slot];
    if (!session || typeof session.view !== "function") return [];
    var view = session.view();
    var buttons = [];
    if (view.page > 0) {
      buttons.push({
        text: "Back",
        onClick: function (sl) {
          var s = sessions[sl];
          if (s && typeof s.pickNumber === "function") s.pickNumber(8);
        }
      });
    }
    if (view.page < view.pageCount - 1) {
      buttons.push({
        text: "Next",
        onClick: function (sl) {
          var s = sessions[sl];
          if (s && typeof s.pickNumber === "function") s.pickNumber(9);
        }
      });
    }
    if (view.exit) {
      buttons.push({
        text: "Close",
        onClick: function (sl) {
          var s = sessions[sl];
          if (s && typeof s.cancel === "function") s.cancel();
        }
      });
    }
    return buttons;
  }

  if (!hudkit || typeof hudkit.modal !== "function" || !Menu || !MenuStyle) return;

  var claim = hudkit.modal({
    title: function (slot) {
      var session = sessions[slot];
      return session && session.menu ? (session.menu.title || "") : "";
    },
    rows: itemRows,
    buttons: footerButtons,
    pageSize: 8,
    onPick: function (slot, _index, row) {
      if (!row || row.disabled || !row.key) return;
      var session = sessions[slot];
      if (session && typeof session.pickNumber === "function") session.pickNumber(parseInt(row.key, 10));
    }
  });

  if (!claim) {
    log("hudkit modal claim unavailable; Menu renderer not registered (Chat fallback remains active)");
    return;
  }

  function setCursor(slot, on) {
    if (typeof claim.setCursor === "function") claim.setCursor(slot, on);
  }

  var renderer = {
    open: function (session) {
      var slot = session.slot;
      sessions[slot] = session;
      var activation = activationOf(session);
      claim.open(slot, { cursor: activation === "immediate" });
      freeze(slot, session);
      if (activation === "tab" && HudInput && typeof HudInput.arm === "function") {
        HudInput.arm(slot, {
          onActivate: function () { setCursor(slot, true); }
        });
      }
    },
    update: function (session) {
      sessions[session.slot] = session;
      if (typeof claim.refresh === "function") claim.refresh(session.slot);
    },
    close: function (slot) {
      if (typeof claim.close === "function") claim.close(slot);
      unfreeze(slot);
      if (HudInput && typeof HudInput.disarm === "function") HudInput.disarm(slot);
      delete sessions[slot];
    }
  };

  /**
   * Tear a slot's menu down WITHOUT restoring its saved moveType.
   *
   * A player who leaves mid-menu never reaches `renderer.close`: a disconnect is not a close, and
   * the plugin that owns the Menu has no reason to close one for someone who is gone. Everything
   * this renderer put on the slot then outlives them — the session, the saved moveType, the cursor
   * grab and the Tab arm — and the next occupant of that slot inherits all of it. In practice that
   * is the SAME person reconnecting, who arrives frozen, input-captured, and waiting to click a
   * sheet that nobody drew for them.
   *
   * `unfreeze` is deliberately not used: it would write a departed player's moveType onto whoever
   * holds the slot now. The replacement pawn is fresh and already has the right one, so the saved
   * value is dropped rather than applied.
   */
  function discardSession(slot) {
    if (typeof slot !== "number" || slot < 0) return;
    // Nothing held, nothing to release. `onActive` fires for every player who joins, and the
    // overwhelming majority of them never had a menu open on that slot.
    if (sessions[slot] === undefined && frozenMoveType[slot] === undefined) return;
    delete sessions[slot];
    delete frozenMoveType[slot];
    if (typeof claim.close === "function") claim.close(slot);
    setCursor(slot, false);
    if (HudInput && typeof HudInput.disarm === "function") HudInput.disarm(slot);
    if (typeof claim.forget === "function") claim.forget(slot);
    log("cleared a stale menu session on slot " + slot);
  }

  Menu.registerRenderer(MenuStyle.Center, renderer);
  Menu.registerRenderer(MenuStyle.Chat, renderer);

  // Both edges, because neither alone is enough. A timeout or a crash does not always deliver the
  // disconnect, so ACTIVATE is the backstop — the last moment before the new occupant of a slot can
  // be frozen by state they never created. Both are idempotent.
  if (Clients) {
    if (typeof Clients.onDisconnect === "function") {
      Clients.onDisconnect(function (client) { discardSession(client && client.slot); });
    }
    if (typeof Clients.onActive === "function") {
      Clients.onActive(function (client) { discardSession(client && client.slot); });
    }
  }

  globalThis.__s2pkg_menuhud = {
    renderer: renderer, rows: itemRows, buttons: footerButtons, discardSession: discardSession
  };
})();
