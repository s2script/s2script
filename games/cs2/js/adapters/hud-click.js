// legacy.hud-click.v1 — the CS2 custom-HUD click adapter on the S2 engine-function service.
//
// The frozen contract (games/cs2/adapters/contracts/legacy.hud-click.v1.json, locked by hash):
// each subscriber receives {player, buttonId}; the button id is COPIED by the host before any
// JavaScript runs (a `string-indirect` projection of the engine's CUtlString*); deliveries happen
// during the NATIVE PRE, BEFORE the original runs (the historical "post" label is only a label);
// the adapter's final action is always Continue so map cs_script handlers still run; a click that
// re-enters synchronously is an ordinary nested dispatch.
//
// Runs once per plugin context at package bootstrap (see acquire.js for the mechanism) and
// publishes globalThis.__s2pkg_cs2_adapters.hudClick for ui.js's ctx.ui wrapper.
(function () {
  "use strict";
  var ID = "legacy.hud-click.v1";
  var FUNCTION = "customHudClicked";

  var adapter = {
    pre: function (d) {
      while (d.cursor.invokeNext() !== null) {}
      return 0; // Continue: never suppress the engine's own click handling.
    }
  };

  function install(natives, hash, log) {
    var status = "available";
    function warn(message) { log("[s2script] WARN: " + message); }
    if (typeof natives.register !== "function" || typeof natives.subscribe !== "function") {
      status = "this host has no engine-function adapter service";
    } else if (typeof hash !== "string") {
      status = "adapter contract " + ID + " was not packaged";
    } else {
      try { natives.register(ID, hash, adapter); }
      catch (e) { status = "adapter registration failed: " + (e && e.message ? e.message : e); }
    }
    var warned = false;
    return Object.freeze({
      id: ID,
      functionName: FUNCTION,
      status: function () { return status; },
      // handler({player, buttonId}) — its return value is ignored (Continue).
      subscribe: function (handler) {
        try {
          if (status !== "available") throw new Error(status);
          return natives.subscribe(FUNCTION, ID, "pre", function (frame) {
            var click = {
              get player() { return frame.player; },
              get buttonId() { return frame.buttonId; }
            };
            try { handler(click); }
            catch (e) { warn("onCustomHudClicked handler threw: " + (e && e.stack ? e.stack : e)); }
            return 0;
          });
        } catch (e) {
          if (!warned) {
            warned = true;
            warn("ctx.ui click routing is unavailable, so HUD clicks will not be delivered: " +
              (e && e.message ? e.message : e));
          }
          return null;
        }
      }
    });
  }

  var contracts = globalThis.__s2_adapter_contracts;
  var api = install({
    register: globalThis.__s2_function_adapter_register,
    subscribe: globalThis.__s2_function_adapter_subscribe
  }, contracts && contracts[ID], function (message) { console.log(message); });
  var previous = globalThis.__s2pkg_cs2_adapters || {};
  globalThis.__s2pkg_cs2_adapters = Object.freeze(Object.assign({}, previous, { hudClick: api }));
})();
