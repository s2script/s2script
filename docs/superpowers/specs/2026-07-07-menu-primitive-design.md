# Menu Primitive — Design

**Status:** Approved (brainstorm complete — both backends, plugin-picks-style, Approach A), ready for the plan.
**Slice:** an interactive player-menu primitive — SourceMod's `Menu` at parity, with a chat renderer and a WASD center-screen renderer. Unblocks `adminmenu`, `basevotes`, `funvotes`, and the `clientprefs` `sm_settings`/`sm_cookies` surface.

## Goal

Give plugins interactive, paginated player menus — `new Menu(title)` + `addItem` + `display(slot, seconds)` + `onSelect`/`onCancel`, with two display styles the plugin picks per menu:
- **Chat** — numbered options in the player's chat, pick by typing the number. Zero new RE.
- **Center** — a center-screen menu navigated with W/S and selected with E (the modern CS2 look).

The plugin-facing `Menu` API is identical regardless of style; the renderer is a swappable backend.

## Motivation & context

The menu primitive is the biggest remaining gap on the SourceMod base-plugin-parity north star. `adminmenu`, `basevotes`, `funvotes`, and the `clientprefs` settings surface all sit on SourceMod's `Menu` handle. SourceMod's `Menu` is **display-agnostic** — a plugin creates a menu, adds items, and displays it; it does not care whether the render is chat, radio, or panel. So the one abstraction unblocks a whole class of base plugins, and the display backend is an implementation detail we can extend without breaking consumers.

## Scope

**In scope:** the `Menu` model (items, pagination, style, select/cancel callbacks); the **chat renderer**; the **CS2 center renderer** (button-schema input polling + a center usermessage); the `registerRenderer` seam; `@s2script/menu` (engine-generic) + the CS2 center renderer in the game layer; one new `client_center_html` shim op; a bots-provable live gate.

**Deferred (named follow-ons):** a **global menu manager** in core (cross-plugin coordination + SourceMod's multi-menu **stack/priority** per client — the MVP is one active menu per player *per plugin*, context-local); a **per-player style cookie** (`sm_settings` "menu style" — plugin picks for now; clientprefs is available to layer it on later); **`VoteMenu`/`displayToAll`** (belongs to the `basevotes` slice); per-item **draw styles** beyond enabled/disabled (spacers, per-item control); a **generic (non-CS2) center renderer**; **HTML** center rendering if the CS2 center usermessage turns out to be plain-text only (plain-text center menu is the fallback).

## Approach (decided)

**Approach A — `@s2script/menu` engine-generic + a CS2 renderer, mirroring the DB `Driver` seam.** The menu model, pagination, chat renderer, and the `registerRenderer` seam are engine-generic and **slot-based** in every callback (`{ slot, item, info }`), exactly like `@s2script/commands` — so the module never imports `@s2script/cs2`. The CS2 layer registers a `"center"` renderer that owns the only game-specific facts (button-mask schema fields + the center usermessage). Base plugins resolve `Player.fromSlot(slot)` themselves.

Rejected — **Approach B (whole `Menu` in `@s2script/cs2`):** simpler now, but buries a generic menu model in the game package (re-extracted the moment a second game appears) and fails the charter litmus ("would it be true on a different Source 2 game?").

## Architecture

One-way deps (game → core). `@s2script/menu` is engine-generic; the CS2 center renderer depends on it (and on `@s2script/cs2` schema access), never the reverse.

### The `Menu` model (core, pure + unit-testable)

State per menu: `title`, `style` (`Center` | `Chat`), `exitButton` (default `true`), `items[] = { info: string, display: string, disabled: boolean }`, and the `onSelect`/`onCancel` handlers.

The framework tracks **one active menu session per player, per plugin**. A new `display(slot, …)` for a slot that already has an open session *in the same plugin* replaces it — the old session cancels with reason `NewMenu`. Because module runtimes are injected per plugin context (V8 contexts are isolated), each plugin's `@s2script/menu` tracks only its own sessions; **cross-plugin coordination** (SourceMod's global menu manager/stack, which serializes two plugins menuing the same player at once) is **deferred** — the MVP's rare collision (two plugins showing a center menu to the same slot simultaneously) renders both, an accepted limitation until the manager follow-up.

