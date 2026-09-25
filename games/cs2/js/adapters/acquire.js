// legacy.acquire.v1 — the CS2 pickup-gate adapter on the S2 engine-function service.
//
// Game semantics live HERE, in reviewed package JavaScript, never in core or the shim: core runs
// this adapter's `pre`/`post` for the trusted `canAcquire` binding and hands it a cursor over the
// plugin subscribers. The fold below is the frozen contract (games/cs2/adapters/contracts/
// legacy.acquire.v1.json, locked by hash):
//
//   * a Changed handler votes the CURRENT shared `result`; a Handled/Stop handler votes the result
//     it WROTE itself, or the implicit deny 1 when it wrote nothing; Continue does not vote;
//   * Handled/Stop votes sort ahead of Changed votes, stable in registration order within each;
//   * any nonzero result (a deny) beats Allowed 0, and the first deny in that order wins;
//   * any Handled/Stop skips the original and returns the folded vote;
//   * otherwise a vote is carried to POST as a proposal and combined with the ENGINE result by the
//     same deny rule — only when the original actually ran — overriding only when it changes.
//
// This file is concatenated ahead of the package bootstrap inputs and runs once per plugin
// context, while the host's bootstrap-only natives are still installed. It captures them, registers
// the adapter, and publishes a narrow subscribe helper at globalThis.__s2pkg_cs2_adapters.acquire
// for pawn.js's ctx.items wrapper. Pointer-free: the item-services receiver and the 4th argument
// are native-only positions; the item is a borrowed record exposing only defIndex.
(function () {
  "use strict";
  var ID = "legacy.acquire.v1";
  var FUNCTION = "canAcquire";
  var ALLOWED = 0;
  var IMPLICIT_DENY = 1;
  var CONTINUE = 0, CHANGED = 1, HANDLED = 2, STOP = 3;

  function mostRestrictive(first, second) { return first !== ALLOWED ? first : second; }

  // votes: [{ result, skip }] in delivery (= registration) order. Returns null when nobody voted.
  function fold(votes) {
    var ordered = [], i;
    for (i = 0; i < votes.length; i++) if (votes[i].skip) ordered.push(votes[i]);
    for (i = 0; i < votes.length; i++) if (!votes[i].skip) ordered.push(votes[i]);
    if (ordered.length === 0) return null;
    var folded = ordered[0].result, skip = false;
    for (i = 0; i < ordered.length; i++) {
      if (i > 0) folded = mostRestrictive(folded, ordered[i].result);
      if (ordered[i].skip) skip = true;
    }
    return { result: folded, skip: skip };
  }

  // Legacy HookResult reading: a non-number, a throw, or an out-of-range number is Continue.
  function hookResult(value) {
    if (typeof value !== "number") return CONTINUE;
    var n = value >>> 0;
    return n === CHANGED || n === HANDLED || n === STOP ? n : CONTINUE;
  }

  // A result write is an i32; anything else is REFUSED (never coerced into a vote).
  function asResult(value) {
    var n = Number(value);
    if (!(n >= -2147483648 && n <= 2147483647)) return null;
    return n | 0;
  }

  var adapter = {
    pre: function (d) {
      var votes = [], strongest = CONTINUE, delivery;
      while ((delivery = d.cursor.invokeNext()) !== null) {
        if (delivery.action === CHANGED) {
          votes.push({ result: d.frame.result, skip: false });
        } else if (delivery.action === HANDLED || delivery.action === STOP) {
          // The subscriber wrapper already resolved "its own write, else the implicit deny".
          votes.push({ result: delivery.returnValue, skip: true });
        } else {
          continue;
        }
        if (delivery.action > strongest) strongest = delivery.action;
      }
      var folded = fold(votes);
      if (folded === null) return CONTINUE;
      if (folded.skip) return { action: strongest, returnValue: folded.result };
      // Changed only: the original runs and POST combines this vote with the engine result.
      return { action: CHANGED, returnValue: folded.result };
    },
    post: function (d) {
      var f = d.frame;
      if ("proposedReturn" in f && !f.skipped) {
        var engine = f.originalReturnValue;
        var effective = mostRestrictive(f.proposedReturn, engine);
        if (effective !== engine) f.overrideReturn(effective);
      }
      // Observers see the effective return at this position in the hook chain.
      while (d.cursor.invokeNext() !== null) {}
    }
  };

  // `player` hop: the receiver is ItemServices*, which is not an entity and never reaches JS. The
  // host answers "does this live pawn's m_pItemServices hold exactly the hidden receiver?" against
  // the LIVE schema offset; the first pawn that matches gives its controller. A miss is null.
  function hopPlayer(frame, pawns, missed) {
    for (var s = 0; s < pawns.maxPlayers; s++) {
      var pawn = pawns.forSlot(s);
      if (!pawn || !pawn.ref) continue;
      var related = false;
      try { related = frame.hiddenReferencedBy("itemServices", pawn.ref, "CBasePlayerPawn", "m_pItemServices") === true; }
      catch (e) { related = false; }
      if (related) return pawn.controller;
    }
    missed();
    return null;
  }

  // The public ctx.items view: {player, defIndex, method, result, skipped}. `player` is resolved
  // lazily on read; `defIndex` falls back to 0; only PRE can write `result`.
  function view(frame, post, state, env) {
    return {
      get player() { return hopPlayer(frame, env.pawns, env.missed); },
      get defIndex() {
        try {
          var item = frame.item;
          var n = item ? item.defIndex : 0;
          return typeof n === "number" ? n : 0;
        } catch (e) { return 0; }
      },
      get method() { return frame.method; },
      get result() { return post ? frame.returnValue : frame.result; },
      set result(v) {
        if (post) return;
        var n = asResult(v);
        if (n === null) {
          env.warn("onCanAcquire: result write refused (not an i32) — the vote keeps the prior value");
          return;
        }
        frame.result = n;
        state.wrote = true;
      },
      get skipped() { return post ? !!frame.skipped : false; }
    };
  }

  function wrapper(handler, post, env) {
    return function (frame) {
      var state = { wrote: false };
      var hr = CONTINUE;
      try {
        hr = hookResult(handler(view(frame, post, state, env)));
      } catch (e) {
        // Legacy: a throwing handler is Continue; its accepted writes stay on the shared frame.
        env.warn("onCanAcquire handler threw: " + (e && e.stack ? e.stack : e));
        hr = CONTINUE;
      }
      if (post) return undefined;
      if (hr === HANDLED || hr === STOP) {
        return { action: hr, returnValue: state.wrote ? frame.result : IMPLICIT_DENY };
      }
      return hr;
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
      log("[s2script] WARN: onCanAcquire player hop missed (ItemServices* matched no live pawn) — view.player is null; the hook still fires");
    }
    return Object.freeze({
      id: ID,
      functionName: FUNCTION,
      status: function () { return status; },
      // pawns: {maxPlayers, forSlot(slot) -> Pawn|null}. Returns the host receipt, or null (named
      // once per context) when the gate is unavailable.
      subscribe: function (phase, handler, pawns) {
        var post = phase === "post";
        try {
          if (status !== "available") throw new Error(status);
          return natives.subscribe(FUNCTION, ID, post ? "post" : "pre", wrapper(handler, post, { pawns: pawns, missed: missed, warn: warn }));
        } catch (e) {
          if (!warned) {
            warned = true;
            warn("ctx.items.onCanAcquire" + (post ? "Post" : "") + ": the pickup gate is unavailable, so this handler will not fire: " +
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
  globalThis.__s2pkg_cs2_adapters = Object.freeze(Object.assign({}, previous, { acquire: api }));
})();
