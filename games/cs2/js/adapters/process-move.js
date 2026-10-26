// cs2.process-move.v1 — each player's movement tick, on the S2 engine-function service, over the
// trusted `processMove` binding (CCSPlayer_MovementServices::ProcessMove: void(MovementServices*,
// CMoveData* move)). It runs once per player per usercmd tick, between SetupMove and FinishMove.
//
// Game semantics live HERE, never in core or the shim. CMoveData's layout is data (package gamedata
// offsets named by the trusted functions file); its MEANING is this file. The frozen contract
// (games/cs2/adapters/contracts/cs2.process-move.v1.json, locked by hash):
//
//   * every subscriber runs, in subscription order; returns are ignored and movement always runs;
//   * `velocity` is writable during PRE only, so the engine moves the player with it this tick;
//     POST sees the move data after the original ran, and a POST assignment is ignored;
//   * a throwing handler is Continue and keeps the writes it made before throwing.
//
// The view is borrowed: it is valid only during the synchronous handler call. The receiver hop is
// weapon-can-use.js's, over m_pMovementServices. Publishes
// globalThis.__s2pkg_cs2_adapters.processMove for the `movement` export in pawn.js.
(function () {
  "use strict";
  var ID = "cs2.process-move.v1";
  var FUNCTION = "processMove";
  var CONTINUE = 0;
  var EXPIRED = "MoveView: expired borrowed view (valid only during the synchronous movement callback)";
  var F32_MAX = 3.4028234663852886e38;

  var adapter = {
    pre: function (d) {
      while (d.cursor.invokeNext() !== null) {}
      return CONTINUE;
    },
    post: function (d) {
      while (d.cursor.invokeNext() !== null) {}
    }
  };

  // The @s2script/math classes, when the context has them; plain {x, y, z} otherwise.
  function vector(x, y, z) {
    var m = globalThis.__s2pkg_math, V = m && m.Vector;
    return typeof V === "function" ? new V(x, y, z) : { x: x, y: y, z: z };
  }
  function qangle(x, y, z) {
    var m = globalThis.__s2pkg_math, Q = m && m.QAngle;
    return typeof Q === "function" ? new Q(x, y, z) : { x: x, y: y, z: z };
  }
  // A Vector-like {x,y,z} or an [x,y,z] triple; null when any component is not a finite f32.
  function components(v) {
    if (v === null || typeof v !== "object") return null;
    var c = Array.isArray(v) ? [+v[0], +v[1], +v[2]] : [+v.x, +v.y, +v.z];
    for (var i = 0; i < 3; i++) if (!isFinite(c[i]) || c[i] > F32_MAX || c[i] < -F32_MAX) return null;
    return c;
  }

  function hopPlayer(frame, pawns, missed) {
    for (var s = 0; s < pawns.maxPlayers; s++) {
      var pawn = pawns.forSlot(s);
      if (!pawn || !pawn.ref) continue;
      var related = false;
      try { related = frame.hiddenReferencedBy("movementServices", pawn.ref, "CBasePlayerPawn", "m_pMovementServices") === true; }
      catch (e) { related = false; }
      if (related) return pawn.controller;
    }
    missed();
    return null;
  }

  function moveView(frame, post, lease, env) {
    function guard() { if (!lease.live) throw new Error(EXPIRED); }
    function read(name) {
      guard();
      try {
        var move = frame.move;
        var v = move ? move[name] : undefined;
        return typeof v === "number" ? v : 0;
      } catch (e) { return 0; }
    }
    var view = {};
    Object.defineProperties(view, {
      player: { get: function () { guard(); return hopPlayer(frame, env.pawns, env.missed); }, enumerable: true },
      viewAngles: { get: function () { return qangle(read("pitch"), read("yaw"), read("roll")); }, enumerable: true },
      forwardMove: { get: function () { return read("forwardMove"); }, enumerable: true },
      sideMove: { get: function () { return read("sideMove"); }, enumerable: true },
      origin: { get: function () { return vector(read("originX"), read("originY"), read("originZ")); }, enumerable: true },
      velocity: {
        get: function () { return vector(read("velocityX"), read("velocityY"), read("velocityZ")); },
        set: function (v) {
          guard();
          if (post) return; // the original already ran: POST assignment is ignored
          var c = components(v);
          if (!c) {
            env.warn(env.api + ": velocity write refused (needs three finite numbers); the velocity keeps its prior value");
            return;
          }
          var move = frame.move;
          if (!move) return;
          move.velocityX = c[0]; move.velocityY = c[1]; move.velocityZ = c[2];
        },
        enumerable: true
      }
    });
    return view;
  }

  function wrapper(handler, post, env) {
    return function (frame) {
      var lease = { live: true };
      try { handler(moveView(frame, post, lease, env)); }
      catch (e) { env.warn(env.api + " handler threw: " + (e && e.stack ? e.stack : e)); }
      finally { lease.live = false; }
      return post ? undefined : CONTINUE;
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
      warn("movement player hop missed (MovementServices* matched no live pawn), so view.player is null; the hook still fires");
    }
    return Object.freeze({
      id: ID,
      functionName: FUNCTION,
      status: function () { return status; },
      subscribe: function (phase, handler, pawns) {
        var api = phase === "post" ? "movement.onProcessMovePost" : "movement.onProcessMove";
        try {
          if (status !== "available") throw new Error(status);
          return natives.subscribe(FUNCTION, ID, phase,
            wrapper(handler, phase === "post", { pawns: pawns, missed: missed, warn: warn, api: api }));
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
  globalThis.__s2pkg_cs2_adapters = Object.freeze(Object.assign({}, previous, { processMove: api }));
})();
