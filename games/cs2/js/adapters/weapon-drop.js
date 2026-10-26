// cs2.weapon-drop.v1 — whether a player's weapon drop happens, on the S2 engine-function service,
// over the trusted `weaponDrop` binding (CCSPlayer_WeaponServices::DropWeapon: void(WeaponServices*,
// CBasePlayerWeapon*, Vector* target, Vector* velocity)). Every drop goes through it: the drop key,
// death, and plugin calls.
//
// Game semantics live HERE, never in core or the shim. The frozen contract
// (games/cs2/adapters/contracts/cs2.weapon-drop.v1.json, locked by hash):
//
//   * Continue and Changed do not block;
//   * Handled/Stop skip the original, so the weapon stays held; Stop also ends delivery;
//   * a throwing handler is Continue.
//
// The receiver hop is weapon-can-use.js's. Publishes globalThis.__s2pkg_cs2_adapters.weaponDrop
// for the `weapons` export in pawn.js.
(function () {
  "use strict";
  var ID = "cs2.weapon-drop.v1";
  var FUNCTION = "weaponDrop";
  var API = "weapons.onDrop";
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
      // A void binding takes Handled/Stop as a plain action; there is no return value to vote.
      return strongest >= HANDLED ? strongest : CONTINUE;
    }
  };

  function hopPlayer(frame, pawns, missed) {
    for (var s = 0; s < pawns.maxPlayers; s++) {
      var pawn = pawns.forSlot(s);
      if (!pawn || !pawn.ref) continue;
      var related = false;
      try { related = frame.hiddenReferencedBy("weaponServices", pawn.ref, "CBasePlayerPawn", "m_pWeaponServices") === true; }
      catch (e) { related = false; }
      if (related) return pawn.controller;
    }
    missed();
    return null;
  }

  function wrapper(handler, env) {
    return function (frame) {
      var hr;
      try {
        hr = hookResult(handler({
          get player() { return hopPlayer(frame, env.pawns, env.missed); },
          get weapon() { return env.weapon(frame.weapon); }
        }));
      } catch (e) {
        env.warn(API + " handler threw: " + (e && e.stack ? e.stack : e));
        return CONTINUE;
      }
      return hr === HANDLED || hr === STOP ? hr : CONTINUE;
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
    var warned = false, hopWarned = false;
    function missed() {
      if (hopWarned) return;
      hopWarned = true;
      warn(API + " player hop missed (WeaponServices* matched no live pawn), so view.player is null; the hook still fires");
    }
    return Object.freeze({
      id: ID,
      functionName: FUNCTION,
      status: function () { return status; },
      subscribe: function (handler, pawns, weapon) {
        try {
          if (status !== "available") throw new Error(status);
          return natives.subscribe(FUNCTION, ID, "pre",
            wrapper(handler, { pawns: pawns, weapon: weapon, missed: missed, warn: warn }));
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
  globalThis.__s2pkg_cs2_adapters = Object.freeze(Object.assign({}, previous, { weaponDrop: api }));
})();
