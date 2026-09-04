const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

const source = readFileSync(join(__dirname, "menuhud.js"), "utf8");

function mount(options = {}) {
  const calls = [];
  const registered = {};
  const arms = [];
  const disarms = [];
  const pawn = { moveType: 2 };
  let spec = null;
  const claim = {
    open: (slot, opts) => calls.push({ method: "open", slot, opts }),
    close: (slot) => calls.push({ method: "close", slot }),
    refresh: (slot) => calls.push({ method: "refresh", slot }),
    setCursor: (slot, on) => calls.push({ method: "setCursor", slot, on }),
    forget: (slot) => calls.push({ method: "forget", slot })
  };
  const clientHandlers = { disconnect: [], active: [] };

  globalThis.__s2pkg_cs2 = {
    hudkit: {
      modal: (s) => {
        spec = s;
        return options.claimNull ? null : claim;
      }
    },
    Player: { fromSlot: () => ({ pawn }) }
  };
  globalThis.__s2pkg_menu = {
    Menu: {
      registerRenderer: (name, renderer) => { registered[name] = renderer; }
    },
    MenuStyle: { Chat: "chat", Center: "center" }
  };
  globalThis.__s2pkg_hudinput = {
    HudInput: {
      arm: (slot, opts) => arms.push({ slot, opts }),
      disarm: (slot) => disarms.push(slot)
    }
  };
  globalThis.__s2pkg_clients = {
    Clients: {
      onDisconnect: (fn) => clientHandlers.disconnect.push(fn),
      onActive: (fn) => clientHandlers.active.push(fn)
    }
  };
  delete globalThis.__s2pkg_menuhud;
  new Function(source)();
  return { calls, registered, arms, disarms, pawn, spec, claim, clientHandlers };
}

function session(options = {}) {
  const selected = [];
  const actions = { prev: 0, next: 0, cancel: 0 };
  const lines = options.lines || [
    { text: "First", key: "1", selectable: true, index: 0 },
    { text: "Second", key: "2", selectable: true, index: 1 }
  ];
  const value = {
    slot: options.slot == null ? 3 : options.slot,
    menu: {
      title: options.title || "Test menu",
      exitButton: options.exitButton !== false,
      freezePlayer: !!options.freezePlayer,
      activation: options.activation || "immediate"
    },
    view: () => ({
      title: options.title || "Test menu",
      lines: lines,
      page: options.page || 0,
      pageCount: options.pageCount || 1,
      exit: options.exitButton !== false
    }),
    pickNumber: (n) => {
      if (n === 8) actions.prev++;
      else if (n === 9) actions.next++;
      else if (n === 0) actions.cancel++;
      else {
        const line = lines.find((l) => l.key === String(n));
        if (line) selected.push(line.index);
      }
    },
    cancel: () => { actions.cancel++; }
  };
  value.selected = selected;
  value.actions = actions;
  return value;
}

test.afterEach(() => {
  delete globalThis.__s2pkg_cs2;
  delete globalThis.__s2pkg_menu;
  delete globalThis.__s2pkg_hudinput;
  delete globalThis.__s2pkg_clients;
  delete globalThis.__s2pkg_menuhud;
});

test("registers the HUD renderer for Center and Chat", () => {
  const mounted = mount();
  assert.equal(mounted.registered.center, globalThis.__s2pkg_menuhud.renderer);
  assert.equal(mounted.registered.chat, globalThis.__s2pkg_menuhud.renderer);
});

test("open maps page rows and picks via the item key", () => {
  const mounted = mount();
  const s = session({
    lines: [
      { text: "First", key: "1", selectable: true, index: 4 },
      { text: "Second", key: "2", selectable: true, index: 8 },
      { text: "Third", key: "3", selectable: true, index: 12 }
    ]
  });
  mounted.registered.center.open(s);
  const rows = mounted.spec.rows(3);
  assert.deepEqual(rows.map((r) => r.a), ["First", "Second", "Third"]);
  mounted.spec.onPick(3, 1, rows[1]);
  assert.deepEqual(s.selected, [8]);
});

test("disabled rows stay visible but are not selectable", () => {
  const mounted = mount();
  const s = session({
    lines: [
      { text: "Available", key: "1", selectable: true, index: 10 },
      { text: "Cooldown", key: null, selectable: false, index: 11 },
      { text: "Also available", key: "2", selectable: true, index: 12 }
    ]
  });
  mounted.registered.center.open(s);
  const rows = mounted.spec.rows(3);
  assert.equal(rows[1].disabled, true);
  mounted.spec.onPick(3, 1, rows[1]);
  assert.deepEqual(s.selected, []);
  mounted.spec.onPick(3, 2, rows[2]);
  assert.deepEqual(s.selected, [12]);
});

test("pager footer Next calls pickNumber(9)", () => {
  const mounted = mount();
  const s = session({ page: 1, pageCount: 3 });
  mounted.registered.center.open(s);
  const buttons = mounted.spec.buttons(3);
  assert.deepEqual(buttons.map((b) => b.text), ["Back", "Next", "Close"]);
  buttons[1].onClick(3);
  assert.equal(s.actions.next, 1);
});

