// @s2script/cs2 — Tab-to-activate. One arm per slot; swallow IN_SCORE until the first
// rising edge fires onActivate, then Tab is the scoreboard again.
//
// The usercmd subscription runs this plugin's JS for every command of every player, so it is held
// only while some slot is armed: the first arm subscribes and the last disarm drops it.
(function () {
  var UserCmd = globalThis.__s2pkg_usercmd && globalThis.__s2pkg_usercmd.UserCmd;
  var Clients = globalThis.__s2pkg_clients && globalThis.__s2pkg_clients.Clients;
  var IN_SCORE = 1n << 16n;
  var states = {};
  var hookId = null;   // the usercmd subscription while any slot is armed

  function state(slot) {
    var s = states[slot];
    if (!s || (s.client && !s.client.isValid())) {
      s = { client: Clients && Clients.fromSlot(slot), armed: false, active: false, held: false, swallowHold: false, onActivate: null };
      states[slot] = s;
    }
    return s;
  }

  function anyArmed() {
    for (var k in states) if (Object.prototype.hasOwnProperty.call(states, k) && states[k].armed) return true;
    return false;
  }

  /** Drop the subscription once nothing is armed. Dispatch runs over a snapshot, so this is safe
   *  from inside onActivate. */
  function releaseHook() {
    if (hookId === null || hookId < 0 || anyArmed() || typeof globalThis.__s2_scope_dispose !== "function") return;
    globalThis.__s2_scope_dispose([hookId]);
    hookId = null;
  }

  function ensureHook() {
    if (hookId !== null || !UserCmd || typeof UserCmd.onRun !== "function") return;
    // While unhooked no command was seen, so what Tab was doing is unknown: start every slot from
    // "not held", as the very first hook does.
    for (var k in states) if (Object.prototype.hasOwnProperty.call(states, k)) { states[k].held = false; states[k].swallowHold = false; }
    var id = UserCmd.onRun(function (cmd, ctx) {
      var slot = ctx && ctx.slot;
      if (typeof slot !== "number" || !cmd) return;
      var s = state(slot);
      var down = (cmd.buttons & IN_SCORE) !== 0n;
      if (!s.armed) {
        s.held = down;
        return;
      }
      if (down) {
        if (!s.held) {
          if (!s.active) {
            s.active = true;
            s.swallowHold = true;
            cmd.buttons &= ~IN_SCORE;
            if (typeof s.onActivate === "function") {
              try { s.onActivate(); }
              catch (e) { globalThis.console && console.log("[s2script/hudinput] onActivate threw: " + e); }
            }
          }
        } else if (!s.active || s.swallowHold) {
          cmd.buttons &= ~IN_SCORE;
        }
        s.held = true;
        return;
      }
      s.held = false;
      s.swallowHold = false;
    });
    // A subscribe that returned no id cannot be dropped later; keep it rather than subscribe again.
    hookId = typeof id === "number" ? id : -1;
  }

  var HudInput = {
    arm: function (slot, opts) {
      var s = state(slot);
      s.armed = true;
      s.active = false;
      s.swallowHold = false;
      s.onActivate = opts && typeof opts.onActivate === "function" ? opts.onActivate : null;
      ensureHook();
    },
    disarm: function (slot) {
      var s = state(slot);
      s.armed = false;
      s.active = false;
      s.swallowHold = false;
      s.onActivate = null;
      releaseHook();
    },
    isActive: function (slot) { return !!state(slot).active; },
    isArmed: function (slot) { return !!state(slot).armed; }
  };

  if (Clients && typeof Clients.onDisconnect === "function") {
    Clients.onDisconnect(function (c) {
      if (c && typeof c.slot === "number" && !Clients.fromSlot(c.slot)) { delete states[c.slot]; releaseHook(); }
    });
  }

  globalThis.__s2pkg_hudinput = { HudInput: HudInput };
  if (globalThis.__s2pkg_cs2) globalThis.__s2pkg_cs2.HudInput = HudInput;
})();
