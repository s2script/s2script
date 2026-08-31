(function () {
  const { UserCmd } = globalThis.__s2pkg_usercmd;
  const IN_SCORE = 1n << 16n;
  const states = new Map(); // slot -> { armed, active, held }

  function state(slot) {
    if (!states.has(slot)) states.set(slot, { armed: false, active: false, held: false });
    return states.get(slot);
  }

  const HudInput = {
    arm(slot) {
      const s = state(slot);
      s.armed = true;
    },
    disarm(slot) {
      const s = state(slot);
      s.armed = false;
      s.active = false;
      s.held = false;
    },
    isActive(slot) {
      return state(slot).active;
    },
    isArmed(slot) {
      return state(slot).armed;
    },
    consumeActive(slot) {
      const s = state(slot);
      if (!s.active) return false;
      s.active = false;
      return true;
    },
  };

  UserCmd.onRun((slot, cmd) => {
    const s = state(slot);
    const down = (cmd.buttons & IN_SCORE) !== 0n;
    if (!s.armed) {
      s.held = down;
      return;
    }
    if (s.active) {
      s.held = down;
      return; // Tab is scoreboard again after activation
    }
    if (down) {
      if (!s.held) {
        s.active = true;
        cmd.buttons &= ~IN_SCORE; // swallow
      } else {
        cmd.buttons &= ~IN_SCORE; // keep swallowing held ticks until release
      }
      s.held = true;
      return;
    }
    s.held = false;
  });

  globalThis.__s2pkg_hudinput = { HudInput };
})();
