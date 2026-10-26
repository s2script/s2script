// cs2.chat-visibility.v1 — who can read whose chat, on the S2 engine-function service, over the
// trusted `playerCanHearChat` binding (CCSGameRules::PlayerCanHearChat:
// bool(gameRules*, receiver controller*, sender controller*, bool teamOnly)). Host_Say calls it once
// per recipient while it builds a chat message's recipient filter, so a false keeps that one player
// from seeing the message; the sender always sees their own.
//
// Game semantics live HERE, never in core or the shim. The frozen contract
// (games/cs2/adapters/contracts/cs2.chat-visibility.v1.json, locked by hash):
//
//   * Continue does not vote; Changed votes `canHear` if the handler wrote it, else does not vote;
//   * Handled/Stop vote the value the handler wrote, or false when it wrote none; Stop also ends
//     delivery;
//   * when anyone voted, the original is skipped and the result is false if any vote is false,
//     true otherwise; when nobody voted the engine decides as before;
//   * a throwing handler is Continue (its write is dropped with it).
//
// Runs once per plugin context at package bootstrap (see acquire.js for the mechanism) and
// publishes globalThis.__s2pkg_cs2_adapters.chatVisibility for the `chat` export in pawn.js.
(function () {
  "use strict";
  var ID = "cs2.chat-visibility.v1";
  var FUNCTION = "playerCanHearChat";
  var CONTINUE = 0, CHANGED = 1, HANDLED = 2, STOP = 3;

  // Legacy HookResult reading: a non-number, or anything but 1..3 after ToUint32, is Continue.
  function hookResult(value) {
    if (typeof value !== "number") return CONTINUE;
    var n = value >>> 0;
    return n >= CHANGED && n <= STOP ? n : CONTINUE;
  }

  var adapter = {
    pre: function (d) {
      var voted = false, canHear = true, strongest = HANDLED, delivery;
      // The host cursor already ends delivery after a Stop.
      while ((delivery = d.cursor.invokeNext()) !== null) {
        if (delivery.action !== HANDLED && delivery.action !== STOP) continue;
        voted = true;
        if (delivery.returnValue === false) canHear = false;
        if (delivery.action > strongest) strongest = delivery.action;
      }
      return voted ? { action: strongest, returnValue: canHear } : CONTINUE;
    }
  };

  function view(frame, state, env) {
    return {
      get receiver() { return env.player(frame.receiver); },
      get sender() { return env.player(frame.sender); },
      get teamOnly() { return frame.teamOnly === true; },
      get canHear() { return state.wrote ? state.value : true; },
      set canHear(v) {
        if (typeof v !== "boolean") {
          env.warn("chat.onCanHear: canHear write refused (not a boolean)");
          return;
        }
        state.value = v;
        state.wrote = true;
      }
    };
  }

  // A vote reaches the adapter as a Handled/Stop delivery carrying the voted value; everything
  // else is Continue. Changed is the "vote only if written" spelling of Handled.
  function wrapper(handler, env) {
    return function (frame) {
      var state = { wrote: false, value: true };
      var hr;
      try {
        hr = hookResult(handler(view(frame, state, env)));
      } catch (e) {
        env.warn("chat.onCanHear handler threw: " + (e && e.stack ? e.stack : e));
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
    var warned = false;
    return Object.freeze({
      id: ID,
      functionName: FUNCTION,
      status: function () { return status; },
      // player(ref) wraps a controller EntityRef (or null). Returns the host receipt, or null
      // (named once per context) when the hook is unavailable.
      subscribe: function (handler, player) {
        try {
          if (status !== "available") throw new Error(status);
          return natives.subscribe(FUNCTION, ID, "pre", wrapper(handler, { player: player, warn: warn }));
        } catch (e) {
          if (!warned) {
            warned = true;
            warn("chat.onCanHear: chat visibility is unavailable, so this handler will not fire: " +
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
  globalThis.__s2pkg_cs2_adapters = Object.freeze(Object.assign({}, previous, { chatVisibility: api }));
})();