**Pagination is automatic:**
- **Chat:** up to 7 selectable items rendered as `1`–`7`; then control keys are appended to the remaining number keys — `8` = Back (only if `page > 0`), `9` = Next (only if more pages follow), `0` = Exit (only if `exitButton`). Disabled items are shown without a number and are not selectable.
- **Center:** a scrolling window with a `▶` cursor over the selectable items; lists longer than the window scroll as the cursor moves past the edge; an `Exit` line is appended when `exitButton`.

**The session → renderer contract.** The core owns page/cursor state and item resolution; a renderer paints and reports input through two input idioms on the `session`:
- `pickNumber(n)` — a number-key pick (chat). The core maps `n` to a selectable item (→ `onSelect`) or a control (Next/Back/Exit → repaint or cancel).
- `moveUp()` / `moveDown()` / `confirm()` / `cancel()` — cursor navigation (center). `confirm()` on an item → `onSelect`; on a control line → the control's action.

After any state change the core calls the renderer's `update(session)` to repaint. `onSelect` receives `{ slot, item, info, display }` where `item` is the index into the full item list; `onCancel` receives `{ slot, reason }`.

### The renderer seam

`Menu.registerRenderer(name, renderer)`. A renderer implements:
- `open(session)` — begin displaying for `session.slot`.
- `update(session)` — repaint (page/cursor changed, or a periodic refresh).
- `close(slot)` — stop displaying (menu ended: selected/cancelled/timeout/disconnect).

The renderer reads the current view via `session.view()` (core-computed: the visible lines, which are selectable, the cursor position, the control keys). The built-in chat renderer registers as `"chat"`; the CS2 layer registers `"center"`. `MenuStyle.Chat`/`.Center` selects which renderer a menu uses; if the requested renderer is unregistered (e.g. `Center` on a future non-CS2 game with no center renderer), the menu falls back to `"chat"` with a logged warning.

### Chat renderer (generic, `@s2script/menu`)

Paints via `Chat.toSlot` (colored, per-player). While a menu is open for slot `S`, it subscribes once to `Chat.onMessage`; a message from `S` that is a bare digit matching an on-screen key calls `session.pickNumber(n)` and returns `HookResult.Handled` to swallow the chat line. Works today, zero RE.

### CS2 center renderer (game layer, `pawn.js` / `@s2script/cs2`)

- **Input (schema polling — no detour):** an `OnGameFrame` poll is subscribed **lazily** (only while ≥ 1 center menu is open in this context; unsubscribed when the last closes, so idle plugins pay nothing). Each tick, for every slot with an open center menu, read the pawn's `movementServices` button mask and **rising-edge-detect** against a per-slot previous-mask snapshot: `IN_FORWARD` → `moveUp`, `IN_BACK` → `moveDown`, `IN_USE` → `confirm`. The button state lives at `CPlayer_MovementServices.m_nButtons` (`CInButtonState.m_pButtonStates`, `uint64[3]`); a short spike confirms which array index is "currently held" (`m_flForwardMove`/`m_flSideMove` are the movement-axis fallback for W/S). Reads ride existing serial-gated pawn access, so a player who leaves mid-menu degrades to no input and is closed on disconnect.
- **Display:** send the center usermessage via the new `client_center_html` op with the rendered menu, **re-sent every few ticks** because center messages fade. The rendered text is HTML if the center usermessage supports it, plain text otherwise.

### Lifecycle, safety, teardown

- **Timeout:** `display(slot, seconds)` with `seconds > 0` arms a `delay` that cancels the session with reason `Timeout`; `seconds === 0` keeps it open until selection/cancel/disconnect.
- **Disconnect:** `@s2script/menu` subscribes `Clients.onDisconnect` and closes any open session for that slot with reason `Disconnect`.
- **Teardown is free — no new ledger resource.** A menu is composed entirely of already-ledgered primitives: a `Chat.onMessage` subscription, an `OnGameFrame` poll (center), and `delay` timers. Plugin unload tears those down through their own modules' ledgers, and the menu stops. The menu module holds no raw resource of its own.
- **Slot-based, no raw pointers.** The center input poll uses serial-gated pawn access; a null pawn (player left) yields no input rather than a crash.
- **Re-entrancy:** `onSelect`/`onCancel` run in the creating plugin's context. If `onSelect` opens another menu, that is a fresh session (the current one is already closing).

