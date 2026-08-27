// @s2script/cs2 — lifecycle-bound custom HUD API (ctx.ui). ES5 IIFE concatenated after pawn.js.
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

  function validateDescriptor(desc) {
    if (!desc || typeof desc !== "object") throw new Error("ctx.ui.hud: descriptor must be an object");
    if (!Array.isArray(desc.addons) || desc.addons.length === 0) {
      throw new Error("ctx.ui.hud: descriptor.addons must be a non-empty string array");
    }
    for (var a = 0; a < desc.addons.length; a++) {
      if (!/^\d+$/.test(String(desc.addons[a]))) {
        throw new Error('ctx.ui.hud: addon id "' + desc.addons[a] + '" must be a decimal workshop id');
      }
    }
    if (typeof desc.resource !== "string" || !desc.resource) {
      throw new Error("ctx.ui.hud: descriptor.resource is required");
    }
    if (desc.resource.indexOf("panorama/layout/custom_game/") !== 0 || !/\.xml$/i.test(desc.resource)) {
      throw new Error('ctx.ui.hud: resource must be under panorama/layout/custom_game/ and end in .xml');
    }
    var vxmlErr = rejectVxml(desc.resource);
    if (vxmlErr) throw new Error("ctx.ui.hud: " + vxmlErr);
    if (typeof desc.hideClass !== "string" || !desc.hideClass) {
      throw new Error("ctx.ui.hud: descriptor.hideClass is required");
    }
    if (!Array.isArray(desc.buttons)) throw new Error("ctx.ui.hud: descriptor.buttons must be an array");
    var seenBtn = {};
    for (var b = 0; b < desc.buttons.length; b++) {
      var bid = desc.buttons[b];
      if (!bid) throw new Error("ctx.ui.hud: button ids must be non-empty");
      if (seenBtn[bid]) throw new Error('ctx.ui.hud: duplicate button id "' + bid + '"');
      seenBtn[bid] = true;
    }
    if (desc.slots) {
      for (var poolName in desc.slots) {
        if (!Object.prototype.hasOwnProperty.call(desc.slots, poolName)) continue;
        var pool = desc.slots[poolName];
        if (!Array.isArray(pool)) throw new Error('ctx.ui.hud: slots.' + poolName + " must be an array");
        for (var i = 0; i < pool.length; i++) {
          var slotDef = pool[i];
          if (!slotDef || !slotDef.id || !Array.isArray(slotDef.vars)) {
            throw new Error('ctx.ui.hud: slots.' + poolName + "[" + i + "] needs { id, vars[] }");
          }
        }
      }
    }
    return desc;
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

    function setClass(slot, panelId, className, on) {
      if (!setHasClassForPlayer) return "unavailable: " + engineStatus("setHasClassForPlayer");
      if (slot < 0) return "needs a player slot";
      var ent = ctxState.ensureEntity(layout);
      if (!ent) return ctxState.notReadyReason();
      setHasClassForPlayer(ent, slot, panelId, className, on ? CLASS_HAS : CLASS_DOES_NOT_HAVE);
      return null;
    }

    function setDialogVariable(slot, panelId, variableName, value) {
      if (!setDialogVariableStringForPlayer) {
        return "unavailable: " + engineStatus("setDialogVariableStringForPlayer");
      }
      if (slot < 0) return "needs a player slot";
      var ent = ctxState.ensureEntity(layout);
      if (!ent) return ctxState.notReadyReason();
      setDialogVariableStringForPlayer(ent, slot, panelId, variableName, String(value));
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
      var key = slot + ":" + panelId;
      if (on) visiblePanels[key] = true;
      else delete visiblePanels[key];
    }

    return {
      layout: layout,
      show: function (slot, panelId, opts) {
        opts = opts || {};
        var err = setClass(slot, panelId, layout.hideClass, false);
        if (err) return err;
        trackVisible(slot, panelId, true);
        if (opts.cursor) return acquireCursor(slot, panelId);
        return null;
      },
      hide: function (slot, panelId) {
        var err = setClass(slot, panelId, layout.hideClass, true);
        if (err) return err;
        trackVisible(slot, panelId, false);
        return releaseCursor(slot, panelId);
      },
      cursor: function (slot, on) {
        return on ? acquireCursor(slot, "*") : releaseCursor(slot, "*");
      },
      set: function (slot, id, value) {
        return setDialogVariable(slot, id, id, value);
      },
      setText: function (slot, panelId, value) {
        var varName = layout.text[panelId];
        if (!varName) return 'panel "' + panelId + '" declares no text variable in this layout';
        return setDialogVariable(slot, panelId, varName, value);
      },
      setClass: setClass,
      setMeter: function (slot, meterName, percent) {
        var fillId = layout.meters[meterName];
        if (!fillId) return 'no meter "' + meterName + '" in this layout';
        var next = meterClassFor(percent);
        var key = slot + ":" + fillId;
        var prev = meterClass[key];
        if (prev && prev !== next) {
          var err = setClass(slot, fillId, prev, false);
          if (err) return err;
        }
        err = setClass(slot, fillId, next, true);
        if (!err) meterClass[key] = next;
        return err;
      },
      capacity: function (poolName) {
        var pool = layout.slots && layout.slots[poolName];
        return pool ? pool.length : 0;
      },
      setPool: function (slot, poolName, entries) {
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
      },
      onClick: function (buttonId, handler) {
        if (ctxState.buttonHandlers[buttonId]) {
          throw new Error('ctx.ui: conflicting handler for button id "' + buttonId + '"');
        }
        ctxState.buttonHandlers[buttonId] = handler;
        handlers[buttonId] = handler;
        onFirstClickHandler();
      },
      setDisabled: function (slot, buttonId, disabledOn) {
        var set = disabled[slot];
        if (!set) { set = {}; disabled[slot] = set; }
        if (disabledOn) set[buttonId] = true; else delete set[buttonId];
        return setClass(slot, buttonId, "s2-btn-disabled", disabledOn);
      },
      dispatchClick: function (slot, buttonId) {
        if (disabled[slot] && disabled[slot][buttonId]) return false;
        var h = handlers[buttonId];
        if (!h) return false;
        h(slot);
        return true;
      },
      forget: function (slot) {
        delete disabled[slot];
        delete cursorLeases[slot];
        for (var key in meterClass) {
          if (Object.prototype.hasOwnProperty.call(meterClass, key) && key.indexOf(slot + ":") === 0) {
            delete meterClass[key];
          }
        }
        for (var vis in visiblePanels) {
          if (Object.prototype.hasOwnProperty.call(visiblePanels, vis) && vis.indexOf(slot + ":") === 0) {
            delete visiblePanels[vis];
          }
        }
      }
    };
  }

  globalThis.__s2pkg_game_ctx = Object.assign({}, globalThis.__s2pkg_game_ctx, {
    ui: function (reg, viaId) {
      var ready = false;
      var entityByResource = {};
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

      function markReadyIfActive() {
        var all = clientsApi().all();
        for (var i = 0; i < all.length; i++) {
          if (all[i].signonState === SIGNON_ACTIVE) { ready = true; return; }
        }
      }

      function resetForMap() {
        ready = false;
        entityByResource = {};
        for (var key in hudByResource) {
          if (Object.prototype.hasOwnProperty.call(hudByResource, key)) delete hudByResource[key];
        }
      }

      reg(viaId(function () { serverApi().onMapStart(resetForMap); }));
      reg(viaId(function () {
        clientsApi().onActive(function () { ready = true; });
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
          ensureEntity: function (desc) {
            if (!ready) return null;
            var cached = entityByResource[desc.resource];
            if (cached && cached.isValid()) return cached;
            var existing = this.findEntity(desc);
            if (existing) { entityByResource[desc.resource] = existing; return existing; }
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
              rawClickHandlers[r]({ player: view.player, buttonId: view.buttonId });
            }
            return 0;
          });
        }));
      }

      return {
        hud: function (descriptor) {
          var desc = validateDescriptor(descriptor || DEFAULT_DESCRIPTOR);
          maybePrintMamBanner(desc);
          if (!hudByResource[desc.resource]) {
            hudByResource[desc.resource] = makeHud(desc, ctxState(), installClickHook);
          }
          return hudByResource[desc.resource];
        },
        onCustomHudClicked: function (handler) {
          installClickHook();
          rawClickHandlers.push(handler);
        }
      };
    }
  });
})();
