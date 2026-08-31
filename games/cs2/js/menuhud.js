// @s2script/cs2 — Menu renderer backed by the shared hudkit modal.
//
// This is deliberately separate from pawn.js: Menu is a generic model, while the CS2 HUD,
// cursor activation, and pawn freeze are game-package concerns.
(function () {
  var components = globalThis.__s2pkg_components;
  var hudkit = components && components.hudkit;
  var menuPackage = globalThis.__s2pkg_menu;
  var Menu = menuPackage && menuPackage.Menu;
  var HudInput = globalThis.__s2pkg_hudinput && globalThis.__s2pkg_hudinput.HudInput;
  var claim = hudkit && typeof hudkit.modal === "function" ? hudkit.modal() : null;
  var frozenSlots = new Set();
  var frozenMoveTypes = {};
  var activationBySlot = {};
  var routesBySlot = {};
  var MOVETYPE_NONE = 0;

  function log(message) {
    if (globalThis.console) globalThis.console.log("[s2script/menuhud] " + message);
  }

  function slotFor(session) {
    if (session && session.player && typeof session.player.slot === "number") {
      return session.player.slot;
    }
    return session && typeof session.slot === "number" ? session.slot : -1;
  }

  function playerApi() {
    return globalThis.Player ||
      (globalThis.__s2pkg_cs2 && globalThis.__s2pkg_cs2.Player);
  }

  // Copy of pawn.js's freeze contract. A menu captures the current move type and restores it
  // instead of assuming WALK on close; that preserves noclip and other caller-owned state.
  function freeze(slot, session) {
    if (!session.menu.freezePlayer || frozenSlots.has(slot)) return;
    var Player = playerApi();
    var player = Player && typeof Player.fromSlot === "function" ? Player.fromSlot(slot) : null;
    var pawn = player && player.pawn;
    if (!pawn) return;
    var moveType = pawn.moveType;
    if (moveType === null || moveType === MOVETYPE_NONE) return;
    frozenSlots.add(slot);
    frozenMoveTypes[slot] = moveType;
    pawn.moveType = MOVETYPE_NONE;
  }

  function unfreeze(slot) {
    if (!frozenSlots.has(slot)) return;
    var moveType = frozenMoveTypes[slot];
    frozenSlots.delete(slot);
    delete frozenMoveTypes[slot];
    var Player = playerApi();
    var player = Player && typeof Player.fromSlot === "function" ? Player.fromSlot(slot) : null;
    var pawn = player && player.pawn;
    if (pawn) pawn.moveType = moveType;
  }

  function itemLabel(item) {
    if (item.display != null) return String(item.display);
    if (item.text != null) return String(item.text);
    if (item.title != null) return String(item.title);
    return "";
  }

  function rowsFromPage(page) {
    var rows = [];
    var clickable = [];
    for (var i = 0; i < page.length; i++) {
      var item = page[i];
      // `a` is the modal row's primary display field. Disabled rows remain visual-only; their
      // null route is intentional because hudkit's cosmetic disabled state still emits onPick.
      rows.push({ a: itemLabel(item), disabled: !!item.disabled });
      clickable.push(item.disabled ? null : item);
    }
    return { rows: rows, clickable: clickable };
  }

  function callIfPresent(name, slot, value) {
    if (typeof claim[name] === "function") claim[name](slot, value);
  }

  function actionId(action, maybeAction) {
    var value = maybeAction === undefined ? action : maybeAction;
    if (typeof value === "string") return value;
    return value && (value.id || value.action || value.name);
  }

  function display(session) {
    var slot = slotFor(session);
    if (slot < 0 || !session || typeof session.pageItems !== "function") return;

    var page = session.pageItems();
    var mapped = rowsFromPage(Array.isArray(page) ? page : []);
    routesBySlot[slot] = mapped.clickable;

    callIfPresent("setTitle", slot, session.menu.title);
    callIfPresent("setSubtitle", slot, "");
    callIfPresent("setRows", slot, mapped.rows);
    callIfPresent("setPager", slot, session.hasPrev || session.hasNext ? {
      page: session.page + 1,
      pageCount: session.pageCount,
      hasPrev: !!session.hasPrev,
      hasNext: !!session.hasNext
    } : null);

    callIfPresent("onPick", slot, function (pickedSlot, index) {
      var actualSlot = index === undefined ? slot :
        (typeof pickedSlot === "number" ? pickedSlot : slot);
      var pickIndex = index === undefined ? pickedSlot : index;
      var route = routesBySlot[actualSlot] || [];
      var item = route[pickIndex];
      if (!item || item.disabled) return;
      session.select(item.index);
    });
    callIfPresent("onBack", slot, function () {
      if (session.hasPrev) session.prevPage();
      else session.close();
    });
    callIfPresent("onClose", slot, function () { session.close(); });
    callIfPresent("onAction", slot, function (actionSlot, action) {
      var id = actionId(actionSlot, action);
      if (id === "back" || id === "prev") {
        if (session.hasPrev) session.prevPage();
        else session.close();
      } else if (id === "next") {
        if (session.hasNext) session.nextPage();
      } else if (id === "close" && session.menu.exitButton) {
        session.close();
      }
    });

    var activation = session.menu.activation === "tab" ? "tab" : "immediate";
    activationBySlot[slot] = activation;
    // components.js is being extended so open(slot, { cursor }) honors this option. Keep the
    // option on both paths; older hosts currently ignore the second argument and are upgraded
    // by the corresponding components.js patch.
    claim.open(slot, { cursor: activation === "immediate" });
    if (activation === "immediate") {
      if (session.menu.freezePlayer) freeze(slot, session);
    } else if (HudInput && typeof HudInput.arm === "function") {
      HudInput.arm(slot, {
        onActivate: function () {
          // Do not re-open the modal: activation must preserve its current page and handlers.
          // The setCursor method is supplied by the components.js host patch. Without it,
          // open(..., { cursor: false }) still paints the tab-waiting sheet but cannot acquire
          // the cursor after activation.
          if (typeof claim.setCursor === "function") claim.setCursor(slot, true);
        }
      });
      if (typeof HudInput.isActive === "function" && HudInput.isActive(slot) &&
          typeof claim.setCursor === "function") {
        claim.setCursor(slot, true);
      }
    }
  }

  function hide(session) {
    var slot = slotFor(session);
    if (slot < 0) return;
    if (claim && typeof claim.close === "function") claim.close(slot);
    unfreeze(slot);
    if (activationBySlot[slot] === "tab" && HudInput &&
        typeof HudInput.disarm === "function") {
      HudInput.disarm(slot);
    }
    delete activationBySlot[slot];
    delete routesBySlot[slot];
  }

  if (!claim) {
    log("hudkit modal claim unavailable; Menu renderer not registered (Chat fallback remains active)");
  } else if (Menu && typeof Menu.registerRenderer === "function") {
    globalThis.__s2pkg_menuhud = {
      name: "cs2-hud",
      display: display,
      hide: hide,
      rowsFromPage: rowsFromPage
    };
    Menu.registerRenderer(globalThis.__s2pkg_menuhud);
  }
})();
