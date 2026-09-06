// @s2script/cs2 — custom_hud_layout (CCSCustomHudLayout). ES5 IIFE concatenated after pawn.js.
//
// The engine object is one layout entity. Almost every drive is per-player
// (SetHasClassForPlayer / SetDialogVariableStringForPlayer / SetInputCaptureEnabledForPlayer),
// so the authoring face is CustomHudLayout.create(spec) + layout.forSlot(slot).
// `ui` remains a deprecated alias of the same load-window object.
(function () {
  if (!globalThis.__s2pkg_entity || !globalThis.__s2pkg_cs2_calls) return;

  function entityApi() { return globalThis.__s2pkg_entity; }
  function callsApi() { return globalThis.__s2pkg_cs2_calls; }
  function serverApi() { return globalThis.__s2pkg_server.Server; }
  function clientsApi() { return globalThis.__s2pkg_clients.Clients; }

  var HUD_CLASS = "custom_hud_layout";
  var CLASS_DOES_NOT_HAVE = 0;
  var CLASS_HAS = 1;
  var SIGNON_ACTIVE = 6;

  function uiOk(value) { return { ok: true, value: value }; }
  function uiFail(code, message) { return { ok: false, error: { code: code, message: message } }; }
  function legacyResult(result) { return result.ok ? null : result.error.message; }

  var DEFAULT_DESCRIPTOR = {
    addons: ["3790153369"],
    resource: "panorama/layout/custom_game/s2script_hud.xml",
    hideClass: "s2-hidden",
    text: {
      s2_dialog_kicker: "kicker",
      s2_dialog_title: "title",
      s2_dialog_body: "body",
      s2_btn_0_text: "btn0",
      s2_btn_1_text: "btn1",
      s2_btn_2_text: "btn2",
      s2_btn_3_text: "btn3",
      s2_hud_tl_head: "tl_head",
      s2_hud_tl_body: "tl_body",
      s2_hud_tr_head: "tr_head",
      s2_hud_tr_body: "tr_body",
      s2_hud_bl_head: "bl_head",
      s2_hud_bl_body: "bl_body",
      s2_hud_br_head: "br_head",
      s2_hud_br_body: "br_body",
      s2_banner_text: "banner",
      s2_list_title: "list_title",
      s2_list_foot: "list_foot",
      s2_meter_label: "meter_label"
    },
    buttons: ["s2_btn_0", "s2_btn_1", "s2_btn_2", "s2_btn_3"],
    meters: { meter: "s2_meter_fill" },
    slots: {
      rows: [
        { id: "s2_row_0", vars: ["row0"] },
        { id: "s2_row_1", vars: ["row1"] },
        { id: "s2_row_2", vars: ["row2"] },
        { id: "s2_row_3", vars: ["row3"] },
        { id: "s2_row_4", vars: ["row4"] },
        { id: "s2_row_5", vars: ["row5"] },
        { id: "s2_row_6", vars: ["row6"] },
        { id: "s2_row_7", vars: ["row7"] }
      ]
    }
  };

  function engineCall(name) {
    var pkg = callsApi();
    var call = pkg && pkg.call ? pkg.call(name) : null;
    if (!call) return null;
    // These HUD descriptors return void: core returns undefined on success and null on
    // rejection (including a stale receiver or a shim-side invocation failure).
    return function () {
      if (call.apply(null, arguments) !== null) return uiOk(undefined);
      var reason = engineStatus(name);
      return uiFail("PaintFailed",
        name + ": " + (reason && reason !== "available" ? reason : "engine invocation failed"));
    };
  }
  function engineStatus(name) {
    var pkg = callsApi();
    return pkg && pkg.status ? pkg.status(name) : "game calls unavailable";
  }

  var setHasClassForPlayer = engineCall("setHasClassForPlayer");
  var setDialogVariableStringForPlayer = engineCall("setDialogVariableStringForPlayer");

  function warn(msg) { if (globalThis.console) console.log("[s2script] " + msg); }

  function meterClassFor(percent) {
    var clamped = Math.max(0, Math.min(100, percent));
    var stepped = Math.round(clamped / 10);
    return "s2-w" + stepped;
  }

  function targetNameForResource(resource) {
    return "s2_ui_" + String(resource).replace(/[^a-zA-Z0-9]+/g, "_").replace(/^_|_$/g, "");
  }

  function rejectVxml(resource) {
    if (/\.vxml(_c)?$/i.test(resource)) {
      return 'layout resource must use the .xml source extension (got "' + resource + '")';
    }
    return null;
  }

  function fail(msg) { throw new Error("CustomHudLayout.create: " + msg); }

  function isFields(x) {
    return x !== null && typeof x === "object" && !Array.isArray(x);
  }

  function slotPrefix(slot) { return "#" + slot + "|"; }

  function validateDescriptor(desc) {
    if (!desc || typeof desc !== "object") fail("spec must be an object");
    if (!Array.isArray(desc.addons) || desc.addons.length === 0) {
      fail("spec.addons must be a non-empty string array");
    }
    for (var a = 0; a < desc.addons.length; a++) {
      if (!/^\d+$/.test(String(desc.addons[a]))) {
        fail('addon id "' + desc.addons[a] + '" must be a decimal workshop id');
      }
    }
    if (typeof desc.resource !== "string" || !desc.resource) {
      fail("spec.resource is required");
    }
    if (desc.resource.indexOf("panorama/layout/custom_game/") !== 0 || !/\.xml$/i.test(desc.resource)) {
      fail("resource must be under panorama/layout/custom_game/ and end in .xml");
    }
    var vxmlErr = rejectVxml(desc.resource);
    if (vxmlErr) fail(vxmlErr);
    var buttons = Array.isArray(desc.buttons) ? desc.buttons : [];
    var seenBtn = {};
    for (var b = 0; b < buttons.length; b++) {
      var bid = buttons[b];
      if (!bid) fail("button ids must be non-empty");
      if (seenBtn[bid]) fail('duplicate button id "' + bid + '"');
      seenBtn[bid] = true;
    }
    if (desc.slots) {
      for (var poolName in desc.slots) {
        if (!Object.prototype.hasOwnProperty.call(desc.slots, poolName)) continue;
        var pool = desc.slots[poolName];
        if (!Array.isArray(pool)) fail("slots." + poolName + " must be an array");
        for (var i = 0; i < pool.length; i++) {
          var slotDef = pool[i];
          if (!slotDef || !slotDef.id || !Array.isArray(slotDef.vars)) {
            fail("slots." + poolName + "[" + i + "] needs { id, vars[] }");
          }
        }
      }
    }
    return {
      addons: desc.addons,
      resource: desc.resource,
      hideClass: desc.hideClass || "s2-hide",
      text: desc.text || {},
      buttons: buttons,
      meters: desc.meters || {},
      slots: desc.slots
    };
  }

  function parseMmAddons(raw) {
    if (raw == null || raw === "") return { present: false, listed: [], missing: [] };
    var want = {};
    for (var i = 0; i < DEFAULT_DESCRIPTOR.addons.length; i++) want[DEFAULT_DESCRIPTOR.addons[i]] = true;
    var parts = String(raw).split(",");
    var listed = [];
    var missing = [];
    for (var p = 0; p < parts.length; p++) {
      var id = parts[p].trim();
      if (!id) continue;
      listed.push(id);
      if (want[id]) delete want[id];
    }
    for (var k in want) if (Object.prototype.hasOwnProperty.call(want, k)) missing.push(k);
    return { present: true, listed: listed, missing: missing };
  }

  function resolveClicker(ref) {
    if (!ref) return null;
    var all = globalThis.__s2pkg_cs2.Player.all();
    for (var i = 0; i < all.length; i++) {
      var p = all[i];
      if (p.ref.index === ref.index && p.ref.id === ref.id) return p;
    }
    return null;
  }

  function makeHud(desc, ctxState) {
    var layout = desc;
    var disabled = {};
    var handlers = {};
    var subscribers = {};
    var meterClass = {};
    var visiblePanels = {};
    var lastValue = {};
    var slotViews = {};
    var slotEpochs = {};
    var componentSlotEpochs = {};
    var entityEpoch = 0;
    // Host id of the layout entity every cache above was built against (EntityRef.id — the
    // host-minted identity, the same one the entity system gates liveness on). null = not bound
    // yet. The caches describe state that lives ON that one entity instance, so they are only
    // meaningful while that exact entity is the one being driven.
    var boundEntityId = null;
    var slotClients = {};
    var boundClient = null;
    var boundBinding = null;

    function sameClient(a, b) { return globalThis.__s2pkg_clients._same(a, b); }
    function bumpSlot(slot) { slotEpochs[slot] = (slotEpochs[slot] || 0) + 1; }
    function forgetSlot(slot, invalidateView) {
      componentSlotEpochs[slot] = (componentSlotEpochs[slot] || 0) + 1;
      if (invalidateView) bumpSlot(slot);
      delete disabled[slot]; delete slotViews[slot];
      if (invalidateView) delete slotClients[slot];
      forgetKeyed(meterClass, slot); forgetKeyed(visiblePanels, slot); forgetKeyed(lastValue, slot);
    }
    // Fresh slot-first bindings adopt the actual occupant, even when captured reentrantly
    // from a retired component's drive. Only primitive writes inherit that ambient fence.
    function resolveCurrentClient(slot) {
      var current = clientsApi().fromSlot(slot);
      if (!current) return null;
      if (!sameClient(slotClients[slot], current)) { forgetSlot(slot, true); slotClients[slot] = current; }
      return current;
    }
    function currentClient(slot) {
      if (boundBinding) {
        return boundBinding.slot === slot && bindingIsValid(boundBinding) ? boundBinding.client : null;
      }
      if (boundClient && (boundClient.slot !== slot || !boundClient.isValid())) return null;
      return resolveCurrentClient(slot);
    }

    function cacheKey(slot, kind, a, b) {
      return slotPrefix(slot) + kind + "|" + a + "|" + (b == null ? "" : b);
    }

    function forgetKeyed(map, slot) {
      var prefix = slotPrefix(slot);
      for (var key in map) {
        if (Object.prototype.hasOwnProperty.call(map, key) && key.indexOf(prefix) === 0) {
          delete map[key];
        }
      }
    }

    /**
     * Drop EVERY cache that mirrors state living on the layout entity.
     *
     * All of these exist to suppress redundant engine calls — setText/setClass return early when
     * the cached value already matches — and every one of them is a lie the moment a DIFFERENT
     * entity is being driven, because a fresh entity has every panel at its markup default and no
     * input capture at all. Left stale, the effect is a PARTIAL PAINT: any value unchanged since
     * the previous entity is suppressed and never re-sent, so a sheet draws its rows and not its
     * buttons — intermittently, depending on what happened to differ. Capture leases live in the
     * host, keyed by entity identity; its entity lifecycle removes them independently of JS.
     *
     * `handlers` is deliberately NOT cleared: click handlers are registered once per button id at
     * claim time and belong to the plugin. Low-level `slotViews` remain reusable for the same
     * client, while component bindings carry `entityEpoch` and expire here.
     */
    function resetEntityCaches() {
      entityEpoch++;
      disabled = {};
      meterClass = {};
      visiblePanels = {};
      lastValue = {};
      boundEntityId = null;
    }

    /**
     * Identity-gate every entity resolve. The caches are keyed to ONE entity instance (compared
     * by host id), not to a map: if the entity dies and is recreated mid-map, or the map-start
     * reset never ran (resetForMap arrives through a `reg` subscription, which must not be the
     * only line of defence), the first resolve of the replacement entity is the moment the
     * caches become lies — so that is where they are dropped. Self-healing no matter which
     * lifecycle notification did or didn't fire.
     */
    function bindEntity(ent) {
      if (!ent) return null;
      if (boundEntityId !== null && boundEntityId !== ent.id) resetEntityCaches();
      boundEntityId = ent.id;
      return ent;
    }

    function bindingIsValid(binding) {
      if (!binding || (typeof binding._componentIsValid === "function" && !binding._componentIsValid())) {
        return false;
      }
      var ent = ctxState.findEntity(layout);
      if (boundEntityId !== null && ent && boundEntityId !== ent.id) resetEntityCaches();
      return !!binding.client && binding.client.isValid() &&
        sameClient(binding.client, clientsApi().fromSlot(binding.slot)) &&
        !!binding.view && binding.view.isValid() &&
        binding.slotEpoch === (componentSlotEpochs[binding.slot] || 0) &&
        binding.entityEpoch === entityEpoch;
    }

    function driveSetClass(slot, panelId, className, on) {
      panelId = String(panelId); className = String(className);
      if (!setHasClassForPlayer) {
        return uiFail("Unavailable", "unavailable: " + engineStatus("setHasClassForPlayer"));
      }
      if (slot < 0) return uiFail("InvalidArgument", "needs a player slot");
      var ent = bindEntity(ctxState.ensureEntity(layout));
      if (!ent) return uiFail("NotReady", ctxState.notReadyReason());
      if (!currentClient(slot)) return uiFail("StaleClient", "stale client");
      var key = cacheKey(slot, "c", panelId, className);
      var s = on ? "1" : "0";
      if (lastValue[key] === s) return uiOk(undefined);
      var result = setHasClassForPlayer(ent, slot, panelId, className,
        on ? CLASS_HAS : CLASS_DOES_NOT_HAVE);
      if (!result.ok) return result;
      if (!currentClient(slot)) return uiFail("StaleClient", "stale client");
      lastValue[key] = s;
      return uiOk(undefined);
    }

    function driveSetDialogVariable(slot, panelId, variableName, value) {
      panelId = String(panelId); variableName = String(variableName);
      if (!setDialogVariableStringForPlayer) {
        return uiFail("Unavailable", "unavailable: " + engineStatus("setDialogVariableStringForPlayer"));
      }
      if (slot < 0) return uiFail("InvalidArgument", "needs a player slot");
      var ent = bindEntity(ctxState.ensureEntity(layout));
      if (!ent) return uiFail("NotReady", ctxState.notReadyReason());
      var str = String(value);
      if (!currentClient(slot)) return uiFail("StaleClient", "stale client");
      var key = cacheKey(slot, "v", panelId, variableName);
      if (lastValue[key] === str) return uiOk(undefined);
      var result = setDialogVariableStringForPlayer(ent, slot, panelId, variableName, str);
      if (!result.ok) return result;
      if (!currentClient(slot)) return uiFail("StaleClient", "stale client");
      lastValue[key] = str;
      return uiOk(undefined);
    }

    function driveCursorSwitch(slot, token, on) {
      if (typeof globalThis.__s2_shared_entity_switch !== "function") {
        return uiFail("Unavailable", "unavailable: shared entity switch host support");
      }
      if (slot < 0) return uiFail("InvalidArgument", "needs a player slot");
      var ent = bindEntity(on ? ctxState.ensureEntity(layout) : ctxState.findEntity(layout));
      if (!ent) return on ? uiFail("NotReady", ctxState.notReadyReason()) : uiOk(undefined);
      if (!currentClient(slot)) return uiFail("StaleClient", "stale client");
      var err = globalThis.__s2_shared_entity_switch("setInputCaptureEnabledForPlayer",
        ent.index, ent.id, slot, token, !!on);
      if (err) return uiFail("PaintFailed", err);
      return currentClient(slot) ? uiOk(undefined) : uiFail("StaleClient", "stale client");
    }
    function driveAcquireCursor(slot, panelId) {
      return driveCursorSwitch(slot, "panel:" + panelId, true);
    }
    function driveReleaseCursor(slot, panelId) {
      return driveCursorSwitch(slot, "panel:" + panelId, false);
    }

    function trackVisible(slot, panelId, on) {
      var key = slotPrefix(slot) + panelId;
      if (on) visiblePanels[key] = true;
      else delete visiblePanels[key];
    }

    function driveShow(slot, panelId, opts) {
      opts = opts || {};
      var result = driveSetClass(slot, panelId, layout.hideClass, false);
      if (!result.ok) return result;
      trackVisible(slot, panelId, true);
      if (opts.cursor) {
        result = driveAcquireCursor(slot, panelId);
        if (!result.ok) {
          driveSetClass(slot, panelId, layout.hideClass, true);
          trackVisible(slot, panelId, false);
          return result;
        }
      }
      return uiOk(undefined);
    }

    function driveHide(slot, panelId) {
      var result = driveSetClass(slot, panelId, layout.hideClass, true);
      if (result.ok) trackVisible(slot, panelId, false);
      // Releasing input is independent of painting: close must not strand a cursor if paint fails.
      var captureResult = driveReleaseCursor(slot, panelId);
      return result.ok ? captureResult : result;
    }

    function driveSet(slot, id, value) {
      if (isFields(id)) {
        var first = null;
        for (var key in id) {
          if (!Object.prototype.hasOwnProperty.call(id, key)) continue;
          var fieldResult = driveSetDialogVariable(slot, key, key, id[key]);
          if (!fieldResult.ok && !first) first = fieldResult;
        }
        return first || uiOk(undefined);
      }
      return driveSetDialogVariable(slot, id, id, value);
    }

    function driveSetText(slot, panelId, value) {
      if (isFields(panelId)) {
        var first = null;
        for (var key in panelId) {
          if (!Object.prototype.hasOwnProperty.call(panelId, key)) continue;
          var fieldResult = driveSetText(slot, key, panelId[key]);
          if (!fieldResult.ok && !first) first = fieldResult;
        }
        return first || uiOk(undefined);
      }
      var varName = layout.text[panelId] || panelId;
      return driveSetDialogVariable(slot, panelId, varName, value);
    }

    function driveSetMeter(slot, meterName, percent) {
      var fillId = layout.meters[meterName];
      if (!fillId) return uiFail("InvalidArgument", 'no meter "' + meterName + '" in this layout');
      var next = meterClassFor(percent);
      var key = slotPrefix(slot) + fillId;
      var prev = meterClass[key];
      if (prev && prev !== next) {
        var clearResult = driveSetClass(slot, fillId, prev, false);
        if (!clearResult.ok) return clearResult;
      }
      var result = driveSetClass(slot, fillId, next, true);
      if (result.ok) meterClass[key] = next;
      return result;
    }

    function driveSetPool(slot, poolName, entries) {
      var pool = layout.slots && layout.slots[poolName];
      if (!pool) return uiFail("InvalidArgument", 'no pool "' + poolName + '" in this layout');
      if (entries.length > pool.length) {
        return uiFail("InvalidArgument", 'pool "' + poolName + '" holds ' + pool.length +
          ' slot(s); ' + entries.length + " given — paginate instead");
      }
      for (var i = 0; i < pool.length; i++) {
        var slotDef = pool[i];
        var row = entries[i];
        if (!row) {
          var hideResult = driveSetClass(slot, slotDef.id, layout.hideClass, true);
          if (!hideResult.ok) return hideResult;
          continue;
        }
        var showResult = driveSetClass(slot, slotDef.id, layout.hideClass, false);
        if (!showResult.ok) return showResult;
        for (var f = 0; f < slotDef.vars.length && f < row.length; f++) {
          var varResult = driveSetDialogVariable(slot, slotDef.id, slotDef.vars[f], row[f]);
          if (!varResult.ok) return varResult;
        }
      }
      return uiOk(undefined);
    }

    function driveSetDisabled(slot, buttonId, disabledOn) {
      // Paint before recording: driveSetClass's entity resolve can detect a replacement entity and
      // reset the books, and a book entry written first would be swallowed by that very reset.
      // Still recorded even when the paint fails (world not ready): dispatchClick suppression is
      // plugin logic and must not depend on the visual having landed.
      var result = driveSetClass(slot, buttonId, "s2-btn-disabled", disabledOn);
      if (boundBinding && !bindingIsValid(boundBinding)) {
        return result.ok ? uiFail("StaleClient", "stale client") : result;
      }
      var set = disabled[slot];
      if (!set) { set = {}; disabled[slot] = set; }
      if (disabledOn) set[buttonId] = true; else delete set[buttonId];
      return result;
    }

    var api = { spec: layout, layout: layout };

    var rawDrive = {
      show: driveShow,
      hide: driveHide,
      cursor: function (slot, on) { return driveCursorSwitch(slot, "manual:*", !!on); },
      cursorForPanel: function (slot, panelId, on) {
        return on ? driveAcquireCursor(slot, panelId) : driveReleaseCursor(slot, panelId);
      },
      set: driveSet,
      setText: driveSetText,
      setClass: driveSetClass,
      setMeter: driveSetMeter,
      setPool: driveSetPool,
      setDisabled: driveSetDisabled,
      ensure: function () {
        var ref = bindEntity(ctxState.createEntity(layout));
        return ref ? uiOk(undefined) : uiFail("NotReady", ctxState.notReadyReason());
      }
    };

    // Private structured seam for game-package components. Every call captures the actual client
    // generation, then the raw operation re-checks it after argument coercion and reentrancy.
    api._drive = {};
    "show hide cursor cursorForPanel set setText setClass setMeter setPool setDisabled".split(" ")
      .forEach(function (name) {
        api._drive[name] = function (slot) {
          var actualClient = currentClient(slot);
          if (!actualClient) return uiFail("StaleClient", "stale client");
          var previous = boundClient; boundClient = actualClient;
          try { return rawDrive[name].apply(rawDrive, arguments); }
          finally { boundClient = previous; }
        };
      });
    api._drive.ensure = rawDrive.ensure;

    api.show = function (slot, panelId, opts) {
      return legacyResult(api._drive.show(slot, panelId, opts));
    };
    api.tryShow = function (slot, panelId, opts) { return api._drive.show(slot, panelId, opts); };
    api.hide = function (slot, panelId) {
      return legacyResult(api._drive.hide(slot, panelId));
    };
    api.cursor = function (slot, on) {
      return legacyResult(api._drive.cursor(slot, on));
    };
    // Game-presenter seam: changing one root must not touch a manual or another root's lease.
    api._cursorForPanel = function (slot, panelId, on) {
      return legacyResult(api._drive.cursorForPanel(slot, panelId, on));
    };
    api.set = function (slot, id, value) {
      return legacyResult(api._drive.set(slot, id, value));
    };
    api.setText = function (slot, panelId, value) {
      return legacyResult(api._drive.setText(slot, panelId, value));
    };
    api.setClass = function (slot, panelId, className, on) {
      return legacyResult(api._drive.setClass(slot, panelId, className, on));
    };
    // Internal component-pool handoff: clear only this panel tree's diff cache. Other live
    // components keep their leases and state. Panel ids occupy field 2 in cacheKey().
    api.invalidatePanelTree = function (root) {
      for (var key in lastValue) {
        if (!Object.prototype.hasOwnProperty.call(lastValue, key)) continue;
        var panel = key.split("|")[2];
        if (panel === root || panel.indexOf(root + "_") === 0) delete lastValue[key];
      }
    };
    api.setMeter = function (slot, meterName, percent) {
      return legacyResult(api._drive.setMeter(slot, meterName, percent));
    };
    api.capacity = function (poolName) {
      var pool = layout.slots && layout.slots[poolName];
      return pool ? pool.length : 0;
    };
    api.setPool = function (slot, poolName, entries) {
      return legacyResult(api._drive.setPool(slot, poolName, entries));
    };
    api.onClick = function (buttonId, handler) {
      if (ctxState.buttonHandlers[buttonId]) {
        throw new Error('CustomHud: conflicting handler for button id "' + buttonId + '"');
      }
      ctxState.buttonHandlers[buttonId] = handler;
      handlers[buttonId] = handler;
    };
    api.subscribeClick = function (buttonId, handler) {
      var id = String(buttonId);
      var list = subscribers[id];
      if (!list) { list = []; subscribers[id] = list; }
      var entry = { handler: handler, live: true };
      list.push(entry);
      return { dispose: function () {
        if (!entry.live) return;
        entry.live = false;
        var current = subscribers[id];
        if (!current) return;
        var index = current.indexOf(entry);
        if (index >= 0) current.splice(index, 1);
        if (current.length === 0) delete subscribers[id];
      } };
    };
    api.setDisabled = function (slot, buttonId, disabledOn) {
      return legacyResult(api._drive.setDisabled(slot, buttonId, disabledOn));
    };
    api.dispatchClick = function (slot, buttonId) {
      if (disabled[slot] && disabled[slot][buttonId]) return false;
      var h = handlers[buttonId];
      var list = subscribers[buttonId];
      var snapshot = list ? list.slice() : [];
      if (!h && snapshot.length === 0) return false;
      var player = api.forSlot(slot);
      if (h) h(player);
      for (var i = 0; i < snapshot.length; i++) snapshot[i].handler(player);
      return true;
    };
    api.forget = function (slot, client) {
      if (client) {
        if (!sameClient(slotClients[slot], client)) return;
        // Deferred disconnect must never repaint or release a replacement occupant's UI.
        var occupant = clientsApi().fromSlot(slot);
        if (occupant && !sameClient(occupant, client)) { forgetSlot(slot, true); return; }
      }
      // Forget releases only this plugin's leases. The host's unconditional disconnect path
      // clears ALL owners before JS callbacks, even if this plugin never registered a listener.
      // Never disable another plugin's capture during ordinary local cleanup.
      driveCursorSwitch(slot, null, false);
      var ent = bindEntity(ctxState.findEntity(layout));
      if (ent) {
        if (setHasClassForPlayer) {
          var prefix = slotPrefix(slot);
          var key;
          for (key in visiblePanels) {
            if (Object.prototype.hasOwnProperty.call(visiblePanels, key) && key.indexOf(prefix) === 0) {
              setHasClassForPlayer(ent, slot, key.slice(prefix.length), layout.hideClass, CLASS_HAS);
            }
          }
          // Meter fill + disabled classes too: books-wiped-but-class-painted is exactly the
          // mismatch this teardown exists to prevent. A wiped meterClass book means the next
          // occupant's first setMeter would ADD its width class without removing this one, and
          // two s2-w* classes then fight in CSS; a lingering s2-btn-disabled shows the next
          // occupant a greyed button that dispatchClick would happily fire.
          for (key in meterClass) {
            if (Object.prototype.hasOwnProperty.call(meterClass, key) && key.indexOf(prefix) === 0) {
              setHasClassForPlayer(ent, slot, key.slice(prefix.length), meterClass[key], CLASS_DOES_NOT_HAVE);
            }
          }
          var dis = disabled[slot];
          if (dis) {
            for (key in dis) {
              if (Object.prototype.hasOwnProperty.call(dis, key)) {
                setHasClassForPlayer(ent, slot, key, "s2-btn-disabled", CLASS_DOES_NOT_HAVE);
              }
            }
          }
        }
      }
      forgetSlot(slot, false);
    };
    api.resetEntityCaches = resetEntityCaches;
    api.ensure = function () {
      return legacyResult(api._drive.ensure());
    };
    api.status = function () {
      if (!setHasClassForPlayer) {
        return { server: "unavailable", clientContent: "unknown",
          reason: "setHasClassForPlayer: " + engineStatus("setHasClassForPlayer") };
      }
      if (!setDialogVariableStringForPlayer) {
        return { server: "unavailable", clientContent: "unknown",
          reason: "setDialogVariableStringForPlayer: " +
            engineStatus("setDialogVariableStringForPlayer") };
      }
      if (typeof globalThis.__s2_shared_entity_switch !== "function") {
        return { server: "unavailable", clientContent: "unknown",
          reason: "__s2_shared_entity_switch: unavailable" };
      }
      if (!ctxState.isReady()) {
        return { server: "not-ready", clientContent: "unknown", reason: ctxState.notReadyReason() };
      }
      var ent = bindEntity(ctxState.findEntity(layout));
      if (!ent) {
        return { server: "not-ready", clientContent: "unknown", reason: ctxState.notReadyReason() };
      }
      return { server: "ready", clientContent: "unknown", reason: null };
    };
    api.forSlot = function (slot) {
      var client = resolveCurrentClient(slot);
      var view = slotViews[slot];
      if (view && view.isValid()) return view;
      var capturedSlotEpoch = slotEpochs[slot] || 0;
      function isValid() {
        if (!client || !client.isValid()) return false;
        var occupant = clientsApi().fromSlot(slot);
        if (!sameClient(client, occupant)) return false;
        return capturedSlotEpoch === (slotEpochs[slot] || 0);
      }
      view = {
        slot: slot,
        isValid: isValid,
        show: function (panelId, opts) { return api.show(slot, panelId, opts); },
        tryShow: function (panelId, opts) { return api.tryShow(slot, panelId, opts); },
        hide: function (panelId) { return api.hide(slot, panelId); },
        cursor: function (on) { return api.cursor(slot, on); },
        set: function (id, value) { return api.set(slot, id, value); },
        setText: function (panelId, value) { return api.setText(slot, panelId, value); },
        setClass: function (panelId, className, on) { return api.setClass(slot, panelId, className, on); },
        setMeter: function (meterName, percent) { return api.setMeter(slot, meterName, percent); },
        setPool: function (poolName, entries) { return api.setPool(slot, poolName, entries); },
        setDisabled: function (buttonId, on) { return api.setDisabled(slot, buttonId, on); },
        forget: function () { api.forget(slot, client); }
      };
      Object.keys(view).forEach(function (name) {
        if (name === "isValid" || typeof view[name] !== "function") return;
        var call = view[name];
        view[name] = function () {
          if (!isValid()) {
            if (name === "forget") return undefined;
            if (name === "tryShow") return uiFail("StaleClient", "stale client");
            return "stale client";
          }
          var previous = boundClient; boundClient = client;
          try { return call.apply(view, arguments); } finally { boundClient = previous; }
        };
      });
      slotViews[slot] = view;
      return view;
    };
    // Internal component seam. The binding carries the host-minted Client handle and low-level
    // view epochs, so component.js never has to recreate identity from slot or SteamID. A derived
    // binding may add `_componentIsValid`; `_withBinding` keeps that whole fence active through
    // primitive coercion/native calls and revalidates before publishing primitive cache state.
    api._captureBinding = function (slot) {
      var view = api.forSlot(slot);
      return { slot: slot, client: resolveCurrentClient(slot), view: view,
        slotEpoch: componentSlotEpochs[slot] || 0, entityEpoch: entityEpoch };
    };
    api._bindingIsValid = bindingIsValid;
    api._withBinding = function (binding, fn) {
      if (!bindingIsValid(binding)) return "stale client";
      var previous = boundClient, previousBinding = boundBinding;
      boundClient = binding.client;
      boundBinding = binding;
      try {
        var result = fn();
        return bindingIsValid(binding) ? result : "stale client";
      } finally {
        boundClient = previous;
        boundBinding = previousBinding;
      }
    };
    function invalidateBoundRoot(slot, root) {
      root = String(root);
      var prefix = slotPrefix(slot);
      function under(id) { return id === root || id.indexOf(root + "_") === 0; }
      for (var key in lastValue) {
        if (key.indexOf(prefix) === 0 && under(key.slice(prefix.length).split("|")[1])) delete lastValue[key];
      }
      for (var panel in visiblePanels) {
        if (panel.indexOf(prefix) === 0 && under(panel.slice(prefix.length))) delete visiblePanels[panel];
      }
      for (var meter in meterClass) {
        if (meter.indexOf(prefix) === 0 && under(meter.slice(prefix.length))) delete meterClass[meter];
      }
      var dis = disabled[slot];
      if (dis) for (var id in dis) { if (under(id)) delete dis[id]; }
    }
    // Private game adapter. Opaque tokens and retirement remain owned by the native ledger.
    api._focus = {
      reserve: function (binding, root, priority) {
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        if (typeof globalThis.__s2_surface_reserve !== "function" ||
            typeof globalThis.__s2_surface_state !== "function" ||
            typeof globalThis.__s2_surface_activate !== "function" ||
            typeof globalThis.__s2_surface_active !== "function" ||
            typeof globalThis.__s2_surface_release !== "function") {
          return uiFail("Unavailable", "surface focus is unavailable");
        }
        var ent = bindEntity(ctxState.ensureEntity(layout));
        if (!ent) return uiFail("NotReady", ctxState.notReadyReason());
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        var result = globalThis.__s2_surface_reserve("cs2:hudkit:exclusive", ent.index, ent.id,
          binding.slot, priority, JSON.stringify({
            capture: { call: "setInputCaptureEnabledForPlayer", token: "panel:" + root },
            suspend: { call: "setHasClassForPlayer", args: [root, layout.hideClass, CLASS_HAS] }
          }));
        if (!bindingIsValid(binding)) {
          if (result.ok) api._focus.release(result.value);
          return uiFail("StaleClient", "stale client or component");
        }
        return result;
      },
      reserveLinked: function (binding, root, priority, parentToken) {
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        if (typeof globalThis.__s2_surface_reserve_linked !== "function" ||
            typeof globalThis.__s2_surface_state !== "function" ||
            typeof globalThis.__s2_surface_activate !== "function" ||
            typeof globalThis.__s2_surface_active !== "function" ||
            typeof globalThis.__s2_surface_release !== "function") {
          return uiFail("Unavailable", "surface focus is unavailable");
        }
        var ent = bindEntity(ctxState.ensureEntity(layout));
        if (!ent) return uiFail("NotReady", ctxState.notReadyReason());
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        var result = globalThis.__s2_surface_reserve_linked("cs2:hudkit:exclusive", ent.index, ent.id,
          binding.slot, priority, JSON.stringify({
            capture: { call: "setInputCaptureEnabledForPlayer", token: "panel:" + root },
            suspend: { call: "setHasClassForPlayer", args: [root, layout.hideClass, CLASS_HAS] }
          }), parentToken);
        if (!bindingIsValid(binding)) {
          if (result.ok) api._focus.release(result.value);
          return uiFail("StaleClient", "stale client or component");
        }
        return result;
      },
      state: function (token) {
        return typeof globalThis.__s2_surface_state === "function" ?
          globalThis.__s2_surface_state(token) : "invalid";
      },
      activate: function (binding, token) {
        return bindingIsValid(binding) && typeof globalThis.__s2_surface_activate === "function" &&
          globalThis.__s2_surface_activate(token) && bindingIsValid(binding);
      },
      active: function (binding, token) {
        return bindingIsValid(binding) && typeof globalThis.__s2_surface_active === "function" &&
          globalThis.__s2_surface_active(token) && bindingIsValid(binding);
      },
      release: function (token) {
        return typeof globalThis.__s2_surface_release === "function" && globalThis.__s2_surface_release(token);
      },
      invalidate: function (binding, root) {
        if (!bindingIsValid(binding)) return;
        invalidateBoundRoot(binding.slot, root);
      },
      onFrame: ctxState.onFocusFrame
    };
    function surfaceAdapters(roots, profile) {
      var adapters = [];
      for (var i = 0; i < roots.length; i++) {
        var root = String(roots[i]);
        if (profile === "occupancy") { adapters.push({}); continue; }
        var adapter = {
          suspend: { call: "setHasClassForPlayer", args: [root, layout.hideClass, CLASS_HAS] }
        };
        if (profile === "interactive") {
          adapter.capture = { call: "setInputCaptureEnabledForPlayer", token: "panel:" + root };
        }
        adapters.push(adapter);
      }
      return adapters;
    }
    api._surface = {
      reserve: function (binding, key, mode, roots, profile) {
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        if (typeof globalThis.__s2_surface_reserve_owned !== "function" ||
            typeof globalThis.__s2_surface_state !== "function" ||
            typeof globalThis.__s2_surface_activate !== "function" ||
            typeof globalThis.__s2_surface_active !== "function" ||
            typeof globalThis.__s2_surface_release !== "function") {
          return uiFail("Unavailable", "surface ownership is unavailable");
        }
        var ent = bindEntity(ctxState.ensureEntity(layout));
        if (!ent) return uiFail("NotReady", ctxState.notReadyReason());
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        var adapters = surfaceAdapters(roots, profile);
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        var result = globalThis.__s2_surface_reserve_owned(key, ent.index, ent.id, binding.slot,
          mode, JSON.stringify(adapters));
        if (!bindingIsValid(binding)) {
          if (result.ok) api._surface.release(result.value.token);
          return uiFail("StaleClient", "stale client or component");
        }
        return result;
      },
      clearLegacy: function (binding, key, roots, profile) {
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        if (typeof globalThis.__s2_surface_clear_legacy !== "function") {
          return uiFail("Unavailable", "surface ownership is unavailable");
        }
        var ent = bindEntity(ctxState.ensureEntity(layout));
        if (!ent) return uiFail("NotReady", ctxState.notReadyReason());
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        var adapters = surfaceAdapters(roots, profile);
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        var result = globalThis.__s2_surface_clear_legacy(key, ent.index, ent.id, binding.slot,
          JSON.stringify(adapters));
        // The host may have hidden free or legacy lanes before returning a partial failure. Those
        // writes bypass this context's mirrors, so none of the supplied roots remains authoritative.
        for (var i = 0; i < roots.length; i++) invalidateBoundRoot(binding.slot, roots[i]);
        if (!bindingIsValid(binding)) return uiFail("StaleClient", "stale client or component");
        return result.ok ? uiOk(undefined) : result;
      },
      state: api._focus.state,
      activate: api._focus.activate,
      active: api._focus.active,
      release: api._focus.release,
      invalidate: api._focus.invalidate
    };
    api._disconnectOwnsSlot = function (slot, client) {
      if (!client || !sameClient(slotClients[slot], client)) return false;
      var occupant = clientsApi().fromSlot(slot);
      return !occupant || sameClient(occupant, client);
    };
    // Direct slot APIs adopt the current occupant; retained forSlot views keep their original one.
    // Structured drives already capture and bind that occupant. Click dispatch has no drive result,
    // so keep its existing boolean stale-client adapter here.
    var dispatchClick = api.dispatchClick;
    api.dispatchClick = function (slot) {
      if (!currentClient(slot)) return false;
      return dispatchClick.apply(api, arguments);
    };
    return api;
  }

  globalThis.__s2pkg_game_ctx = Object.assign({}, globalThis.__s2pkg_game_ctx, {
    ui: function (reg, viaId) {
      var ready = false;
      var entityByResource = {};
      var registered = {};
      var hudByResource = {};
      var buttonHandlers = {};
      var rawClickHandlers = [];
      var mamBannerShown = false;
      var focusReconcilers = [];
      reg(viaId(function () {
        var frame = globalThis.__s2pkg_frame;
        if (frame) return frame.OnGameFrame.subscribe(function () {
          var pending = focusReconcilers.slice();
          for (var i = 0; i < pending.length; i++) pending[i]();
        }, { phase: "pre" });
      }));
      reg(viaId(function () {
        return __s2_hook_on("@s2script/cs2", "onCustomHudClicked", function (view) {
          var clicker = resolveClicker(view.player);
          var slot = clicker ? clicker.slot : -1;
          if (slot >= 0) {
            for (var res in hudByResource) {
              if (Object.prototype.hasOwnProperty.call(hudByResource, res)) {
                hudByResource[res].dispatchClick(slot, view.buttonId);
              }
            }
          }
          var rawSnapshot = rawClickHandlers.slice();
          for (var r = 0; r < rawSnapshot.length; r++) {
            rawSnapshot[r]({ player: view.player, buttonId: view.buttonId, slot: slot });
          }
          return 0;
        });
      }));

      function notReadyReason() {
        return ready
          ? "custom_hud_layout entity unavailable (stale or create failed)"
          : "world not ready — wait for an active client before driving HUDs";
      }

      function remember(desc) {
        registered[desc.resource] = desc;
      }

      function spawnRegistered() {
        if (!ready) return;
        var st = ctxState();
        for (var res in registered) {
          if (Object.prototype.hasOwnProperty.call(registered, res)) {
            st.createEntity(registered[res]);
          }
        }
      }

      function becomeReady() {
        ready = true;
        spawnRegistered();
      }

      function markReadyIfActive() {
        var all = clientsApi().all();
        for (var i = 0; i < all.length; i++) {
          if (all[i].signonState === SIGNON_ACTIVE) { becomeReady(); return; }
        }
      }

      function resetForMap() {
        ready = false;
        entityByResource = {};
        // The entities died with the map; so did everything the paint caches described. This is
        // a belt over bindEntity's identity gate (which also catches the replacement on its
        // first resolve): clearing here drops obsolete paint caches immediately. The host resets
        // capture leases before the map callback runs.
        for (var res in hudByResource) {
          if (Object.prototype.hasOwnProperty.call(hudByResource, res)) {
            hudByResource[res].resetEntityCaches();
          }
        }
      }

      reg(viaId(function () { serverApi().onMapStart(resetForMap); }));
      reg(viaId(function () {
        clientsApi().onActive(function () { becomeReady(); });
        clientsApi().onDisconnect(function (client) {
          for (var res in hudByResource) {
            if (Object.prototype.hasOwnProperty.call(hudByResource, res)) {
              hudByResource[res].forget(client.slot, client);
            }
          }
        });
        markReadyIfActive();
      }));

      function ctxState() {
        return {
          buttonHandlers: buttonHandlers,
          onFocusFrame: function (fn) { focusReconcilers.push(fn); },
          notReadyReason: notReadyReason,
          isReady: function () { return ready; },
          findEntity: function (desc) {
            var tn = targetNameForResource(desc.resource);
            var found = entityApi().Entity.findByClass(HUD_CLASS);
            for (var i = 0; i < found.length; i++) {
              if (found[i].name === tn && found[i].isValid()) return found[i];
            }
            var cached = entityByResource[desc.resource];
            return cached && cached.isValid() ? cached : null;
          },
          /** Resolve the layout entity for a drive. Creation happens in createEntity, not here. */
          ensureEntity: function (desc) {
            if (!ready) return null;
            var cached = entityByResource[desc.resource];
            if (cached && cached.isValid()) return cached;
            var existing = this.findEntity(desc);
            if (existing) { entityByResource[desc.resource] = existing; return existing; }
            return null;
          },

          /**
           * Spawn the layout entity once the world has an active client. Safe from player-join,
           * game events, commands, and hud() after that point. OnMapStart is still too early —
           * becomeReady waits for a SIGNON_ACTIVE client.
           */
          createEntity: function (desc) {
            if (!ready) return null;
            var already = this.ensureEntity(desc);
            if (already) return already;
            var vxmlErr = rejectVxml(desc.resource);
            if (vxmlErr) return null;
            var ref = entityApi().createEntity(HUD_CLASS, {
              targetname: targetNameForResource(desc.resource),
              origin: "0 0 0",
              layout: desc.resource
            });
            if (!ref || !ref.isValid()) return null;
            entityByResource[desc.resource] = ref;
            return ref;
          }
        };
      }

      function maybePrintMamBanner(desc) {
        if (mamBannerShown) return;
        mamBannerShown = true;
        var req = desc.addons.join(", ");
        warn("[cs2/ui] required workshop addons: " + req);
        warn("[cs2/ui] set mm_extra_addons to that list in game/csgo/cfg/multiaddonmanager/multiaddonmanager.cfg");
        var mm = parseMmAddons(serverApi().getCvar("mm_extra_addons"));
        if (!mm.present) {
          warn("[cs2/ui] MultiAddonManager not detected (mm_extra_addons empty). Clients must already be subscribed to " + req + ".");
          warn("[cs2/ui] +host_workshop_map will not deliver this content addon (IsPlayable=false, no map inside).");
          return;
        }
        if (mm.missing.length === 0) {
          warn("[cs2/ui] MAM: present (" + req + " listed)");
        } else {
          warn("[cs2/ui] MAM: present (" + req + " MISSING — listed: " + mm.listed.join(",") + ")");
        }
      }

      function getLayout(desc) {
        maybePrintMamBanner(desc);
        remember(desc);
        if (!hudByResource[desc.resource]) {
          hudByResource[desc.resource] = makeHud(desc, ctxState());
        }
        if (ready) ctxState().createEntity(desc);
        return hudByResource[desc.resource];
      }

      function onClicked(handler) {
        rawClickHandlers.push(handler);
      }

      return {
        PROBE: DEFAULT_DESCRIPTOR,
        create: function (spec) {
          if (spec == null) fail("a layout spec is required (use CustomHudLayout.probe() for the workshop probe)");
          return getLayout(validateDescriptor(spec));
        },
        probe: function () {
          return getLayout(validateDescriptor(DEFAULT_DESCRIPTOR));
        },
        onClicked: onClicked,
        /**
         * Spawn the layout entity for `descriptor` (probe layout if omitted).
         *
         * Call from player-join, a game event, a command, or any other callback after a client
         * is active. Returns null on success, or a reason (the world is not ready yet, or spawn
         * failed). Idempotent. `create()` / `kit` also spawn once a client is active.
         */
        createLayout: function (descriptor) {
          var desc = validateDescriptor(descriptor || DEFAULT_DESCRIPTOR);
          remember(desc);
          var st = ctxState();
          var ref = st.createEntity(desc);
          return ref ? null : st.notReadyReason();
        },
        hud: function (descriptor) {
          return getLayout(validateDescriptor(descriptor || DEFAULT_DESCRIPTOR));
        },
        onCustomHudClicked: onClicked
      };
    }
  });
  if (typeof globalThis.__s2_game_ns === "function" && globalThis.__s2pkg_cs2) {
    var ns = globalThis.__s2_game_ns("ui");
    globalThis.__s2pkg_cs2.ui = ns;
    globalThis.__s2pkg_cs2.CustomHudLayout = ns;
    globalThis.__s2pkg_cs2.PROBE_LAYOUT = DEFAULT_DESCRIPTOR;
    globalThis.__s2pkg_cs2.DEFAULT_HUD_DESCRIPTOR = DEFAULT_DESCRIPTOR;
  }
})();
