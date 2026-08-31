# HUD menus + Tab-to-activate — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/cs2-custom-hud-layout`.
**Date:** 2026-08-31.
**Scope:** replace CS2 center WASD HTML and chat-number menus with `hudkit` sheets; Tab (scoreboard) activates a HUD that is waiting for input when the player did not ask for it.
**Verified against:** `cursor/cs2-custom-hud-layout` @ `e4051b1` (PR #148). Menu model in `core/js/prelude.js`; CS2 Center renderer + vote tally in `games/cs2/js/pawn.js`; `hudkit` in `games/cs2/js/components.js`.

---

## Why

Today every interactive list is one of two CS2-specific hacks:

- **Center / WASD** — `show_survival_respawn_status` HTML, polled `IN_FORWARD` / `IN_BACK` / `IN_USE`, optional `MOVETYPE_NONE` freeze. Admin menu, top-menu categories, `pickPlayer`, ban duration, change-map picker.
- **Chat numbers** — print 7 lines, type `1`–`0`. Nominations (mid-round, deliberately non-freezing). Votes print a chat ballot; an optional live tally reuses the same HTML event and is **display-only**.

`hudkit` already has the real surface: a shared Panorama sheet (`s2script_lib.xml`), mouse clicks via `CustomHudClicked`, disabled rows, paging. Menus should paint that, not fake HTML. Votes that appear unsolicited must stay playable until the player opts in — **Tab**, which would otherwise open the scoreboard, is that opt-in.

---

## Locked product decisions

1. **Keep `Menu` as the plugin authoring API.** Plugins continue to `new Menu()`, `addItem`, `onSelect` / `onCancel`, `display`. They do not each `hudkit.modal()`. `pickPlayer`, adminmenu, basebans, basecommands, nominations stay on `Menu`.
2. **On CS2, both `MenuStyle.Center` and `MenuStyle.Chat` paint a `hudkit` sheet.** Chat-number rendering and WASD/`show_survival_respawn_status` stop being the interactive path. `MenuStyle` values remain (plugins do not have to change style) but they no longer pick a different CS2 backend.
3. **Two activation modes**, on the menu (and on any other HUD that waits for a click):
   - **`immediate`** — player *asked* for this (typed `!nominate`, `sm_admin`, a top-menu item). Sheet is shown and input is armed now (`cursor(true)`, honor `freezePlayer`).
   - **`tab`** — player did *not* ask (map vote, kick vote, RTV ballot). Sheet is shown **without** cursor. Tab activates it. Default scoreboard Tab is swallowed only while this is armed and not yet activated.
4. **Default `activation` is `immediate`.** Push UIs set `tab` explicitly. Nominations (user-requested, currently Chat/non-freeze) become `immediate` HUD — they typed the command. Votes set `tab`.
5. **`freezePlayer` stays independent of activation.** Admin menus keep freeze. Nominations and votes do not freeze. Tab-activate does not freeze unless the menu asked for it.
6. **Disabled items stay unselectable.** Nominations’ “played recently” rows stay visible, grey, and must not fire `onSelect`. Do not rely on `hudkit`’s cosmetic `disabled` (its `onPick` still fires). The Menu HUD renderer omits those rows from click routing.
7. **One host-lifetime modal claim for all `Menu` displays.** `hudkit.modal()` is a 2-slot *plugin* pool. The Menu renderer claims one sheet at first display and never releases it. Per-player `ForPlayer` already lets every slot show a different menu on that same sheet. Remaining `hudkit.modal()` capacity is 1 until a workshop XML bump adds a third sheet (out of this stack).
8. **Votes use `Menu` + `activation: "tab"`**, not a second HUD type. Chat digits still *cast* (existing `Vote` path) so a player who never Tabs can still vote. The chat *list* of options is replaced by a one-line hint that Tab opens the vote. The live tally HTML renderer is retired once the HUD ballot paints counts.
9. **Tab intercept is a CS2 host primitive (`HudInput`), not per-plugin `onRunCmd`.** `IN_SCORE` (`1 << 16`) is cleared on the live usercmd while a slot is armed. Live-gate must prove CS2’s scoreboard actually respects a server-side clear; if it does not, HUD still activates and the caveat is documented — do not invent a second key.
10. **GitHub stack via `gh stack`.** Bottom: `cursor/cs2-custom-hud-layout`. This spec/plan is the next layer. Implementation layers `gh stack add` on top. Not Graphite / not `gt`.

## Explicit non-goals

- A new plugin-facing widget type named MOTD / VoteHud / AdminHud.
- Rewriting plugins onto `hudkit.modal()` / `hudkit.badge()`.
- Publishing a new workshop XML in this stack (no third modal sheet yet).
- Removing `Menu` / `MenuStyle` / `freezePlayer`.
- Engine SUPERCEDE of `+showscores` as a ConCommand (usercmd bit only).
- Replacing toasts/badges; they are not menus.
- Making `hudkit` disabled rows refuse `onPick` globally (Menu renderer handles it; a later hudkit change may follow).

---

## Authoring shape

```ts
import { Menu, MenuStyle } from "@s2script/sdk";

// User-requested — admin / !nominate / top menu. Cursor on immediately.
const m = new Menu("Nominate a map");
m.style = MenuStyle.Chat;          // ignored on CS2; HUD sheet either way
m.activation = "immediate";        // default; may be omitted
m.freezePlayer = false;
m.addItem("de_dust2", "de_dust2");
m.addItem("de_mirage", "de_mirage (played recently)", { disabled: true });
m.onSelect((e) => nominate(e.slot, e.info));
m.display(slot, 30);

// Push — Vote.start does this internally for every eligible slot.
const ballot = new Menu(question);
ballot.activation = "tab";
ballot.freezePlayer = false;
for (const opt of options) ballot.addItem(opt, opt);
ballot.onSelect((e) => Vote.cast(e.slot, e.item));
ballot.display(slot, duration);
```

Direct hudkit users (rare) arm the same Tab primitive:

```ts
import { hudkit, HudInput } from "@s2script/cs2";

const sheet = hudkit.modal({ title: "Map vote", rows, onPick });
sheet?.forSlot(slot).open();          // visible, no cursor
HudInput.arm(slot, {
  onActivate: () => hudkit.layout.forSlot(slot).cursor(true),
});
```

`Menu.display` with `activation: "tab"` is `HudInput.arm`. Close / select / disconnect is `HudInput.disarm`.

---

## Architecture

```
plugin  →  Menu.display(slot)     [@s2script/menu, engine-generic]
                │
                ▼
         MenuSession + renderer seam (prelude)
                │
                ▼
     CS2 HUD renderer (games/cs2/js)  ── paints hudkit sheet s2_mN via ForPlayer
                │
                ├── activation immediate → HudPlayer.cursor(true) [+ freeze]
                └── activation tab       → HudInput.arm(slot)
                                              │
                                              ▼
                                    UserCmd.onRun: IN_SCORE rising
                                      clear bit, onActivate once
                                      (cursor on, optional freeze)
```

### `Menu.activation`

Add on the generic `Menu` class (`core/js/prelude.js`, `packages/sdk/menu.d.ts`):

```ts
type MenuActivation = "immediate" | "tab";
// Menu.activation, default "immediate"
```

Unknown values coerce to `"immediate"`. Non-CS2 games: `"tab"` degrades to immediate (no HudInput). Chat renderer in core, if ever used without a CS2 HUD renderer, ignores it (typing a number is already opt-in).

### `HudInput` (CS2)

Lives in `games/cs2/js` (concat after `components.js`), typed on `@s2script/cs2`.

```ts
export declare const HudInput: {
  /** Arm Tab-to-activate for `slot`. Replaces any previous arm on that slot. */
  arm(slot: number, opts: { onActivate: () => void }): void;
  /** Drop the arm. Safe if nothing is armed. Does not close the HUD. */
  disarm(slot: number): void;
  isArmed(slot: number): boolean;
  /** True after Tab (or an immediate activate) until disarm. */
  isActive(slot: number): boolean;
};
```

Rules:

- One arm per slot. A newer `Menu.display` already supersedes (`MenuCancelReason.NewMenu`); the renderer disarms then re-arms.
- `onActivate` runs **once** on the first tick where `IN_SCORE` is set while armed and not yet active. Further Tab while active: do **not** keep swallowing — player may open the scoreboard over an already-interactive HUD.
- Swallow = `cmd.buttons &= ~IN_SCORE` (`1n << 16n`) on that tick (and while the key is held, until released, so the scoreboard does not flash). After activation, subsequent holds of Tab are left alone.
- Disconnect: `Clients.onDisconnect` → disarm. `hud.forget(slot)` already drops modal open-state; HudInput must too.
- Do not subscribe `UserCmd.onRun` until at least one slot is armed (same lazy pattern as the current Center poll).

### HUD Menu renderer

Replaces the Center renderer IIFE in `games/cs2/js/pawn.js` (WASD + `show_survival_respawn_status`). Also registers for `MenuStyle.Chat` on CS2 so Chat-styled plugins (nominations) get the sheet.

Paint path:

1. Claim `hudkit.modal(...)` once (host tag `"s2:menu"`), keep it. If the pool is already exhausted, log and fall back to the **existing Chat renderer** (do not silently show nothing).
2. `open` / `update`: map `session.view()` → modal rows (`a` = display text, `disabled` from the item). Title = menu title. Footer: Close if `exitButton`. Paging uses hudkit’s own pager (`pageSize` 8 vs Menu’s 7 — **drive hudkit paging from the session page**, or set `pageSize` to 7 so Menu and sheet agree). Prefer **Menu session as source of truth**: pass the current page’s lines in as `rows` (already paged), hide hudkit pager, keep Menu Back/Next as footer buttons that call `session` page verbs.
3. `onPick`: only for selectable item lines; call the same path as `session.confirm()` on that item. Disabled lines must not be in the pickable set (filter; do not depend on hudkit).
4. Close footer → `session.cancel()`.
5. `activation === "immediate"`: after paint, `layout.forSlot(slot).cursor(true)` + existing freeze helper.
6. `activation === "tab"`: paint, **do not** cursor, `HudInput.arm`.
7. `close`: hide sheet for that slot, unfreeze, disarm, `cursor(false)`.

Keep the freeze/unfreeze helpers in pawn.js; they are pawn `moveType` facts.

### Votes

`Vote.start` today: chat capture + optional tally HTML.

Change (CS2 only, inside the vote module or a CS2 registration that wraps start):

- For each eligible slot, `display` a Menu of the options with `activation: "tab"`, `freezePlayer: false`, `exitButton: false` (leaving does not cancel the server vote).
- On select: existing cast (same as typing the digit). Revote still allowed.
- Refresh counts onto the open sheet (`detail` or row `b`/`c`) when `showLiveTally` would have repainted. If the player has not Tab-activated, they still *see* updated counts (ForPlayer text) without cursor.
- One chat line: Tab to vote / digits still work. Stop printing the per-option chat list.
- Retire `registerTallyRenderer` HTML path on CS2 once the sheet paints; keep the seam for non-CS2.

`Vote` stays engine-generic. CS2 registers a “ballot presenter” analogous to today’s tally renderer, so prelude does not import hudkit.

### Plugin cutover

Because the renderer swap is CS2-global, **most plugins need no logic change**:

| Caller | Today | After |
|---|---|---|
| `adminmenu` | Center + freeze | same Menu; immediate HUD + freeze |
| `basecommands` map picker | Center + freeze | same |
| `basebans` duration | Center + freeze | same |
| `pickPlayer` | Center + freeze | same |
| `nominations` | Chat, disabled recent | same Menu; HUD immediate, recent still disabled |
| cookbook / hud-lab demos | Center or Chat | HUD |
| `basevotes` / RTV / funvotes | `Vote.start` chat | Vote presenter (tab HUD) |

Comment-only edits where they say “WASD” / “type the number”. Nominations must keep `{ disabled: true }` on cooldown maps.

---

## Tab / scoreboard risk

`third_party/hl2sdk/game/shared/in_buttons.h`: `IN_SCORE (1 << 16) // Used by client.dll for when scoreboard is held down`.

CS2’s scoreboard may be client-predicted. The live gate is:

1. Arm a tab-menu on a real client.
2. Press Tab: cursor appears on the sheet, scoreboard does **not**.
3. After a pick/close, Tab opens the scoreboard again.
4. While the sheet is already active, Tab may open the scoreboard (locked: we stop swallowing after activate).

If step 2 still shows the scoreboard, record it and ship activate-anyway. Do not bind a different key in this stack.

---

## Testing

**Offline (required per implementation PR):**

- Menu HUD renderer: `open` paints title + rows; disabled row is not in click handlers; `onSelect` does not fire for it.
- `activation: "tab"` does not call `cursor(true)` on open; `HudInput.isArmed`.
- Simulated `IN_SCORE` while armed: `onActivate` once, bit cleared, `isActive`, second tick with held Tab still cleared until release; after activate, bit left intact.
- New menu on the same slot: previous disarmed (`NewMenu`).
- Disconnect: disarm.
- Vote presenter: eligible slots get a tab Menu; cast on pick; chat digit still casts.

**Live CS2 (cloud VM, after renderer PR):**

- `sm_admin` → sheet + cursor + freeze; click a category; Tab is scoreboard (immediate, not armed).
- `sm_nominate` (no arg, plugin enabled) → sheet with recent maps grey; click refuses them; click an available map nominates.
- `sm_votekick` as a non-admin victim: sheet visible while moving/shooting; Tab captures mouse instead of scoreboard; click Yes; after close Tab is scoreboard.
- Chat `1` still casts during a vote without Tab.

---

## Boundary

- `Menu.activation` is engine-generic (a string on the Menu model).
- `HudInput`, IN_SCORE, cursor, freeze, hudkit paint are CS2 (`games/cs2/js`, `@s2script/cs2`).
- `Vote` remains engine-generic; CS2 registers the ballot presenter.
- Workshop addon `3790153369` / `s2script_lib.xml` is unchanged in this stack.
