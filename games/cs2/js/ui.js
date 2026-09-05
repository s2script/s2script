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
    return pkg && pkg.call ? pkg.call(name) : null;
  }
  function engineStatus(name) {
    var pkg = callsApi();
    return pkg && pkg.status ? pkg.status(name) : "game calls unavailable";
  }

  var setHasClassForPlayer = engineCall("setHasClassForPlayer");
  var setDialogVariableStringForPlayer = engineCall("setDialogVariableStringForPlayer");
  var setInputCaptureEnabledForPlayer = engineCall("setInputCaptureEnabledForPlayer");

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

  function applyFields(fields, fn) {
    var first = null;
    for (var k in fields) {
      if (!Object.prototype.hasOwnProperty.call(fields, k)) continue;
      var err = fn(k, fields[k]);
      if (err && !first) first = err;
    }
    return first;
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

  function makeHud(desc, ctxState, onFirstClickHandler) {
    var layout = desc;
    var disabled = {};
    var handlers = {};
    var meterClass = {};
    var visiblePanels = {};
    var cursorLeases = {};
    var lastValue = {};
    var slotViews = {};

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

    function setClass(slot, panelId, className, on) {
      if (!setHasClassForPlayer) return "unavailable: " + engineStatus("setHasClassForPlayer");
      if (slot < 0) return "needs a player slot";
      var ent = ctxState.ensureEntity(layout);
      if (!ent) return ctxState.notReadyReason();
      var key = cacheKey(slot, "c", panelId, className);
      var s = on ? "1" : "0";
      if (lastValue[key] === s) return null;
      setHasClassForPlayer(ent, slot, panelId, className, on ? CLASS_HAS : CLASS_DOES_NOT_HAVE);
      lastValue[key] = s;
      return null;
    }

    function setDialogVariable(slot, panelId, variableName, value) {
      if (!setDialogVariableStringForPlayer) {
        return "unavailable: " + engineStatus("setDialogVariableStringForPlayer");
      }
      if (slot < 0) return "needs a player slot";
      var ent = ctxState.ensureEntity(layout);
      if (!ent) return ctxState.notReadyReason();
      var str = String(value);
      var key = cacheKey(slot, "v", panelId, variableName);
      if (lastValue[key] === str) return null;
      setDialogVariableStringForPlayer(ent, slot, panelId, variableName, str);
      lastValue[key] = str;
      return null;
    }

    function acquireCursor(slot, panelId) {
      if (!setInputCaptureEnabledForPlayer) {
        return "unavailable: " + engineStatus("setInputCaptureEnabledForPlayer");
      }
      var ent = ctxState.ensureEntity(layout);
      if (!ent) return ctxState.notReadyReason();
      var set = cursorLeases[slot];
      if (!set) { set = {}; cursorLeases[slot] = set; }
      var had = Object.keys(set).length > 0;
      set[panelId] = true;
      if (!had) setInputCaptureEnabledForPlayer(ent, slot, true);
      return null;
    }

    function releaseCursor(slot, panelId) {
      var set = cursorLeases[slot];
      if (!set || !set[panelId]) return null;
      delete set[panelId];
      if (Object.keys(set).length > 0) return null;
      if (!setInputCaptureEnabledForPlayer) return null;
      var ent = ctxState.findEntity(layout);
      if (!ent || !ent.isValid()) return null;
      setInputCaptureEnabledForPlayer(ent, slot, false);
      return null;
    }

    function trackVisible(slot, panelId, on) {
      var key = slotPrefix(slot) + panelId;
      if (on) visiblePanels[key] = true;
      else delete visiblePanels[key];
    }

    var api = { spec: layout, layout: layout };

    api.show = function (slot, panelId, opts) {
      opts = opts || {};
      var err = setClass(slot, panelId, layout.hideClass, false);
      if (err) return err;
      trackVisible(slot, panelId, true);
      if (opts.cursor) return acquireCursor(slot, panelId);
      return null;
    };
    api.hide = function (slot, panelId) {
      var err = setClass(slot, panelId, layout.hideClass, true);
      if (err) return err;
      trackVisible(slot, panelId, false);
      return releaseCursor(slot, panelId);
    };
    api.cursor = function (slot, on) {
      return on ? acquireCursor(slot, "*") : releaseCursor(slot, "*");
    };
    api.set = function (slot, id, value) {
      if (isFields(id)) {
        return applyFields(id, function (k, v) { return setDialogVariable(slot, k, k, v); });
      }
      return setDialogVariable(slot, id, id, value);
    };
    api.setText = function (slot, panelId, value) {
      if (isFields(panelId)) {
        return applyFields(panelId, function (k, v) { return api.setText(slot, k, v); });
      }
      var varName = layout.text[panelId] || panelId;
      return setDialogVariable(slot, panelId, varName, value);
    };
    api.setClass = setClass;
    api.setMeter = function (slot, meterName, percent) {
      var fillId = layout.meters[meterName];
      if (!fillId) return 'no meter "' + meterName + '" in this layout';
      var next = meterClassFor(percent);
      var key = slotPrefix(slot) + fillId;
      var prev = meterClass[key];
      if (prev && prev !== next) {
        var err = setClass(slot, fillId, prev, false);
        if (err) return err;
      }
      var applied = setClass(slot, fillId, next, true);
      if (!applied) meterClass[key] = next;
      return applied;
    };
    api.capacity = function (poolName) {
      var pool = layout.slots && layout.slots[poolName];
      return pool ? pool.length : 0;
    };
    api.setPool = function (slot, poolName, entries) {
      var pool = layout.slots && layout.slots[poolName];
      if (!pool) return 'no pool "' + poolName + '" in this layout';
      if (entries.length > pool.length) {
        return 'pool "' + poolName + '" holds ' + pool.length + ' slot(s); ' + entries.length +
          " given — paginate instead";
      }
      for (var i = 0; i < pool.length; i++) {
        var slotDef = pool[i];
        var row = entries[i];
        if (!row) {
          var hideErr = setClass(slot, slotDef.id, layout.hideClass, true);
          if (hideErr) return hideErr;
          continue;
        }
        var showErr = setClass(slot, slotDef.id, layout.hideClass, false);
        if (showErr) return showErr;
        for (var f = 0; f < slotDef.vars.length && f < row.length; f++) {
          var varErr = setDialogVariable(slot, slotDef.id, slotDef.vars[f], row[f]);
          if (varErr) return varErr;
        }
      }
      return null;
    };
    api.onClick = function (buttonId, handler) {
      if (ctxState.buttonHandlers[buttonId]) {
        throw new Error('CustomHud: conflicting handler for button id "' + buttonId + '"');
      }
      ctxState.buttonHandlers[buttonId] = handler;
      handlers[buttonId] = handler;
      onFirstClickHandler();
    };
    api.setDisabled = function (slot, buttonId, disabledOn) {
      var set = disabled[slot];
      if (!set) { set = {}; disabled[slot] = set; }
      if (disabledOn) set[buttonId] = true; else delete set[buttonId];
      return setClass(slot, buttonId, "s2-btn-disabled", disabledOn);
    };
    api.dispatchClick = function (slot, buttonId) {
      if (disabled[slot] && disabled[slot][buttonId]) return false;
      var h = handlers[buttonId];
      if (!h) return false;
      h(api.forSlot(slot));
      return true;
    };
    api.forget = function (slot) {
      delete disabled[slot];
      delete cursorLeases[slot];
      forgetKeyed(meterClass, slot);
      forgetKeyed(visiblePanels, slot);
      forgetKeyed(lastValue, slot);
    };
    /**
     * Drop EVERY cache that mirrors state living on the layout entity.
     *
     * Called when that entity goes away (a map change). All of these exist to suppress redundant
     * engine calls — `setText`/`setClass` return early when the cached value already matches — and
     * every one of them is a lie the moment a NEW entity is created, because the new entity has
     * every panel at its markup default and no input capture at all.
     *
     * Left stale, the effect is a PARTIAL PAINT: any value unchanged since the previous map is
     * suppressed and never re-sent, so a sheet draws its rows and not its buttons, or its title and
     * not its rows — intermittently, depending on what happened to differ. A surviving cursor lease
     * is worse than cosmetic: `releaseCursor` sees a non-empty lease set, concludes something still
     * wants the cursor, and never disables a capture the new entity never had — leaving a player
     * holding a pointer that no click can clear.
     *
     * `handlers` is deliberately NOT cleared: click handlers are registered once per button id at
     * claim time and belong to the plugin, not to the entity.
     */
    api.resetEntityCaches = function () {
      disabled = {};
      meterClass = {};
      visiblePanels = {};
      cursorLeases = {};
      lastValue = {};
    };
    api.ensure = function () {
      var ref = ctxState.createEntity(layout);
      return ref ? null : ctxState.notReadyReason();
    };
    api.forSlot = function (slot) {
      var view = slotViews[slot];
      if (view) return view;
      view = {
        slot: slot,
        show: function (panelId, opts) { return api.show(slot, panelId, opts); },
        hide: function (panelId) { return api.hide(slot, panelId); },
        cursor: function (on) { return api.cursor(slot, on); },
        set: function (id, value) { return api.set(slot, id, value); },
        setText: function (panelId, value) { return api.setText(slot, panelId, value); },
        setClass: function (panelId, className, on) { return api.setClass(slot, panelId, className, on); },
        setMeter: function (meterName, percent) { return api.setMeter(slot, meterName, percent); },
        setPool: function (poolName, entries) { return api.setPool(slot, poolName, entries); },
        setDisabled: function (buttonId, on) { return api.setDisabled(slot, buttonId, on); },
        forget: function () { api.forget(slot); }
      };
      slotViews[slot] = view;
      return view;
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
      var clickHookInstalled = false;
      var mamBannerShown = false;

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
        // The entity is gone; so is everything that described it. Without this the caches survive
        // into the next map and silently suppress the writes that would repaint it.
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
              hudByResource[res].forget(client.slot);
            }
          }
        });
        markReadyIfActive();
      }));

      function ctxState() {
        return {
          buttonHandlers: buttonHandlers,
          notReadyReason: notReadyReason,
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

      function installClickHook() {
        if (clickHookInstalled) return;
        clickHookInstalled = true;
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
            for (var r = 0; r < rawClickHandlers.length; r++) {
              rawClickHandlers[r]({ player: view.player, buttonId: view.buttonId, slot: slot });
            }
            return 0;
          });
        }));
      }

      function getLayout(desc) {
        maybePrintMamBanner(desc);
        remember(desc);
        if (!hudByResource[desc.resource]) {
          hudByResource[desc.resource] = makeHud(desc, ctxState(), installClickHook);
        }
        if (ready) ctxState().createEntity(desc);
        return hudByResource[desc.resource];
      }

      function onClicked(handler) {
        installClickHook();
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
