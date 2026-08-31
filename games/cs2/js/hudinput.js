// @s2script/cs2 — Tab-to-activate. One arm per slot; swallow IN_SCORE until the first
// rising edge fires onActivate, then Tab is the scoreboard again.
(function () {
  var UserCmd = globalThis.__s2pkg_usercmd && globalThis.__s2pkg_usercmd.UserCmd;
  var Clients = globalThis.__s2pkg_clients && globalThis.__s2pkg_clients.Clients;
  var IN_SCORE = 1n << 16n;
  var states = {};
  var hooked = false;

  function state(slot) {
    var s = states[slot];
    if (!s) {
      s = { armed: false, active: false, held: false, swallowHold: false, onActivate: null };
      states[slot] = s;
    }
    return s;
  }

  function ensureHook() {
    if (hooked || !UserCmd || typeof UserCmd.onRun !== "function") return;
    hooked = true;
    UserCmd.onRun(function (cmd, ctx) {
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
    },
    isActive: function (slot) { return !!state(slot).active; },
    isArmed: function (slot) { return !!state(slot).armed; }
  };

  if (Clients && typeof Clients.onDisconnect === "function") {
    Clients.onDisconnect(function (c) {
      if (c && typeof c.slot === "number") HudInput.disarm(c.slot);
    });
  }

  globalThis.__s2pkg_hudinput = { HudInput: HudInput };
  if (globalThis.__s2pkg_cs2) globalThis.__s2pkg_cs2.HudInput = HudInput;
})();
