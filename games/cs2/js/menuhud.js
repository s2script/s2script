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

  /**
   * Freeze the player while the menu is up.
   *
   * DISABLED, and deliberately so — see the guard below. Freezing is a convenience (a player does
   * not wander off mid-menu); being unable to move is a TRAP the moment anything about the menu
   * fails. On a live server a broken `sm_admin` left admins frozen, unable to select anything and
   * unable to close it, with no recovery short of a reconnect — and reconnecting did not help
   * either, because the teardown could not find a session to clean up.
   *
   * The asymmetry is the whole argument: the upside of freezing is small and cosmetic, the
   * downside is a player who cannot play. Until the menu surface can guarantee it is interactive,
   * this stays off. `frozenMoveType` and {@link unfreeze} remain so that any player frozen by an
   * older build is still released on the next teardown.
   */
  var FREEZE_ENABLED = false;

  function freeze(slot, session) {
    if (!FREEZE_ENABLED) return;
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

  if (!hudkit || typeof hudkit.whenLive !== "function" || !Menu || !MenuStyle) return;

  // Register the presenter at go-live, but reserve a panel only while a menu is open.
  // A plugin that never displays menus must not consume the host's finite panel pool.
  hudkit.whenLive(function (kit) {
    var claim = null;
    var fallbackSlots = {};
    var spec = {
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
    };

    function releaseIfIdle() {
      if (claim && Object.keys(sessions).length === 0) {
        claim.release();
        claim = null;
      }
    }

    function setCursor(slot, on) {
      if (claim && typeof claim.setCursor === "function") claim.setCursor(slot, on);
    }

    var renderer = {
      open: function (session) {
        var slot = session.slot;
        if (!claim) claim = kit.modal(spec);
        if (!claim) {
          log("modal pool busy; using the chat menu for slot " + slot);
          fallbackSlots[slot] = true;
          if (fallback) fallback.open(session);
          else session.cancel();
          return;
        }
        sessions[slot] = session;
        var activation = activationOf(session);
        var result = claim.tryOpen(slot, { cursor: activation === "immediate" });
        if (!result.ok) {
          delete sessions[slot];
          releaseIfIdle();
          log("HUD open failed (" + result.error + "); using the chat menu for slot " + slot);
          fallbackSlots[slot] = true;
          if (fallback) fallback.open(session);
          else session.cancel();
          return;
        }
        freeze(slot, session);
        if (activation === "tab" && HudInput && typeof HudInput.arm === "function") {
          HudInput.arm(slot, {
            onActivate: function () { setCursor(slot, true); }
          });
        }
      },
      update: function (session) {
        if (fallbackSlots[session.slot]) { if (fallback) fallback.update(session); return; }
        if (!claim || !sessions[session.slot]) return;
        sessions[session.slot] = session;
        if (typeof claim.refresh === "function") claim.refresh(session.slot);
      },
      close: function (slot) {
        if (fallbackSlots[slot]) {
          delete fallbackSlots[slot];
          if (fallback) fallback.close(slot);
          return;
        }
        if (claim && typeof claim.close === "function") claim.close(slot);
        unfreeze(slot);
        if (HudInput && typeof HudInput.disarm === "function") HudInput.disarm(slot);
        delete sessions[slot];
        releaseIfIdle();
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
      var had = sessions[slot] !== undefined || frozenMoveType[slot] !== undefined;
      delete sessions[slot];
      delete frozenMoveType[slot];
      // UNCONDITIONAL — this used to return early when no session was tracked, on the reasoning that
      // there was nothing to release. That reasoning was wrong in the exact case this function exists
      // for. The session is deleted by `renderer.close`, but the CURSOR GRAB is per-player state on
      // the layout entity and is released separately; any path that drops one without the other
      // leaves a player captured, pointing at a menu no longer drawn — and reconnecting was then a
      // no-op, because by that point there was no session left to find. Measured on a live server:
      // the teardown ran and logged NOTHING, because it had already returned above this line.
      //
      // Releasing state that is already released is free. Failing to release it is a player who
      // cannot play. So the teardown no longer asks whether it thinks it has anything to do.
      if (claim && typeof claim.close === "function") claim.close(slot);
      setCursor(slot, false);
      if (HudInput && typeof HudInput.disarm === "function") HudInput.disarm(slot);
      if (claim && typeof claim.forget === "function") claim.forget(slot);
      if (fallbackSlots[slot]) {
        delete fallbackSlots[slot];
        if (fallback) fallback.close(slot);
      }
      releaseIfIdle();
      // Only NOISE is conditional: `onActive` fires for every joiner and most never had a menu.
      if (had) log("cleared a stale menu session on slot " + slot);
    }

    Menu.registerRenderer(MenuStyle.Center, renderer);
    var fallback = Menu.registerRenderer(MenuStyle.Chat, renderer);
    var Server = globalThis.__s2pkg_server && globalThis.__s2pkg_server.Server;
    if (Server && typeof Server.onMapStart === "function") {
      Server.onMapStart(function () {
        Object.keys(sessions).forEach(function (slot) { discardSession(Number(slot)); });
        Object.keys(fallbackSlots).forEach(function (slot) { discardSession(Number(slot)); });
      });
    }

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
  });
})();
