// cs2.weapon-can-use.v1 — whether a player may pick up a weapon, on the S2 engine-function service, over the trusted `weaponCanUse` binding
// (CCSPlayer_WeaponServices::CanUse: bool(WeaponServices*, CBasePlayerWeapon*)). A pawn asks it before touching a weapon on the ground, so a false leaves the weapon where it is.
//
// Game semantics live HERE, never in core or the shim. The frozen contract
// (games/cs2/adapters/contracts/cs2.weapon-can-use.v1.json, locked by hash) is chat-visibility's vote:
//
//   * Continue does not vote; Changed votes `allowed` if the handler wrote it, else does not vote;
//   * Handled/Stop vote the value the handler wrote, or false when it wrote none; Stop also ends
//     delivery;
//   * when anyone voted, the original is skipped and the result is false if any vote is false,
//     true otherwise; when nobody voted the engine decides as before;
//   * a throwing handler is Continue (its write is dropped with it).
//
// The receiver is WeaponServices*, which is not an entity: `player` hops through the pawns'
// m_pWeaponServices with hiddenReferencedBy, as acquire.js does for m_pItemServices. Publishes
// globalThis.__s2pkg_cs2_adapters.weaponCanUse for the `weapons` export in pawn.js.
(function () {
  "use strict";
  var ID = "cs2.weapon-can-use.v1";
  var FUNCTION = "weaponCanUse";
  var API = "weapons.onCanUse";
  var CONTINUE = 0, CHANGED = 1, HANDLED = 2, STOP = 3;

  function hookResult(value) {
    if (typeof value !== "number") return CONTINUE;
    var n = value >>> 0;
    return n >= CHANGED && n <= STOP ? n : CONTINUE;
  }

  var adapter = {
    pre: function (d) {
      var voted = false, allowed = true, strongest = HANDLED, delivery;
      while ((delivery = d.cursor.invokeNext()) !== null) {
        if (delivery.action !== HANDLED && delivery.action !== STOP) continue;
        voted = true;
        if (delivery.returnValue === false) allowed = false;
        if (delivery.action > strongest) strongest = delivery.action;
      }
      return voted ? { action: strongest, returnValue: allowed } : CONTINUE;
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

  function view(frame, state, env) {
    return {
      get player() { return hopPlayer(frame, env.pawns, env.missed); },
      get weapon() { return env.weapon(frame.weapon); },
      get allowed() { return state.wrote ? state.value : true; },
      set allowed(v) {
        if (typeof v !== "boolean") {
          env.warn(API + ": allowed write refused (not a boolean)");
          return;
        }
        state.value = v;
        state.wrote = true;
      }
    };
  }

  function wrapper(handler, env) {
    return function (frame) {
      var state = { wrote: false, value: true };
      var hr;
      try {
        hr = hookResult(handler(view(frame, state, env)));
      } catch (e) {
        env.warn(API + " handler threw: " + (e && e.stack ? e.stack : e));
        return CONTINUE;
      }
      if (hr === HANDLED || hr === STOP) return { action: hr, returnValue: state.wrote ? state.value : false };
      if (hr === CHANGED && state.wrote) return { action: HANDLED, returnValue: state.value };
      return CONTINUE;
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
      // pawns: {maxPlayers, forSlot(slot) -> Pawn|null}; weapon(ref) wraps a weapon EntityRef or
      // null. Returns the host receipt, or null (named once per context) when unavailable.
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
  globalThis.__s2pkg_cs2_adapters = Object.freeze(Object.assign({}, previous, { weaponCanUse: api }));
})();
