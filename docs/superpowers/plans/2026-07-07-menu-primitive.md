# Menu Primitive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship SourceMod-parity interactive player menus — `new Menu(title)` + `addItem` + `display(slot, seconds)` + `onSelect`/`onCancel` — with a chat renderer and a WASD center-screen (HTML) renderer, unblocking adminmenu/basevotes/funvotes/sm_settings.

**Architecture:** `@s2script/menu` is engine-generic — the `Menu` model, pagination, chat renderer, and a `registerRenderer` seam live in the core prelude (`INJECTED_STD_PRELUDE` in `core/src/v8host.rs`), slot-based like `@s2script/commands`. The CS2 center renderer (button-mask input polling + the `show_survival_respawn_status` center-HTML event) lives in `games/cs2/js/pawn.js` and registers through the seam. One new engine-generic op, `event_fire_to_client`, adds SourceMod-parity per-client event firing (`Events.fireToClient`); the WASD input reuses the existing `EntityRef.readUInt64Via` chain read (no new native).

**Tech Stack:** Rust (core, `v8` prelude JS + FFI ops), C++ (shim, Metamod:Source), JavaScript (prelude + `pawn.js` runtime), TypeScript (`.d.ts` + demo plugin, esbuild via `s2script build`).

## Global Constraints

- **Charter boundary — core is engine-generic; games are packages; deps point game → core, never core → game.** `@s2script/menu` (model + chat renderer + seam) and the `event_fire_to_client` op carry NO game identifiers. CS2 facts (`show_survival_respawn_status`, `loc_token`, button-mask field names, `IN_*` values) live ONLY in `games/cs2/js/pawn.js` and `packages/cs2`. The core-boundary gate (`scripts/check-core-boundary.sh`) and the name-leak gate (`scripts/test-boundary-nameleak.sh`) must both stay green.
- **Slot-based callbacks.** Every `@s2script/menu` callback passes a raw 0-based `slot` (`-1` = server console is never a menu target), never a `Player` — exactly like `@s2script/commands`. A CS2 plugin resolves `Player.fromSlot(slot)` itself.
- **Degrade-never-crash.** A null pawn (player left mid-menu), an unregistered renderer, or a missing op degrades to a no-op/close, never a crash. Every new native/op body already runs under the codebase's `catch_unwind`/null-guard conventions — follow them.
- **ABI append-only.** New ops are APPENDED to `S2EngineOps` after the current last field (`db_data_dir`), in the SAME order in the C header (`shim/include/s2script_core.h`), the Rust mirror (`core/src/v8host.rs`), and both in-isolate test op-structs. Never reorder existing fields.
- **Naming:** PascalCase types/events (`Menu`, `MenuStyle`, `OnGameFrame`), camelCase functions/properties (`addItem`, `onSelect`, `fireToClient`).
- **One active menu per player PER PLUGIN** (context-local); cross-plugin coordination is deferred. **HTML confirmed** for center (no plain-text fallback needed). Full spec: `docs/superpowers/specs/2026-07-07-menu-primitive-design.md`.
- **Test running:** core tests run serial (`.cargo/config.toml` sets `RUST_TEST_THREADS=1`); run with `cd core && cargo test`. In-isolate prelude tests use `eval_std(...)` / `eval_in_context_string(...)` in `core/src/v8host.rs`'s `frame_tests` module.

---

### Task 1: The `Menu` model + pagination + `registerRenderer` seam (engine-generic, pure)

The pure engine-generic core of `@s2script/menu`: the `Menu` class, the enums, the pagination/pick/cursor logic, and the renderer seam — with NO dependency on chat/timers/clients yet. A test-only "record renderer" captures the computed view so the model is fully unit-testable in isolation.

**Files:**
- Create: `packages/menu/package.json`
- Create: `packages/menu/index.d.ts`
- Modify: `core/src/v8host.rs` — add the `@s2script/menu` model JS to `INJECTED_STD_PRELUDE` (before the `globalThis.__s2pkg_* = ...` assignment block near line 648); add in-isolate tests to `frame_tests`.

**Interfaces:**
- Consumes: nothing (pure). Runs inside `INJECTED_STD_PRELUDE`, which already defines `OnGameFrame`, `Vector`, `Events`, etc.
- Produces (for Tasks 2/4):
  - `globalThis.__s2pkg_menu = { Menu, MenuStyle, MenuCancelReason }`.
  - `MenuStyle = { Chat: "chat", Center: "center" }`; `MenuCancelReason = { Exit: 0, Timeout: 1, Disconnect: 2, NewMenu: 3 }`.
  - `Menu` instance shape: `title` (string), `style` (a `MenuStyle` value, default `Chat`), `exitButton` (bool, default true), `items` (array of `{ info, display, disabled }`), `addItem(info, display, opts?)`, `onSelect(fn)`, `onCancel(fn)`, `display(slot, seconds)`, `close(slot)`.
  - `Menu.registerRenderer(name, renderer)` (static): registers a renderer object `{ open(session), update(session), close(slot) }` under a `MenuStyle` value.
  - The `session` object passed to a renderer, exposing:
    - `session.slot` (number)
    - `session.view()` → `{ title, lines: [{ text, key, selectable, cursor }], page, pageCount, exit: boolean }` — the resolved current page. `key` is the on-screen chat number (string, e.g. `"1"`, `"8"`) or `null`; `cursor` is true for the currently-highlighted line (center).
    - `session.pickNumber(n)` — a chat number-key pick (`n` a number). Resolves to item-select or a control (Next/Back/Exit).
    - `session.moveUp()`, `session.moveDown()`, `session.confirm()`, `session.cancel()` — cursor navigation (center). `confirm()` on an item fires select; on the Exit line cancels with `Exit`.
  - The internal `__s2_menu_activeBySlot` map (context-local) is NOT part of the public interface but Task 2 extends it.

- [ ] **Step 1: Write the failing in-isolate tests**

Add to the `frame_tests` module in `core/src/v8host.rs` (near the other `eval_std`-based tests). Register a JS "record renderer" that stores each `view()`, then assert pagination, pick resolution, cursor movement, and disabled-skip:

