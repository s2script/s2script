// cs2.join-team.v1 — whether a player's team request goes through, on the S2 engine-function
// service, over the trusted `joinTeam` binding (CCSPlayerController::HandleCommandJoinTeam:
// bool(CCSPlayerController*, int team, bool queue)). The jointeam command and the team menu both
// reach it; a plugin's own changeTeam/switchTeam calls do not.
//
// Game semantics live HERE, never in core or the shim. The frozen contract
// (games/cs2/adapters/contracts/cs2.join-team.v1.json, locked by hash):
//
//   * Continue and Changed do not block;
//   * Handled/Stop skip the original and answer false, so the player stays where they are; Stop
//     also ends delivery;
//   * a throwing handler is Continue.
//
// Publishes globalThis.__s2pkg_cs2_adapters.joinTeam for Teams.onJoinTeam in pawn.js.
(function () {
  "use strict";
  var ID = "cs2.join-team.v1";
  var FUNCTION = "joinTeam";
  var API = "Teams.onJoinTeam";
  var CONTINUE = 0, CHANGED = 1, HANDLED = 2, STOP = 3;

  function hookResult(value) {
    if (typeof value !== "number") return CONTINUE;
    var n = value >>> 0;
    return n >= CHANGED && n <= STOP ? n : CONTINUE;
  }

  var adapter = {
    pre: function (d) {
      var strongest = CONTINUE, delivery;
      while ((delivery = d.cursor.invokeNext()) !== null) {
        if (delivery.action > strongest) strongest = delivery.action;
      }
      return strongest >= HANDLED ? { action: strongest, returnValue: false } : CONTINUE;
    }
  };

  function wrapper(handler, env) {
    return function (frame) {
      var hr;
      try {
        hr = hookResult(handler({
          get player() { return env.player(frame.player); },
          get team() { return frame.team; },
          get queue() { return frame.queue; }
        }));
      } catch (e) {
        env.warn(API + " handler threw: " + (e && e.stack ? e.stack : e));
        return CONTINUE;
      }
      return hr >= HANDLED ? { action: hr, returnValue: false } : CONTINUE;
    };
  }

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
      // player(ref) wraps a controller EntityRef or null. Returns the host receipt, or null (named
      // once per context) when unavailable.
      subscribe: function (handler, player) {
        try {
          if (status !== "available") throw new Error(status);
          return natives.subscribe(FUNCTION, ID, "pre", wrapper(handler, { player: player, warn: warn }));
        } catch (e) {
          if (!warned) {
            warned = true;
            warn(API + ": this hook is unavailable, so this handler will not fire: " +
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
  globalThis.__s2pkg_cs2_adapters = Object.freeze(Object.assign({}, previous, { joinTeam: api }));
})();
