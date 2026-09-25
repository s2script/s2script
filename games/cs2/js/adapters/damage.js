// legacy.damage.v1 — CS2 damage (SDKHook OnTakeDamage / OnTakeDamagePost) on the S2 engine-function
// service, over the trusted `takeDamageOld` binding (CBaseEntity::TakeDamageOld:
// void(victim*, mutable CTakeDamageInfo*, optional result*)).
//
// Game semantics live HERE, never in core or the shim. CTakeDamageInfo's layout is data (the trusted
// functions file names each field by schema offset key; the host selects the offsets at commit), and
// its MEANING is this file: the frozen contract (games/cs2/adapters/contracts/legacy.damage.v1.json,
// locked by hash) is the historical SDKHook damage behaviour:
//
//   * every OnTakeDamage handler whose hooked entity IS the victim runs, in subscription order;
//   * the collapsed result is the max HookResult; Stop ends delivery, Handled does not;
//   * a collapsed Handled/Stop BLOCKS by writing damage 0 after the fan-out — the engine original
//     always runs (both native phases return Ignore); nothing is ever superseded;
//   * `.damage` is writable during PRE only; POST (OnTakeDamagePost) sees the info after the original
//     ran, its return is ignored and its `.damage` assignment is silently ignored;
//   * a throwing handler is Continue and keeps the writes it made before throwing;
//   * the optional result storage is a native-only pass-through: never exposed, forwarded unchanged.
//
// The DamageInfo view is borrowed: it is valid only during the synchronous handler call. Reading it
// afterwards (including after an `await`) throws "expired borrowed view".
//
// Hand-off to the engine-generic SDK: the core prelude's SDKHook/SDKUnhook route a hook type to the
// provider the selected game package registers through `__s2_sdkhook_provider_register` during
// package bootstrap (the host deletes the registrar afterwards). This file registers the two damage
// types; every other SDKHook type stays on the core per-entity table.
(function () {
  "use strict";
  var ID = "legacy.damage.v1";
  var FUNCTION = "takeDamageOld";
  var CONTINUE = 0, HANDLED = 2;
  var EXPIRED = "DamageInfo: expired borrowed view (valid only during the synchronous SDKHook callback)";

  // Legacy HookResult reading: a non-number is Continue; ToUint32 and anything but 1..3 is Continue.
  function hookResult(value) {
    if (typeof value !== "number") return CONTINUE;
    var n = value >>> 0;
    return n >= 1 && n <= 3 ? n : CONTINUE;
  }

  var adapter = {
    pre: function (d) {
      var strongest = CONTINUE, delivery;
      // The host cursor already ends delivery after a Stop.
      while ((delivery = d.cursor.invokeNext()) !== null) {
        if (delivery.action > strongest) strongest = delivery.action;
      }
      if (strongest >= HANDLED) {
        var info = d.frame.info;
        if (info) info.damage = 0; // block-to-zero, staged and committed before the original
      }
      return CONTINUE; // never suppress: the engine original always runs
    },
    post: function (d) {
      while (d.cursor.invokeNext() !== null) {}
    }
  };

  // The public DamageInfo over one borrowed frame. `lease.live` is true only while the handler runs.
  function damageInfo(frame, post, lease) {
    function guard() { if (!lease.live) throw new Error(EXPIRED); }
    function field(name, fallback) {
      guard();
      try {
        var info = frame.info;
        if (!info) return fallback;
        var v = info[name];
        return v === undefined || v === null ? fallback : v;
      } catch (e) { return fallback; }
    }
    var view = {};
    Object.defineProperties(view, {
      damage: {
        get: function () { return field("damage", 0); },
        set: function (v) {
          guard();
          if (post) return; // the original already ran: POST assignment is ignored
          var info = frame.info;
          if (info) info.damage = +v;
        },
        enumerable: true, configurable: true,
      },
      damageType: { get: function () { return field("damageType", 0); }, enumerable: true, configurable: true },
      attacker: { get: function () { return field("attacker", null); }, enumerable: true, configurable: true },
      inflictor: { get: function () { return field("inflictor", null); }, enumerable: true, configurable: true },
      victim: {
        get: function () {
          guard();
          try { return frame.victim || null; } catch (e) { return null; }
        },
        enumerable: true, configurable: true,
      },
    });
    return view;
  }

  function wrapper(entityId, handler, post, warn, label) {
    return function (frame) {
      var victim = null;
      try { victim = frame.victim; } catch (e) { victim = null; }
      if (!victim || victim.id !== entityId) return post ? undefined : CONTINUE;
      var lease = { live: true };
      var hr = CONTINUE;
      try {
        hr = hookResult(handler(damageInfo(frame, post, lease)));
      } catch (e) {
        warn(label + " handler threw: " + (e && e.stack ? e.stack : e));
        hr = CONTINUE;
      } finally {
        lease.live = false;
      }
      return post ? undefined : hr;
    };
  }

  function install(natives, hash, log, entityValid) {
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
    // [{type, index, id, callback, receipt}] in subscription order, this plugin context only.
    var entries = [];
    var warned = false;
    function prune() {
      if (typeof entityValid !== "function") return;
      for (var i = entries.length - 1; i >= 0; i--) {
        var e = entries[i];
        var live = false;
        try { live = entityValid(e.index, e.id) === true; } catch (x) { live = false; }
        if (!live) {
          try { e.receipt.dispose(); } catch (x) { /* already gone with its owner */ }
          entries.splice(i, 1);
        }
      }
    }
    function provider(type) {
      var post = type === "OnTakeDamagePost";
      return Object.freeze({
        // (index, id) is an entity the SDK prelude already checked is live. Returns true once the
        // subscription receipt exists; false (named once per context) when damage is unavailable.
        hook: function (index, id, callback) {
          prune();
          var receipt = null;
          try {
            if (status !== "available") throw new Error(status);
            receipt = natives.subscribe(FUNCTION, ID, post ? "post" : "pre", wrapper(id, callback, post, warn, type));
          } catch (e) {
            if (!warned) {
              warned = true;
              warn("SDKHook " + type + ": damage hooks are unavailable, so this hook will not fire: " +
                (e && e.message ? e.message : e));
            }
            return false;
          }
          entries.push({ type: type, index: index, id: id, callback: callback, receipt: receipt });
          return true;
        },
        // Removes the FIRST matching registration (entity identity + callback identity), like SDKUnhook.
        unhook: function (index, id, callback) {
          for (var i = 0; i < entries.length; i++) {
            var e = entries[i];
            if (e.type === type && e.id === id && e.callback === callback) {
              entries.splice(i, 1);
              try { e.receipt.dispose(); } catch (x) { /* already gone with its owner */ }
              return true;
            }
          }
          return false;
        }
      });
    }
    return {
      id: ID,
      functionName: FUNCTION,
      status: function () { return status; },
      providers: { OnTakeDamage: provider("OnTakeDamage"), OnTakeDamagePost: provider("OnTakeDamagePost") },
      // Test/diagnostic seam: the number of live registrations in this context.
      count: function () { return entries.length; }
    };
  }

  var contracts = globalThis.__s2_adapter_contracts;
  var api = install({
    register: globalThis.__s2_function_adapter_register,
    subscribe: globalThis.__s2_function_adapter_subscribe
  }, contracts && contracts[ID], function (message) { console.log(message); }, globalThis.__s2_ent_ref_valid);
  var provide = globalThis.__s2_sdkhook_provider_register;
  if (typeof provide === "function") {
    for (var type in api.providers) {
      try { provide(type, api.providers[type]); }
      catch (e) { console.log("[s2script] WARN: SDKHook " + type + " provider refused: " + (e && e.message ? e.message : e)); }
    }
  }
  var previous = globalThis.__s2pkg_cs2_adapters || {};
  globalThis.__s2pkg_cs2_adapters = Object.freeze(Object.assign({}, previous, {
    damage: Object.freeze({ id: api.id, functionName: api.functionName, status: api.status, count: api.count })
  }));
})();