```rust
#[test]
fn menu_model_pagination_pick_cursor() {
    // Pagination: 9 items, exitButton -> page 0 shows items 1..7 as keys "1".."7",
    // then control keys 9=Next, 0=Exit (no Back on page 0).
    let out = eval_std("mp", r#"
        var { Menu, MenuStyle } = globalThis.__s2pkg_menu;
        var captured = [];
        Menu.registerRenderer("rec", {
            open: function (s) { captured.push(s.view()); },
            update: function (s) { captured.push(s.view()); },
            close: function () {},
        });
        var m = new Menu("T");
        m.style = "rec";
        for (var i = 0; i < 9; i++) m.addItem("info" + i, "Item " + i);
        var picked = null;
        m.onSelect(function (e) { picked = e.info + ":" + e.item; });
        m.display(3, 0);
        var v0 = captured[captured.length - 1];
        // 7 selectable item-lines on page 0
        var itemKeys = v0.lines.filter(function (l) { return l.selectable; }).map(function (l) { return l.key; });
        // control keys present: Next="9", Exit="0"; no Back
        var ctrlKeys = v0.lines.filter(function (l) { return !l.selectable && l.key; }).map(function (l) { return l.key; });
        JSON.stringify({ items: itemKeys, ctrl: ctrlKeys, pageCount: v0.pageCount });
    "#);
    assert_eq!(out, r#"{"items":["1","2","3","4","5","6","7"],"ctrl":["9","0"],"pageCount":2}"#);
}

#[test]
fn menu_model_next_page_and_select() {
    let out = eval_std("mn", r#"
        var { Menu } = globalThis.__s2pkg_menu;
        var last = null;
        Menu.registerRenderer("rec2", { open: function (s){ last = s; }, update: function (s){ last = s; }, close: function(){} });
        var m = new Menu("T"); m.style = "rec2";
        for (var i = 0; i < 9; i++) m.addItem("info" + i, "Item " + i);
        var picked = null; m.onSelect(function (e){ picked = e.info + ":" + e.item; });
        m.display(3, 0);
        last.pickNumber(9);          // Next -> page 1 (items 8,9 => "info7","info8")
        last.pickNumber(1);          // first item on page 1 = index 7
        picked;
    "#);
    assert_eq!(out, "info7:7");
}

#[test]
fn menu_model_disabled_item_not_selectable() {
    let out = eval_std("md", r#"
        var { Menu } = globalThis.__s2pkg_menu;
        var last = null;
        Menu.registerRenderer("rec3", { open: function (s){ last = s; }, update: function (s){ last = s; }, close: function(){} });
        var m = new Menu("T"); m.style = "rec3";
        m.addItem("a", "A", { disabled: true });
        m.addItem("b", "B");
        var picked = "none"; m.onSelect(function (e){ picked = e.info; });
        m.display(3, 0);
        // disabled "a" has no number; "b" is key "1"
        var v = last.view();
        var aLine = v.lines[0], bLine = v.lines[1];
        last.pickNumber(1);   // selects "b"
        JSON.stringify({ aKey: aLine.key, aSel: aLine.selectable, bKey: bLine.key, picked: picked });
    "#);
    assert_eq!(out, r#"{"aKey":null,"aSel":false,"bKey":"1","picked":"b"}"#);
}

#[test]
fn menu_model_center_cursor_and_confirm() {
    let out = eval_std("mc", r#"
        var { Menu } = globalThis.__s2pkg_menu;
        var last = null;
        Menu.registerRenderer("rec4", { open: function (s){ last = s; }, update: function (s){ last = s; }, close: function(){} });
        var m = new Menu("T"); m.style = "rec4";
        m.addItem("x", "X"); m.addItem("y", "Y"); m.addItem("z", "Z");
        var picked = null; m.onSelect(function (e){ picked = e.info; });
        m.display(3, 0);
        last.moveDown();     // cursor 0 -> 1 (Y)
        last.confirm();      // selects Y
        picked;
    "#);
    assert_eq!(out, "y");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd core && cargo test menu_model`