## The new engine primitive

`client_center_html(slot, text)` — a center-screen message to one client via the same protobuf-reflection path proven for SayText2 (`FindNetworkMessagePartial` → `AllocateMessage` → set fields via reflection → `PostEventAbstract`), bot-skip-guarded like `client_print`. ABI-appended after the last existing op (C header + Rust mirror + both test op-structs), with a degrade-safe stub (no-op when interfaces/the message type are unavailable).

**Discovery risk (the one real unknown):** *which* CS2 usermessage renders a center-screen message, and whether it accepts HTML. The spike inspects candidate messages (`FindNetworkMessagePartial` on `TextMsg`/center-hint/HTML-panel names, dump their fields as SayText2 was dumped). If a center message exists but is plain-text only, the center menu renders plain text (still fully navigable). The chat backend and the input primitive do not depend on this, so the slice always delivers a working menu.

## The API

```ts
import { Menu, MenuStyle, MenuCancelReason } from "@s2script/menu";

const m = new Menu("Admin Menu");
m.style = MenuStyle.Center;                 // or MenuStyle.Chat — the plugin picks
m.exitButton = true;                         // auto Exit control (default true)
m.addItem("kick", "Kick Player");            // (info, display)
m.addItem("ban",  "Ban Player", { disabled: true });
m.onSelect(e => { /* e.slot, e.item (index), e.info === "kick", e.display */ });
m.onCancel(e => { /* e.reason: Exit | Timeout | Disconnect | NewMenu */ });
m.display(slot, 20);                          // show to slot for 20s; 0 = until closed
m.close(slot);                               // close early (optional)
```

- `MenuStyle`: `Chat | Center`.
- `MenuCancelReason`: `Exit | Timeout | Disconnect | NewMenu`.
- Pagination is automatic (7 items/page in chat; a scrolling window in center); Next/Back/Exit are appended by the core.
- All callbacks are **slot-based**; a CS2 plugin resolves `Player.fromSlot(e.slot)` itself.

## Testing & live gate

- **Core unit tests** (`@s2script/menu`, pure model, node:test like `schemagen`): pagination chunking + control-key layout across multiple pages; `pickNumber` → item-vs-control resolution (including the `8`/`9`/`0` control keys appearing only when applicable); cursor `moveUp`/`moveDown` with wrap/scroll + `confirm` on item vs control; disabled-item skipping (chat number omitted, center cursor skips).
- **In-isolate tests:** the chat renderer against a fake `Chat` (render lines on `open`, a digit message on `Chat.onMessage` fires `onSelect`, a non-matching message is passed through); registerRenderer fallback when a style's renderer is absent.
- **Live gate (bots-provable):** a `menu-demo` plugin registers a command that (1) shows a **chat menu** and a **center menu** — the rendered lines / the `client_center_html` send happen and do not crash; (2) **proves the WASD input primitive live** by reading a bot's live button mask each frame via `pawn.movementServices` and logging it non-zero/changing (bots press movement/attack buttons even though they will not navigate our menu); (3) pick/pagination correctness is covered by the unit tests. RestartCount stays 0.
- **Deferred human-client test:** an actual player navigating W/S/E and selecting an item end-to-end (same ceiling as SayText2-visual and damage-real-bullet — the mechanism is proven live, the human nav is not bot-reachable).
- **Gates:** core-boundary (`@s2script/menu` engine-generic — no game names), the name-leak gate, typecheck, full `cargo test`, and the `check-*` freshness gates if any generated artifact is touched (none expected).

## Boundary & safety summary

`@s2script/menu` (model + chat renderer + seam) is engine-generic and slot-based — no game identifiers, no dependency on `@s2script/cs2`. The CS2 center renderer, the `client_center_html` op, and the button-mask schema reads are the only CS2-specific parts and live in the game/shim layer. One sniper rebuild (the `client_center_html` shim op). Both boundary gates stay green.
