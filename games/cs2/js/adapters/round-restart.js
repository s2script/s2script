// cs2.round-restart.v1 — the start of every round, on the S2 engine-function service, over the
// trusted `roundRestart` binding (CCSGameRules::RestartRound: void(CCSGameRules*)). It runs when a
// round begins after the round-end delay and on mp_restartgame; round_prestart fires from inside it.
//
// The frozen contract (games/cs2/adapters/contracts/cs2.round-restart.v1.json, locked by hash):
//
//   * every subscriber runs, in subscription order, with no arguments; returns are ignored and the
//     round always restarts;
//   * PRE runs before the engine resets the round, POST after it respawned players and reset the map;
//   * a throwing handler is Continue.
//
// Publishes globalThis.__s2pkg_cs2_adapters.roundRestart for GameRules.onRoundRestart in pawn.js.
(function () {
  "use strict";
  var ID = "cs2.round-restart.v1";
  var FUNCTION = "roundRestart";
  var CONTINUE = 0;

  var adapter = {
    pre: function (d) {
      while (d.cursor.invokeNext() !== null) {}
      return CONTINUE;
    },
    post: function (d) {
      while (d.cursor.invokeNext() !== null) {}
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
      subscribe: function (phase, handler) {
        var post = phase === "post";
        var api = post ? "GameRules.onRoundRestartPost" : "GameRules.onRoundRestart";
        try {
          if (status !== "available") throw new Error(status);
          return natives.subscribe(FUNCTION, ID, phase, function () {
            try { handler(); }
            catch (e) { warn(api + " handler threw: " + (e && e.stack ? e.stack : e)); }
            return post ? undefined : CONTINUE;
          });
        } catch (e) {
          if (!warned) {
            warned = true;
            warn(api + ": this hook is unavailable, so this handler will not fire: " +
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
  globalThis.__s2pkg_cs2_adapters = Object.freeze(Object.assign({}, previous, { roundRestart: api }));
})();