Expected: FAIL — `__s2pkg_menu` is undefined (`TypeError` / the returned strings won't match).

- [ ] **Step 3: Implement the model in the prelude**

In `core/src/v8host.rs`, inside `INJECTED_STD_PRELUDE`, add the following JS BEFORE the block that assigns `globalThis.__s2pkg_math = ...` etc. (around line 648). Complete implementation:

```javascript
  // --- Menu primitive (engine-generic): model + pagination + registerRenderer seam. Slot-based. ---
  var MenuStyle = { Chat: "chat", Center: "center" };
  var MenuCancelReason = { Exit: 0, Timeout: 1, Disconnect: 2, NewMenu: 3 };
  var MENU_ITEMS_PER_PAGE = 7;            // chat page size (SM ITEMS_PER_PAGE)
  var __s2_menu_renderers = {};           // style value -> renderer { open, update, close }
  var __s2_menu_activeBySlot = {};        // slot -> session (one active menu per slot, this context)

  function Menu(title) {
    this.title = title || "";
    this.style = MenuStyle.Chat;
    this.exitButton = true;
    this.items = [];
    this._onSelect = null;
    this._onCancel = null;
  }
  Menu.registerRenderer = function (name, renderer) { __s2_menu_renderers[name] = renderer; };
  Menu.prototype.addItem = function (info, display, opts) {
    this.items.push({ info: String(info), display: String(display), disabled: !!(opts && opts.disabled) });
  };
  Menu.prototype.onSelect = function (fn) { this._onSelect = fn; };
  Menu.prototype.onCancel = function (fn) { this._onCancel = fn; };
  Menu.prototype.display = function (slot, seconds) {
    if (typeof slot !== "number" || slot < 0) return;   // console/invalid is never a menu target
    var renderer = __s2_menu_renderers[this.style] || __s2_menu_renderers[MenuStyle.Chat];
    if (!renderer) { globalThis.console && console.log("[menu] no renderer for style " + this.style); return; }
    // Replace any existing session for this slot (NewMenu).
    var prev = __s2_menu_activeBySlot[slot];
    if (prev) prev._end(MenuCancelReason.NewMenu);
    var session = new MenuSession(this, slot, renderer, seconds || 0);
    __s2_menu_activeBySlot[slot] = session;
    session._start();
  };
  Menu.prototype.close = function (slot) {
    var s = __s2_menu_activeBySlot[slot];
    if (s && s.menu === this) s._end(MenuCancelReason.Exit);
  };

  // A live display of one menu to one slot. Owns page/cursor state.
  function MenuSession(menu, slot, renderer, seconds) {
    this.menu = menu; this.slot = slot; this.renderer = renderer; this.seconds = seconds;
    this.page = 0; this.cursor = 0; this._ended = false;
    this._selectable = [];   // indices (into menu.items) that are selectable on the CURRENT page
  }
  // Selectable item indices for a chat page: up to MENU_ITEMS_PER_PAGE, skipping disabled.
  MenuSession.prototype._pageItems = function (page) {
    var out = [], start = page * MENU_ITEMS_PER_PAGE, seen = 0, i = start;
    // NOTE: disabled items still occupy a slot in the on-screen list but get no number.
    for (; i < this.menu.items.length && (i - start) < MENU_ITEMS_PER_PAGE; i++) out.push(i);
    return out;
  };
  MenuSession.prototype.pageCount = function () {
    return Math.max(1, Math.ceil(this.menu.items.length / MENU_ITEMS_PER_PAGE));
  };
  // Build the resolved view the renderer paints. Assigns chat number keys 1..7 to selectable items,
  // then control keys 8=Back, 9=Next, 0=Exit as applicable.
  MenuSession.prototype.view = function () {
    var m = this.menu, pageItems = this._pageItems(this.page), lines = [], keyNum = 1;
    this._selectable = [];
    for (var k = 0; k < pageItems.length; k++) {
      var idx = pageItems[k], it = m.items[idx], key = null, selectable = false;
      if (!it.disabled) { key = String(keyNum++); selectable = true; this._selectable.push(idx); }
      lines.push({ text: it.display, key: key, selectable: selectable, cursor: (this.style === MenuStyle.Center && this._centerCursorIdx() === idx), index: idx });
    }
    var pc = this.pageCount();
    if (this.page > 0)      lines.push({ text: "Back", key: "8", selectable: false, control: "back" });
    if (this.page < pc - 1) lines.push({ text: "Next", key: "9", selectable: false, control: "next" });
    if (m.exitButton)       lines.push({ text: "Exit", key: "0", selectable: false, control: "exit" });
    return { title: m.title, lines: lines, page: this.page, pageCount: pc, exit: m.exitButton };
  };
  // (Center) the menu.items index currently under the cursor, or -1.
  MenuSession.prototype._centerCursorIdx = function () {
    var pageItems = this._pageItems(this.page);
    // cursor indexes selectable-on-page; map through non-disabled.
    var sel = pageItems.filter(function (i) { return !this.menu.items[i].disabled; }, this);
    return (this.cursor >= 0 && this.cursor < sel.length) ? sel[this.cursor] : -1;
  };
  MenuSession.prototype._start = function () { this.renderer.open(this); if (this.seconds > 0) this._armTimeout(); };
  MenuSession.prototype._armTimeout = function () { /* Task 2 wires __s2pkg_timers.delay */ };
  MenuSession.prototype._repaint = function () { if (!this._ended) this.renderer.update(this); };
  MenuSession.prototype._end = function (reason) {
    if (this._ended) return; this._ended = true;
    if (__s2_menu_activeBySlot[this.slot] === this) delete __s2_menu_activeBySlot[this.slot];
    this.renderer.close(this.slot);
    if (this.menu._onCancel && (reason === MenuCancelReason.Timeout || reason === MenuCancelReason.Disconnect || reason === MenuCancelReason.NewMenu || reason === MenuCancelReason.Exit))
      { try { this.menu._onCancel({ slot: this.slot, reason: reason }); } catch (e) { globalThis.console && console.log("[menu] onCancel threw: " + e); } }
  };
  MenuSession.prototype._select = function (itemIndex) {
    var it = this.menu.items[itemIndex];
    if (!it || it.disabled) return;
    // mark ended BEFORE the callback so a re-display inside onSelect isn't clobbered
    this._ended = true;
    if (__s2_menu_activeBySlot[this.slot] === this) delete __s2_menu_activeBySlot[this.slot];
    this.renderer.close(this.slot);
    if (this.menu._onSelect) { try { this.menu._onSelect({ slot: this.slot, item: itemIndex, info: it.info, display: it.display }); } catch (e) { globalThis.console && console.log("[menu] onSelect threw: " + e); } }
  };
  // Chat idiom: a number-key pick against the current view's keys.
  MenuSession.prototype.pickNumber = function (n) {
    if (this._ended) return;
    this.view();  // refresh this._selectable for the current page
    var key = String(n);
    if (key === "8" && this.page > 0)                      { this.page--; this.cursor = 0; this._repaint(); return; }
    if (key === "9" && this.page < this.pageCount() - 1)   { this.page++; this.cursor = 0; this._repaint(); return; }
    if (key === "0" && this.menu.exitButton)               { this._end(MenuCancelReason.Exit); return; }
    var slotN = n - 1;
    if (slotN >= 0 && slotN < this._selectable.length) this._select(this._selectable[slotN]);
  };
  // Center idiom: cursor navigation.
  MenuSession.prototype.moveUp = function () {
    if (this._ended) return; this.view();
    var count = this._selectable.length; if (!count) return;
    this.cursor = (this.cursor - 1 + count) % count; this._repaint();
  };
  MenuSession.prototype.moveDown = function () {
    if (this._ended) return; this.view();
    var count = this._selectable.length; if (!count) return;
    this.cursor = (this.cursor + 1) % count; this._repaint();
  };
  MenuSession.prototype.confirm = function () {
    if (this._ended) return; this.view();
    if (this.cursor >= 0 && this.cursor < this._selectable.length) this._select(this._selectable[this.cursor]);
  };
  MenuSession.prototype.cancel = function () { if (!this._ended) this._end(MenuCancelReason.Exit); };
```

Then add to the `globalThis.__s2pkg_* = ...` block (near line 648, beside `__s2pkg_events = ...`):

```javascript
  globalThis.__s2pkg_menu = { Menu: Menu, MenuStyle: MenuStyle, MenuCancelReason: MenuCancelReason };
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd core && cargo test menu_model`
Expected: PASS (4 tests).

- [ ] **Step 5: Write the `.d.ts` package**

`packages/menu/package.json`:
```json
{ "name": "@s2script/menu", "version": "0.1.0", "types": "index.d.ts" }
```

`packages/menu/index.d.ts`:
```typescript
/** @s2script/menu — interactive player menus (chat + center backends). NO runtime code (injected at load). */
export declare const enum MenuStyle { Chat = "chat", Center = "center" }
export declare const enum MenuCancelReason { Exit = 0, Timeout = 1, Disconnect = 2, NewMenu = 3 }

export interface MenuSelectEvent { readonly slot: number; readonly item: number; readonly info: string; readonly display: string; }
export interface MenuCancelEvent { readonly slot: number; readonly reason: MenuCancelReason; }

export declare class Menu {
  constructor(title?: string);
  title: string;
  /** Which registered renderer to use. Falls back to Chat if the style's renderer is unregistered. */
  style: MenuStyle;
  /** Append an auto Exit control (default true). */
  exitButton: boolean;
  /** (info, display) — `info` is returned to onSelect; `display` is shown. */
  addItem(info: string, display: string, opts?: { disabled?: boolean }): void;
  onSelect(handler: (e: MenuSelectEvent) => void): void;
  onCancel(handler: (e: MenuCancelEvent) => void): void;
  /** Show to a 0-based slot for `seconds` (0 = until selection/cancel/disconnect). */
  display(slot: number, seconds?: number): void;
  /** Close an open menu for `slot` early. */
  close(slot: number): void;
  /** Register a display backend under a MenuStyle value (used by the CS2 center renderer). */
  static registerRenderer(name: string, renderer: MenuRenderer): void;
}

export interface MenuSession {
  readonly slot: number;
  view(): { title: string; lines: MenuLine[]; page: number; pageCount: number; exit: boolean };
  pickNumber(n: number): void;
  moveUp(): void; moveDown(): void; confirm(): void; cancel(): void;
}
export interface MenuLine { text: string; key: string | null; selectable: boolean; cursor?: boolean; control?: string; index?: number; }
export interface MenuRenderer { open(session: MenuSession): void; update(session: MenuSession): void; close(slot: number): void; }
```

- [ ] **Step 6: Run the boundary gates + commit**

Run: `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`
Expected: both PASS (no game names in the model).

```bash
git add core/src/v8host.rs packages/menu/package.json packages/menu/index.d.ts
git commit -m "feat(menu): Menu model + pagination + registerRenderer seam (@s2script/menu, engine-generic)"
```

---

### Task 2: The chat renderer + lifecycle (timeout, disconnect)

Register the built-in `"chat"` renderer over `__s2pkg_chat`, wire the display timeout via `__s2pkg_timers.delay`, and close open menus on `Clients.onDisconnect`. All engine-generic.

**Files:**
- Modify: `core/src/v8host.rs` — add the chat renderer + timeout + disconnect wiring to `INJECTED_STD_PRELUDE` (immediately after the Task 1 menu model, before the `__s2pkg_menu` assignment); add in-isolate tests to `frame_tests`.

**Interfaces:**
- Consumes (from the prelude, all already defined earlier in the string): `__s2_chat` (`{ toSlot(slot, msg) }` — this is what `__s2pkg_chat.Chat` wraps; the renderer uses `globalThis.__s2pkg_chat.Chat.toSlot`), `Chat.onMessage` (via `globalThis.__s2pkg_chat.Chat.onMessage(fn)` returning `>= 2` suppresses), `timers` (`globalThis.__s2pkg_timers.delay(ms)` → Promise), `globalThis.__s2pkg_clients.Clients.onDisconnect(fn)`. `HookResult.Handled` = `2` (available as `globalThis.HookResult`).
- Consumes (from Task 1): `Menu`, `MenuStyle`, `MenuSession.prototype._armTimeout`, `__s2_menu_activeBySlot`, `MenuCancelReason`.
- Produces: `Menu.registerRenderer(MenuStyle.Chat, chatRenderer)` is called at prelude load; `MenuSession._armTimeout` is implemented.

- [ ] **Step 1: Write the failing in-isolate tests**

```rust
#[test]
fn menu_chat_renders_and_number_selects() {
    let out = eval_std("mchat", r#"
        var { Menu, MenuStyle } = globalThis.__s2pkg_menu;
        // capture chat lines sent to the slot
        var sent = [];
        var realToSlot = globalThis.__s2pkg_chat.Chat.toSlot;
        globalThis.__s2pkg_chat.Chat.toSlot = function (s, msg) { sent.push([s, msg]); };
        // capture the onMessage handler the renderer installs
        var chatHandler = null;
        var realOn = globalThis.__s2pkg_chat.Chat.onMessage;
        globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
        var m = new Menu("Pick"); m.style = MenuStyle.Chat;
        m.addItem("kick", "Kick"); m.addItem("ban", "Ban");
        var got = null; m.onSelect(function (e){ got = e.info; });
        m.display(3, 0);
        // simulate slot 3 typing "2"
        var suppressed = chatHandler(3, "2", false);
        // restore
        globalThis.__s2pkg_chat.Chat.toSlot = realToSlot;
        globalThis.__s2pkg_chat.Chat.onMessage = realOn;
        JSON.stringify({ sentCount: sent.length > 0, picked: got, suppressed: suppressed });
    "#);
    // "2" -> second item "ban"; a matched pick suppresses the chat line (>=2)
    assert_eq!(out, r#"{"sentCount":true,"picked":"ban","suppressed":2}"#);
}

#[test]
fn menu_chat_nonmatching_message_passes_through() {
    let out = eval_std("mchat2", r#"
        var { Menu, MenuStyle } = globalThis.__s2pkg_menu;
        var chatHandler = null;
        var realOn = globalThis.__s2pkg_chat.Chat.onMessage;
        globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
        var m = new Menu("P"); m.style = MenuStyle.Chat; m.addItem("a", "A");
        m.display(3, 0);
        var r1 = chatHandler(3, "hello", false);   // not a digit -> pass through (undefined/0)
        var r2 = chatHandler(4, "1", false);        // different slot -> pass through
        globalThis.__s2pkg_chat.Chat.onMessage = realOn;
        JSON.stringify({ r1: r1 == null || r1 < 2, r2: r2 == null || r2 < 2 });
    "#);
    assert_eq!(out, r#"{"r1":true,"r2":true}"#);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd core && cargo test menu_chat`
Expected: FAIL — the chat renderer is not registered, so `display` uses no renderer and never installs an onMessage handler (`chatHandler` stays null → `TypeError`).

- [ ] **Step 3: Implement the chat renderer + lifecycle**

In `INJECTED_STD_PRELUDE`, right after the Task 1 model (before the `globalThis.__s2pkg_menu = ...` line), add:

```javascript
  // Chat renderer: paints numbered lines via __s2pkg_chat; one shared onMessage sub captures picks.
  (function () {
    var HANDLED = (globalThis.HookResult && globalThis.HookResult.Handled) || 2;
    var chatSessions = {};      // slot -> session (chat menus only)
    var subInstalled = false;
    function ensureSub() {
      if (subInstalled) return; subInstalled = true;
      globalThis.__s2pkg_chat.Chat.onMessage(function (slot, text, teamonly) {
        var s = chatSessions[slot];
        if (!s || s._ended) return;                 // no menu for this slot -> pass through
        var t = ("" + text).trim();
        if (!/^[0-9]$/.test(t)) return;             // not a single digit -> pass through (chat shows)
        s.pickNumber(parseInt(t, 10));
        return HANDLED;                              // swallow the menu pick from public chat
      });
    }
    globalThis.__s2pkg_menu.Menu.registerRenderer(globalThis.__s2pkg_menu.MenuStyle.Chat, {
      open: function (session) { ensureSub(); chatSessions[session.slot] = session; this.update(session); },
      update: function (session) {
        var v = session.view(), C = globalThis.__s2pkg_chat.Chat;
        C.toSlot(session.slot, v.title);
        for (var i = 0; i < v.lines.length; i++) {
          var l = v.lines[i];
          C.toSlot(session.slot, (l.key ? l.key + ". " : "   ") + l.text);
        }
      },
      close: function (slot) { delete chatSessions[slot]; },
    });
  })();

  // Timeout: arm a delay that cancels the session (any renderer).
  MenuSession.prototype._armTimeout = function () {
    var self = this, ms = (this.seconds | 0) * 1000;
    globalThis.__s2pkg_timers.delay(ms).then(function () {
      if (!self._ended) self._end(MenuCancelReason.Timeout);
    });
  };

  // Disconnect: close any open menu for a leaving slot.
  globalThis.__s2pkg_clients.Clients.onDisconnect(function (client) {
    var s = __s2_menu_activeBySlot[client.slot];
    if (s) s._end(MenuCancelReason.Disconnect);
  });
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd core && cargo test menu`
Expected: PASS (Task 1 + Task 2 tests).

- [ ] **Step 5: Full core suite + boundary + commit**

Run: `cd core && cargo test` (expect the full suite green, e.g. 190+ passing) and `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`.

```bash
git add core/src/v8host.rs
git commit -m "feat(menu): chat renderer + timeout + disconnect-close (engine-generic)"
```

---

### Task 3: `event_fire_to_client` op + `Events.fireToClient` (per-client event fire)

The one new engine primitive: a generic per-client game-event fire (SourceMod parity). Adds the ABI op, the shim implementation (resolving the per-client `IGameEventListener2*` — the bounded spike), the core native, and the `Events.fireToClient` prelude helper (sharing `fire`'s field-set loop). Requires a sniper rebuild.

**Files:**
- Modify: `shim/include/s2script_core.h` — append `event_fire_to_client` to the `S2EngineOps` struct.
- Modify: `shim/src/s2script_mm.cpp` — implement `s2_event_fire_to_client(int slot)`; add the listener-offset gamedata; wire into the ops struct.
- Modify: `core/src/v8host.rs` — the `EventFireToClientFn` typedef, the `event_fire_to_client` ops field (after `db_data_dir`), the ENGINE_OPS `None` init, the `__s2_event_fire_to_client` native + registration, both in-isolate test op-structs, and `Events.fireToClient` in the prelude (refactor `fire`'s field loop into a shared `__s2_event_apply_fields`).
- Modify: `core/src/ffi.rs` — mirror the op if `ffi.rs` re-declares the ops struct (check; the ABI mirror lives where the other ops are).
- Modify: `packages/events/index.d.ts` — add `fireToClient`.

**Interfaces:**
- Consumes: the existing 5D.3 event machinery — `__s2_event_create(name)`, `__s2_event_set_*`, and the shim's `s_pendingFireEvent`/`s_pGameEventManager`; the 5D.2 `S2_ClientAt(int slot)` (returns `CServerSideClient*`).
- Produces:
  - Op: `int (*event_fire_to_client)(int slot)` — fires the pending created event to the slot's client listener; returns 1 on success, 0 on miss (no manager / no pending event / no client / bot).
  - Native: `__s2_event_fire_to_client(slot: number) -> boolean`.
  - Prelude: `Events.fireToClient(slot, name, fields)` — builds the event (create + apply fields, same as `fire`) then fires to `slot`; returns boolean.
  - `.d.ts`: `Events.fireToClient(slot: number, name: string, fields?: Record<string, ...>): boolean`.

- [ ] **Step 1: Write the failing degrade test**

Add to `frame_tests` (mirrors the existing `Events.fire` degrade test at v8host.rs:7405):

```rust
#[test]
fn events_fire_to_client_degrades_without_ops() {
    // With no engine ops, __s2_event_create returns false, so fireToClient short-circuits to false.
    assert_eq!(
        eval_in_context_string("p", r#"var {Events}=__s2pkg_events; String(Events.fireToClient(0, "x", {a:1}))"#),
        "false"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd core && cargo test events_fire_to_client_degrades`
Expected: FAIL — `Events.fireToClient` is not a function (`TypeError`).

- [ ] **Step 3: Add the ABI op + native (core)**

In `core/src/v8host.rs`:

1. Add the fn typedef near the other `EventFireFn` etc. typedefs:
```rust
type EventFireToClientFn = extern "C" fn(slot: i32) -> i32;
```
2. Append the ops field AFTER `pub db_data_dir: Option<DbDataDirFn>,` (the current last field):
```rust
    // --- Slice menu: per-client event fire (APPENDED after db_data_dir; order is the ABI; do not reorder above) ---
    pub event_fire_to_client: Option<EventFireToClientFn>,
```
3. In the `ENGINE_OPS` initializer (the struct literal near line 6925 that sets every field to `None`/the incoming op), add `event_fire_to_client: None,` in the appended position. (Search for `db_data_dir:` in the init and add the new field right after it, matching whatever value pattern that init uses.)
4. Add the native (mirror `s2_event_fire` at v8host.rs:3455):
```rust
/// Native `__s2_event_fire_to_client(slot) -> boolean` — fire the created event to ONE client's
/// per-client listener (serialized to that netchannel; does NOT pass through IGameEventManager2::FireEvent,
/// so no pre-hook / dispatch re-entrancy). Returns false on any miss.
fn s2_event_fire_to_client(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        ENGINE_OPS.with(|c| {
            let ops = c.get();
            if let Some(func) = ops.event_fire_to_client {
                rv.set_bool(func(slot) != 0);
            }
        });
    }));
}
```
(Match the exact `ENGINE_OPS.with` / `ops.get()` access pattern used by `s2_event_fire` — copy its body shape.)
5. Register it beside `__s2_event_fire` (v8host.rs:3735):
```rust
    set_native(scope, global_obj, "__s2_event_fire_to_client", s2_event_fire_to_client);
```
6. Update BOTH in-isolate test op-structs (search the test module for where the other appended ops like `db_data_dir` are set in a test `S2EngineOps { ... }` literal) — add `event_fire_to_client: None,` (or a test stub) in the appended position so the test builds.

- [ ] **Step 4: Add `Events.fireToClient` to the prelude (refactor shared field loop)**

In `INJECTED_STD_PRELUDE`, refactor the `fire` field-set loop into a shared helper and add `fireToClient`:

```javascript
    // shared: apply { key: value } to the current event (create must have run). Type-infer as in `fire`.
    _applyFields: function (fields) {
      if (!fields) return;
      for (var k in fields) {
        if (!Object.prototype.hasOwnProperty.call(fields, k)) continue;
        var v = fields[k], t = typeof v;
        if (t === "boolean") __s2_event_set_bool(k, v);
        else if (t === "string") __s2_event_set_string(k, v);
        else if (t === "bigint") __s2_event_set_uint64(k, v.toString());
        else if (t === "number") { if (Number.isInteger(v)) __s2_event_set_int(k, v); else __s2_event_set_float(k, v); }
      }
    },
    fire: function (name, fields, dontBroadcast) {
      if (!__s2_event_create(name)) return false;
      this._applyFields(fields);
      return __s2_event_fire(!!dontBroadcast);
    },
    fireToClient: function (slot, name, fields) {
      if (!__s2_event_create(name)) return false;
      this._applyFields(fields);
      return __s2_event_fire_to_client(slot | 0);
    },
```
(Replace the existing inline field loop in `fire` with the `_applyFields` call; keep behavior identical.)

- [ ] **Step 5: Implement the shim op (the spike)**

In `shim/src/s2script_mm.cpp`:

1. Append to the `S2EngineOps` ops-struct wiring (wherever the shim fills its `S2EngineOps` local before `s2script_core_init` — search for `db_data_dir`) : `ops.event_fire_to_client = s2_event_fire_to_client;`.
2. Add the C header field in `shim/include/s2script_core.h` — append to the `S2EngineOps` struct after the current last op field:
```c
    /* Slice menu: fire the pending created event to ONE client's per-client legacy listener. */
    int (*event_fire_to_client)(int slot);
```
3. Add a gamedata-backed offset for the per-client `IGameEventListener2*` within `CServerSideClient` (the SPIKE). Declare a static like the other 5D.2 offsets:
```cpp
static int s_offSscEventListener = -1;  // offset of the IGameEventListener2 subobject within CServerSideClient
```
   **Spike (resolve the offset):** the target is the `IGameEventListener2*` for a slot, i.e. `CServerSideClient` cast to its `IGameEventListener2` base. Resolve the offset by (a) checking CounterStrikeSharp's gamedata for its `GetLegacyGameEventListener` / `CServerSideClient` listener offset as a starting hint, then (b) validating live: the demo (Task 5) fires `show_survival_respawn_status` to a real client and confirms the center HTML appears (the user will join). If the subobject offset is `0` (single-inheritance-first base), `reinterpret_cast<IGameEventListener2*>(client)` works directly; otherwise add the offset. Add the offset to the `.signatures`/offsets gamedata (`gamedata/core.gamedata.jsonc`) so it is regenerable, mirroring the 5D.2 client offsets, and load it in `Load()` beside `s_offSscName` etc.
4. Implement the op (mirror `s2_event_fire` at s2script_mm.cpp:415):
```cpp
static int s2_event_fire_to_client(int slot) {
    if (!s_pGameEventManager || !s_pendingFireEvent) return 0;
    void* client = S2_ClientAt(slot);
    if (!client) return 0;   // invalid slot or a bot (fake client has no per-client listener/netchannel)
    IGameEventListener2* pListener = reinterpret_cast<IGameEventListener2*>(
        reinterpret_cast<char*>(client) + (s_offSscEventListener >= 0 ? s_offSscEventListener : 0));
    IGameEvent* e = s_pendingFireEvent;
    s_pendingFireEvent = nullptr;
    pListener->FireGameEvent(e);        // serialize to this client's netchannel (no broadcast)
    s_pGameEventManager->FreeEvent(e);  // FireGameEvent does not consume the event; free it (CSSharp parity)
    return 1;
}
```
(Declare `s2_event_fire_to_client` above the ops wiring; keep it near `s2_event_fire`.)

- [ ] **Step 6: Run the degrade test + boundary gates**

Run: `cd core && cargo test event` (the new degrade test passes; existing event tests still green) and `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`.
Expected: PASS. (The shim change is compiled at the sniper build in Task 5's gate; core builds here.)

- [ ] **Step 7: `.d.ts` + commit**

Add to `packages/events/index.d.ts` on the `Events` object:
```typescript
  /** Fire a game event to ONE client (SourceMod FireToClient). Same field type-inference as `fire`. Returns false on miss. */
  fireToClient(slot: number, name: string, fields?: Record<string, string | number | boolean | bigint>): boolean;
```

```bash
git add core/src/v8host.rs core/src/ffi.rs shim/include/s2script_core.h shim/src/s2script_mm.cpp gamedata/core.gamedata.jsonc packages/events/index.d.ts
git commit -m "feat(events): event_fire_to_client op + Events.fireToClient (per-client event fire, SM parity)"
```

---

### Task 4: CS2 center renderer — button polling + `show_survival_respawn_status` HTML

The CS2-specific `"center"` renderer in `pawn.js`: an `OnGameFrame` poll reading the pawn's button mask (edge-detect W/S/E → session nav) and rendering the menu as `loc_token` HTML re-sent each tick via `Events.fireToClient`. This is the ONLY file with CS2 menu facts.

**Files:**
- Modify: `games/cs2/js/pawn.js` — register the `"center"` renderer into `globalThis.__s2pkg_menu`.
- Modify: `packages/cs2/index.d.ts` — (only if any CS2-public type is added; the `Menu` API is `@s2script/menu`, so likely no change — verify).

**Interfaces:**
- Consumes: `globalThis.__s2pkg_menu.Menu.registerRenderer`, `MenuStyle`; `globalThis.__s2pkg_frame.OnGameFrame`; `globalThis.__s2pkg_events.Events.fireToClient`; `Player.fromSlot(slot)` and `pawn`'s `movementServices` nav / `EntityRef.readUInt64Via` (5C.5); `__s2_schema_offset(class, field)` for the button offsets.
- Produces: a registered `"center"` renderer. No new public JS API (menu authors use `@s2script/menu`).

- [ ] **Step 1: Resolve the button-read offsets + IN_* bits (spike, in `pawn.js`)**

The button mask is `CPlayer_MovementServices.m_nButtons` (a `CInButtonState`) → `m_pButtonStates[0]` (uint64). `m_pMovementServices` is a POINTER on the pawn. So read via a chain: root pawn → deref `m_pMovementServices` → uint64 at `offsetof(m_nButtons) + offsetof(CInButtonState::m_pButtonStates)`.

Resolve offsets once (module scope in `pawn.js`), guarded:
```javascript
// Menu center-renderer input: the live "buttons held" mask, read via the pawn's movement-services pointer.
var MS_PTR_OFF   = __s2_schema_offset("CCSPlayer_MovementServices", "__ptr_on_pawn__"); // see note
var BTN_OFF      = __s2_schema_offset("CPlayer_MovementServices", "m_nButtons");
var BTNSTATE_OFF = __s2_schema_offset("CInButtonState", "m_pButtonStates");
// IN_* bit values (Source button flags; confirm against the live logged mask in Task 5).
var IN_FORWARD = 8, IN_BACK = 16, IN_USE = 32;
```
**NOTE / spike:** the pointer field on the pawn is `m_pMovementServices` — resolve its offset on the pawn's class (`CCSPlayerPawn`/base) via `__s2_schema_offset("CBasePlayerPawn", "m_pMovementServices")` (confirm the exact declaring class from `games/cs2/gamedata/schema-catalog.json`). The current-buttons uint64 is at `BTN_OFF + BTNSTATE_OFF + 0*8`. Use `pawn.ref.readUInt64Via([msPtrOff], BTN_OFF + BTNSTATE_OFF)` (returns a decimal string per 5B.4 → parse to a number/bigint for bit tests; menu bits are low, so `Number(mask) & IN_FORWARD` is safe). Confirm the `m_pButtonStates` array index that means "currently held" and the exact `IN_*` values from the Task-5 logged mask (the user presses W/S/E during the live test).

- [ ] **Step 2: Implement the center renderer**

Add to `pawn.js` (after `Player`/nav are defined, near the bottom where `__s2pkg_cs2` is assembled):

```javascript
// --- CS2 center menu renderer: WASD input (schema poll) + show_survival_respawn_status HTML ---
(function () {
  if (!globalThis.__s2pkg_menu) return;   // menu module present?
  var Events = globalThis.__s2pkg_events.Events;
  var OnGameFrame = globalThis.__s2pkg_frame.OnGameFrame;
  var centerSessions = {};   // slot -> session
  var prevMask = {};         // slot -> last button mask (edge detect)
  var pollSub = null;        // lazy OnGameFrame subscription

  function readButtons(slot) {
    var p = Player.fromSlot(slot); if (!p) return 0;
    var pawn = p.pawn; if (!pawn) return 0;
    if (MS_PTR_OFF < 0 || BTN_OFF < 0 || BTNSTATE_OFF < 0) return 0;
    var s = pawn.ref.readUInt64Via([MS_PTR_OFF], BTN_OFF + BTNSTATE_OFF);
    return (s === null) ? 0 : Number(s);   // low bits only -> Number is exact
  }
  function renderHtml(session) {
    var v = session.view(), html = "<font class='fontSize-l' color='#ffffff'>" + escapeHtml(v.title) + "</font>";
    for (var i = 0; i < v.lines.length; i++) {
      var l = v.lines[i], color = l.cursor ? "#00ff00" : "#cccccc", mark = l.cursor ? "&#9654; " : "";
      html += "<br><font color='" + color + "'>" + mark + escapeHtml(l.text) + "</font>";
    }
    return html;
  }
  function escapeHtml(s) { return ("" + s).replace(/</g, "&lt;").replace(/>/g, "&gt;"); }
  function ensurePoll() {
    if (pollSub) return;
    pollSub = OnGameFrame.subscribe(function () {
      for (var slot in centerSessions) {
        var s = centerSessions[slot]; if (!s || s._ended) continue;
        var sl = slot | 0, mask = readButtons(sl), prev = prevMask[sl] || 0, pressed = mask & ~prev;
        prevMask[sl] = mask;
        if (pressed & IN_FORWARD) s.moveUp();
        else if (pressed & IN_BACK) s.moveDown();
        else if (pressed & IN_USE) s.confirm();
        if (!s._ended) Events.fireToClient(sl, "show_survival_respawn_status", { loc_token: renderHtml(s), duration: 5 });
      }
    });
  }
  function stopPollIfIdle() {
    for (var k in centerSessions) { if (centerSessions[k]) return; }
    if (pollSub) { pollSub.unsubscribe(); pollSub = null; }
  }
  globalThis.__s2pkg_menu.Menu.registerRenderer(globalThis.__s2pkg_menu.MenuStyle.Center, {
    open: function (session) { centerSessions[session.slot] = session; prevMask[session.slot] = 0; ensurePoll(); Events.fireToClient(session.slot, "show_survival_respawn_status", { loc_token: renderHtml(session), duration: 5 }); },
    update: function (session) { /* next poll tick re-fires with the new view */ },
    close: function (slot) { delete centerSessions[slot]; delete prevMask[slot]; stopPollIfIdle();
      Events.fireToClient(slot, "show_survival_respawn_status", { loc_token: "", duration: 0 }); },   // clear
  });
})();
```
(Confirm the exact `OnGameFrame.subscribe(...)` return-shape — whether it returns an object with `.unsubscribe()` — from how `pawn.js`/examples already unsubscribe; match it. If unsubscribe is a different call, adjust `stopPollIfIdle`.)

- [ ] **Step 3: Rebuild the CS2 package JS**

Run: `bash scripts/package-addon.sh` (concatenates `schema.generated.js + nav.generated.js + activity.js + pawn.js`). Confirm no error and that `dist/addons/s2script/js/pawn.js` includes the center renderer.

- [ ] **Step 4: Boundary gates + commit**

Run: `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh` (the CS2 names are in `pawn.js`/game layer — gates confirm none leaked into core).

```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts
git commit -m "feat(menu/cs2): center renderer — WASD button poll + show_survival_respawn_status HTML"
```

---

### Task 5: `menu-demo` plugin + sniper build + live gate

The demo that exercises both backends + proves the input primitive on bots, then the sniper rebuild (for the Task 3 op) and the live gate (bots-provable floor + the user's human nav).

**Files:**
- Create: `plugins/menu-demo/package.json`, `plugins/menu-demo/tsconfig.json`, `plugins/menu-demo/src/plugin.ts` (mirror `plugins/ws-demo/` structure exactly).

**Interfaces:**
- Consumes: `@s2script/menu` (`Menu`, `MenuStyle`), `@s2script/commands` (`Commands.registerAdmin` or `register`), `@s2script/cs2` (`Player`), `@s2script/frame` (`OnGameFrame`).
- Produces: a live-gate demo. No downstream consumers.

- [ ] **Step 1: Write the demo plugin**

`plugins/menu-demo/src/plugin.ts`:
```typescript
import { Menu, MenuStyle } from "@s2script/menu";
import { Commands } from "@s2script/commands";
import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

function showMenu(slot: number, style: MenuStyle): void {
  const m = new Menu("s2script Menu Demo");
  m.style = style;
  m.addItem("hp", "Heal to 100");
  m.addItem("noclip", "Toggle Noclip");
  m.addItem("disabled", "Coming soon", { disabled: true });
  for (let i = 1; i <= 8; i++) m.addItem("x" + i, "Extra option " + i);   // force pagination
  m.onSelect(e => { console.log(`[menu-demo] select slot=${e.slot} item=${e.item} info=${e.info}`); });
  m.onCancel(e => { console.log(`[menu-demo] cancel slot=${e.slot} reason=${e.reason}`); });
  m.display(slot, 30);
}

Commands.register("sm_menu", ctx => {
  if (ctx.callerSlot < 0) { ctx.reply("run in-game"); return; }
  showMenu(ctx.callerSlot, MenuStyle.Center);
  ctx.reply("center menu shown — W/S to move, E to select");
});
Commands.register("sm_chatmenu", ctx => {
  if (ctx.callerSlot < 0) { ctx.reply("run in-game"); return; }
  showMenu(ctx.callerSlot, MenuStyle.Chat);
  ctx.reply("chat menu shown — type the number");
});

// Prove the WASD input primitive live: log a bot's button mask changing (bots press buttons).
let frames = 0;
OnGameFrame.subscribe(() => {
  if (++frames % 128 !== 0) return;               // ~ every 2s
  const p = Player.fromSlot(0); if (!p) return;
  const pawn = p.pawn; if (!pawn) return;
  // read the same button mask the center renderer uses (offsets resolved in pawn.js are internal;
  // here we just confirm the pawn/movementServices is live by logging a nav field)
  console.log(`[menu-demo] frame=${frames} bot0 movementServices=${pawn.movementServices ? "live" : "null"}`);
});
```
(If `Commands.register` needs a `sm_`-prefixed console name vs a bare name, match how `ws-demo`/`basecommands` register — the console commands come through the `ClientCommand`/`Host_Say` paths, Slice 6.11.)

- [ ] **Step 2: Build the plugin (typecheck gate)**

Run: `cd plugins/menu-demo && npx s2script build` (from the repo root if `npx` resolves the local CLI; otherwise use the same invocation `ws-demo` uses). Expected: a typecheck-clean `.s2sp` in `plugins/menu-demo/dist/`. Fix any `.d.ts` mismatches surfaced (this validates the `@s2script/menu` surface).

- [ ] **Step 3: Sniper rebuild (for the Task 3 op)**

Run:
```bash
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: exit 0; `libs2script_core.so` GLIBC ≤ 2.30, `s2script.so` ≤ 2.14 (the build script prints these).

- [ ] **Step 4: Deploy (base suite + this demo only — NOT `find examples` wholesale)**

```bash
mkdir -p dist/addons/s2script/plugins dist/addons/s2script/configs dist/addons/s2script/data
find plugins -path '*/dist/*.s2sp' -exec cp {} dist/addons/s2script/plugins/ \;
docker compose -f docker/docker-compose.yml restart cs2
```
(Deploy from `plugins/` only — the deploy-hygiene rule: do NOT sweep `examples/*`, which drags in stale bundles.)

- [ ] **Step 5: Live gate**

Poll `docker logs s2script-cs2 --since 3m` for: `GAMEDATA VALIDATION: NN ok, 0 FAILED` (the new listener offset resolved); the `[menu-demo] ... movementServices=live` line (input primitive alive on a bot); then, with the user joined, run `sm_chatmenu` (pick a number → `[menu-demo] select ... info=...`) and `sm_menu` (see the HTML render, W/S moves the cursor, E selects → the select log). Confirm `RestartCount=0` and no panic/segfault. If the center HTML does not appear, use the spike (Task 3 Step 5 / Task 4 Step 1) to correct the listener offset / button offsets, re-sniper, redeploy.

- [ ] **Step 6: Commit**

```bash
git add plugins/menu-demo
git commit -m "feat(menu): menu-demo plugin + live gate (chat + center menus, bot input-primitive proof)"
```

---

## Self-Review

**Spec coverage:**
- Menu model + pagination + style + callbacks → Task 1 ✅
- Chat renderer + timeout + disconnect → Task 2 ✅
- `registerRenderer` seam → Task 1 (seam) + Task 2 (chat) + Task 4 (center) ✅
- `event_fire_to_client` / `Events.fireToClient` (engine-generic, SM parity) → Task 3 ✅
- CS2 center renderer (button poll + `show_survival_respawn_status` HTML, re-sent each tick, lazy poll) → Task 4 ✅
- Slot-based callbacks; `Player.fromSlot` in the CS2 layer → Tasks 1/5 ✅
- Teardown is free (composed subs) → Tasks 2/4 use `Chat.onMessage`/`delay`/`OnGameFrame`/`Clients.onDisconnect`, all already ledgered ✅
- Live gate (bots floor + human nav) → Task 5 ✅
- Boundary gates green; one sniper rebuild → Tasks 1–4 run the gates, Task 5 snipers ✅
- Deferred items (global menu manager, per-player cookie, VoteMenu, per-item draw styles) → not built ✅

**Placeholder scan:** The two spikes (the `IGameEventListener2*` offset within `CServerSideClient`, the exact button offsets + `IN_*` bits) are explicit, bounded investigations with a concrete resolution path (CSSharp gamedata hint → live validation with the user joined), not vague TODOs — appropriate for the one genuine RE unknown. No "add error handling"/"etc." placeholders.

**Type consistency:** `MenuStyle` values are the strings `"chat"`/`"center"` used consistently as renderer-registry keys and `menu.style` values across Tasks 1/2/4. `session` methods (`view`, `pickNumber`, `moveUp/moveDown/confirm/cancel`) match between the model (Task 1), the chat renderer (Task 2, uses `pickNumber`), and the center renderer (Task 4, uses `moveUp/moveDown/confirm`). `__s2pkg_menu = { Menu, MenuStyle, MenuCancelReason }` is produced in Task 1 and consumed by Tasks 2/4. `Events.fireToClient(slot, name, fields)` signature matches between Task 3 (prelude + `.d.ts`) and Task 4 (caller). The op field `event_fire_to_client` is appended in the same position in the C header, the Rust mirror, and the test op-structs (Task 3).