test("a menu never freezes the player, even when the menu asks for it", () => {
  // Freezing is a convenience; being unable to move is a trap the moment anything else about the
  // menu fails — which on a live server left admins stuck in `sm_admin` with no way out. The
  // upside is cosmetic and the downside is a player who cannot play, so it stays off.
  const mounted = mount();
  mounted.registered.center.open(session({ freezePlayer: true }));
  assert.equal(mounted.pawn.moveType, 2, "moveType is untouched");
});

test("close still hides the sheet and disarms Tab", () => {
  const mounted = mount();
  mounted.registered.center.open(session({ freezePlayer: true }));
  mounted.registered.center.close(3);
  assert.ok(mounted.calls.some((c) => c.method === "close" && c.slot === 3));
  assert.deepEqual(mounted.disarms, [3]);
});

test("tab activation arms HudInput and opens without the cursor", () => {
  const mounted = mount();
  const s = session({ activation: "tab" });
  mounted.registered.center.open(s);
  const opened = mounted.calls.find((c) => c.method === "open");
  assert.deepEqual(opened.opts, { cursor: false });
  assert.equal(mounted.arms.length, 1);
  mounted.arms[0].opts.onActivate();
  assert.ok(mounted.calls.some((c) => c.method === "setCursor" && c.slot === 3 && c.on === true));
});

test("immediate activation opens with the cursor and does not arm Tab", () => {
  const mounted = mount();
  const s = session();
  mounted.registered.center.open(s);
  const opened = mounted.calls.find((c) => c.method === "open");
  assert.deepEqual(opened.opts, { cursor: true });
  assert.equal(mounted.arms.length, 0);
});

test("exhausted modal pool leaves Chat registered as the fallback", () => {
  const mounted = mount({ claimNull: true });
  assert.deepEqual(mounted.registered, {});
  assert.equal(globalThis.__s2pkg_menuhud, undefined);
});

// ── a slot outliving its player ───────────────────────────────────────────────────────────────

test("a disconnect clears the session, the cursor grab and the Tab arm", () => {
  const mounted = mount();
  const s = session({ slot: 3, freezePlayer: true, activation: "tab" });
  mounted.registered.center.open(s);
  assert.deepEqual(mounted.arms.map((a) => a.slot), [3]);

  assert.equal(mounted.clientHandlers.disconnect.length, 1, "menuhud must subscribe to onDisconnect");
  mounted.clientHandlers.disconnect[0]({ slot: 3 });

  assert.deepEqual(mounted.disarms, [3], "the Tab arm must be dropped");
  assert.ok(
    mounted.calls.some((c) => c.method === "setCursor" && c.slot === 3 && c.on === false),
    "the cursor grab must be released",
  );
  assert.ok(mounted.calls.some((c) => c.method === "forget" && c.slot === 3));
  assert.deepEqual(mounted.spec.rows(3), [], "the session must be gone");
});

test("reconnecting into the same slot is not left frozen or captured", () => {
  const mounted = mount();
  mounted.registered.center.open(session({ slot: 3, freezePlayer: true, activation: "tab" }));
  // The disconnect never arrives — a timeout or a crash. Activate is the backstop.
  mounted.clientHandlers.active[0]({ slot: 3 });

  assert.deepEqual(mounted.disarms, [3]);
  assert.deepEqual(mounted.spec.rows(3), []);
  assert.deepEqual(mounted.spec.buttons(3), [], "no footer buttons for a slot with no session");
});

test("the departed player's moveType is dropped, never written onto the next occupant", () => {
  const mounted = mount();
  mounted.registered.center.open(session({ slot: 3, freezePlayer: true }));

  mounted.clientHandlers.disconnect[0]({ slot: 3 });
  // The replacement pawn is fresh: the engine already gave it a moveType, so restoring the saved
  // one would be handing a new player a dead one's movement state.
  mounted.pawn.moveType = 2;
  mounted.clientHandlers.active[0]({ slot: 3 });
  assert.equal(mounted.pawn.moveType, 2, "activate must not re-apply a stale moveType");
});

test("clearing a slot with no TRACKED session still releases the cursor", () => {
  // Regression. This used to assert the opposite — that an untracked slot was skipped entirely —
  // and that early return is what left a player captured on a live server. The session is deleted
  // by `renderer.close`, but the cursor grab is separate state on the layout entity: any path that
  // drops one without the other leaves the player pointing at a menu that is no longer drawn, and
  // reconnecting could not fix it because by then there was no session left to find.
  const mounted = mount();
  mounted.clientHandlers.disconnect[0]({ slot: 5 });
  assert.ok(
    mounted.calls.some((c) => c.method === "setCursor" && c.slot === 5 && c.on === false),
    "the cursor is released whether or not a session was tracked",
  );
});
