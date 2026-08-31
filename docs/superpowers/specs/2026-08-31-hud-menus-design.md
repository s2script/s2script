# HUD menus + Tab-to-activate — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/cs2-custom-hud-layout`.
**Date:** 2026-08-31.
**Scope:** replace CS2 center WASD HTML and chat-number menus with `hudkit` sheets; votes are a non-intrusive right-side rail (Tab to pick, then stay up as a live tally).
**Verified against:** `cursor/cs2-custom-hud-layout` @ `e4051b1` (PR #148). Menu model in `core/js/prelude.js`; CS2 Center renderer + vote tally in `games/cs2/js/pawn.js`; `hudkit` in `games/cs2/js/components.js`.

---

## Why

Today every interactive list is one of two CS2-specific hacks:

- **Center / WASD** — `show_survival_respawn_status` HTML, polled `IN_FORWARD` / `IN_BACK` / `IN_USE`, optional `MOVETYPE_NONE` freeze. Admin menu, top-menu categories, `pickPlayer`, ban duration, change-map picker.
- **Chat numbers** — print 7 lines, type `1`–`0`. Nominations (mid-round, deliberately non-freezing). Votes print a chat ballot; an optional live tally reuses the same HTML event and is **display-only**.

`hudkit` already has the real surface: a shared Panorama sheet (`s2script_lib.xml`), mouse clicks via `CustomHudClicked`, disabled rows, paging. **Menus** (admin, nominate, top menu) paint a center sheet. **Votes** do not — they are a side rail, because a ballot that covers the crosshair is unsolicited noise. Tab still arms input on that rail until the player has voted; after they pick, the rail stays up as a live tally and is no longer waiting.

---

## Locked product decisions

1. **Keep `Menu` as the plugin authoring API.** Plugins continue to `new Menu()`, `addItem`, `onSelect` / `onCancel`, `display`. They do not each `hudkit.modal()`. `pickPlayer`, adminmenu, basebans, basecommands, nominations stay on `Menu`.
2. **On CS2, both `MenuStyle.Center` and `MenuStyle.Chat` paint a `hudkit` sheet.** Chat-number rendering and WASD/`show_survival_respawn_status` stop being the interactive path. `MenuStyle` values remain (plugins do not have to change style) but they no longer pick a different CS2 backend.
3. **Two activation modes**, on the menu (and on the vote rail while it is waiting):
   - **`immediate`** — player *asked* for this (typed `!nominate`, `sm_admin`, a top-menu item). Center sheet is shown and input is armed now (`cursor(true)`, honor `freezePlayer`).
   - **`tab`** — player did *not* ask. Used by the **vote rail**, not by admin/nominate. Rail is shown **without** cursor. Tab activates it. Scoreboard Tab is swallowed only while this is armed and not yet activated.
4. **Default `activation` is `immediate`.** Nominations (user-requested, currently Chat/non-freeze) become `immediate` HUD — they typed the command. Votes are not a `Menu`. The rail waits for Tab **until that player picks or the vote expires** (timer, full turnout, or cancel).
5. **`freezePlayer` stays independent of activation.** Admin menus keep freeze. Nominations do not. Votes never freeze and never use a center sheet.
6. **Disabled items stay unselectable.** Nominations’ “played recently” rows stay visible, grey, and must not fire `onSelect`. Do not rely on `hudkit`’s cosmetic `disabled` (its `onPick` still fires). The Menu HUD renderer omits those rows from click routing.
7. **One host-lifetime modal claim for all `Menu` displays.** `hudkit.modal()` is a 2-slot *plugin* pool. The Menu renderer claims one **center** sheet at first display and never releases it. Votes do **not** take a modal slot.
8. **Votes are a right-side rail, not a Menu and not a center sheet.** Unsolicited, so they must not cover the crosshair or steal the mouse until Tab. **Waiting (Tab instead of scoreboard) lasts until the player picks or the vote expires.** After they cast (HUD click or chat digit), the rail **stays up until expiry** but is no longer waiting: `HudInput.disarm`, `cursor(false)`, pick highlighted, **counts revealed and ticking**. If they never pick, expiry still disarms and **hides** the rail. Before they cast, option labels are visible and counts are hidden (no bandwagon). Chat digits still cast. One chat line points at the rail; do not print the option list. HTML `show_survival_respawn_status` tally is retired on CS2. One vote at a time already — one rail panel, per-player `ForPlayer`.
9. **Tab intercept is a CS2 host primitive (`HudInput`), not per-plugin `onRunCmd`.** `IN_SCORE` (`1 << 16`) is cleared on the live usercmd while a slot is armed. Live-gate must prove CS2’s scoreboard actually respects a server-side clear; if it does not, HUD still activates and the caveat is documented — do not invent a second key.
10. **GitHub stack via `gh stack`.** Bottom: `cursor/cs2-custom-hud-layout`. This spec/plan is the next layer. Implementation layers `gh stack add` on top. Not Graphite / not `gt`.
11. **`!menu` / `sm_menu` is the player hub over the same TopMenu registry as `sm_admin`.** Plugins **opt in** per item (`sheets: ["menu"]`). Default remains `["admin"]`, so today’s Kick/Ban/Slap stay on `sm_admin` only. The hub is a **center** HUD sheet (user-requested → immediate cursor), category → items, same drill-down as `sm_admin`. Each player only sees categories/items they can use (`flags === 0` or they have those ADMFLAGs / ROOT). An empty hub replies that nothing is available — it does not open a blank sheet.

## Explicit non-goals

- A new plugin-facing widget type named MOTD / VoteHud / AdminHud.
- Rewriting plugins onto `hudkit.modal()` / `hudkit.badge()`.
- Publishing a third **modal** sheet. The vote rail is a new panel family on `s2script_lib.xml`, not another center modal.
- Removing `Menu` / `MenuStyle` / `freezePlayer`.
- Engine SUPERCEDE of `+showscores` as a ConCommand (usercmd bit only).
- Replacing toasts/badges; they are not menus.
- Making `hudkit` disabled rows refuse `onPick` globally (Menu renderer handles it; a later hudkit change may follow).
- Merging `sm_admin` into `!menu`. Admins keep `sm_admin`. Player items opt into `!menu`. An item may list both sheets.
- Auto-publishing every TopMenu item on `!menu`. Opt-in is required (`sheets`).

---

## Authoring shape

```ts
import { Menu, MenuStyle, Vote, topmenu, ADMFLAG } from "@s2script/sdk";

// User-requested — admin / !nominate / top menu. Cursor on immediately.
const m = new Menu("Nominate a map");
m.style = MenuStyle.Chat;          // ignored on CS2; HUD sheet either way
m.activation = "immediate";        // default; may be omitted
m.freezePlayer = false;
m.addItem("de_dust2", "de_dust2");
m.addItem("de_mirage", "de_mirage (played recently)", { disabled: true });
m.onSelect((e) => nominate(e.slot, e.info));
m.display(slot, 30);

// Opt a category into the player hub (!menu / sm_menu). flags 0 = everyone.
topmenu.addCategory("Maps");
topmenu.addItem("Maps", {
  id: "nominations:open",
  name: "Nominate",
  flags: 0,
  sheets: ["menu"],
  onSelect: (slot) => nominateMenu(slot),
});

// Existing admin items stay admin-only unless they also pass sheets: ["admin", "menu"].
topmenu.addItem("Player Commands", {
  id: "basebans:kick",
  name: "Kick",
  flags: ADMFLAG.KICK,
  onSelect: (adminSlot) => { /* pickPlayer … */ },
});

// Votes are not a Menu. Vote.start paints a right-side rail (CS2 presenter).
Vote.start({
  question: "Kick Rex?",
  options: ["Yes", "No"],
  duration: 20,
  onEnd: (r) => { /* … */ },
});
```

Direct hudkit users (rare) arm the same Tab primitive on whatever they painted:

```ts
import { hudkit, HudInput } from "@s2script/cs2";

HudInput.arm(slot, {
  onActivate: () => hudkit.layout.forSlot(slot).cursor(true),
});
```

`Menu.display` with `activation: "tab"` is `HudInput.arm`. Close / select / disconnect is `HudInput.disarm`. The vote rail arms on show, and **disarms on that player’s first cast** without hiding.

---

## Architecture

```
plugin  →  Menu.display(slot)          user-requested lists
                │
                ▼
         CS2 HUD renderer  ── center hudkit sheet (s2_mN)
                └── activation immediate → cursor [+ freeze]

plugin  →  topmenu.addItem(..., { sheets: ["menu"] })
player  →  !menu / sm_menu            same center sheet, flag-filtered
admin   →  sm_admin                   same registry, sheets: ["admin"]

plugin  →  Vote.start(...)             unsolicited ballot
                │
                ▼
         CS2 vote rail presenter  ── right-side panel (s2_vote)
                ├── waiting  → until pick OR vote expires
                │                 visible, no cursor, HudInput.arm
                │                 Tab → cursor; click/digit casts
                ├── voted    → until vote expires
                │                 still visible, disarm, cursor off
                │                 pick highlighted; counts tick
                │                 Tab is scoreboard again
                └── expired  → hide rail, disarm (whether they voted or not)
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
- Disconnect: `Clients.onDisconnect` → disarm. `hud.forget(slot)` already drops per-player HUD state; HudInput must too.
- Do not subscribe `UserCmd.onRun` until at least one slot is armed (same lazy pattern as the current Center poll).
- Hiding is not implied by `disarm`. The vote rail **disarms on first cast** and leaves the panel up **until the vote expires**. `clear` (timer, full turnout, or `Vote.cancel`) always disarms **and** hides, including for a player who never picked. Menu close still disarms **and** hides.

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

### Votes — right-side rail

Not a `Menu`. Not a center sheet. A dedicated hudkit panel docked **right, vertical center** (CS2 radar/money already own the left). Probe CSS already has this geometry as `.s2-list`; the lib vote rail is the same idea with clickable rows.

```
                    ┌──────────────┐
                    │ Kick Rex?    │
                    │ Tab to vote  │
                    │              │
                    │  Yes         │
                    │  No          │
                    └──────────────┘
         crosshair →                 ← rail (ignore-parent-flow)
```

After that player casts:

```
                    ┌──────────────┐
                    │ Kick Rex?    │
                    │ 12s · 4 voted│
                    │              │
                    │ ▶ Yes     3  │  ← their pick, counts live
                    │   No      1  │
                    └──────────────┘
```

**States per slot**

| | Waiting | Voted | Expired |
|---|---|---|---|
| Until | pick **or** vote ends | vote ends | — |
| Rail | shown | shown | hidden |
| Counts | hidden | shown, live | — |
| `HudInput` | armed | disarmed | disarmed |
| Cursor | off until Tab | off | off |
| Highlight | none | their option | — |
| Chat digit | casts (then → Voted) | revote updates highlight + counts; does not re-arm | n/a |

**Waiting ends** when that player picks **or** the vote expires (duration elapses, every eligible voter has voted, or `cancel`). Expiry hides the rail for everyone. Do not leave a waiting Tab-hijack up after the vote is over.

**Presenter** reuses `Vote.registerTallyRenderer`: `show(slot, tally)` paints/updates the rail; `clear` hides it. If a renderer is registered, prelude **always** calls it (drop the `showLiveTally` early-return). CS2 always registers, so every vote gets a rail. Non-CS2 stays chat-only. `showLiveTally` on `VoteConfig` is leftover and ignored when a renderer exists.

**Cast path:** extract a shared `__s2_vote_cast(slot, index)` used by chat digits and by rail clicks. Do not add a public `Vote.cast`. Voted-state rail clicks no-op; chat still revotes through the same helper.

**`VoteTally` grows `choice: number | null`** — this slot’s 0-based cast, or null. Prelude sets it per `show(slot, …)` from `st.votes.get(slot)`. Presenter uses it to pick Waiting vs Voted without a second channel.

**XML/CSS (workshop addon 3790153369, `s2script_lib.xml`)** — this stack *does* add a panel family. Commit the fragment under `examples/hud-lab/workshop/` and republish the addon. One root, not a pool (Vote is already one-at-a-time):

- `s2_vote` — root, `horizontal-align: right; vertical-align: center;` compact column, no freeze, does not cover the crosshair
- `s2_vote_q` / `s2_vote_sub` — question + subtitle (timer, or “Tab to vote”)
- `s2_vote_o0`…`s2_vote_o8` — option buttons (Vote is 2..9). Hide unused with `s2-hide`
- `s2_vote_oN_t` / `s2_vote_oN_c` — label and count. Count label collapsed until `choice !== null`

Optional later: `s2-vote-left` class. v1 is right only.

**Chat:** one line (`[Vote] question — Tab, or type 1–N`). Stop printing the per-option list.

**Clicks:** option buttons registered in `spec.buttons`. Waiting+active → click casts. Voted → clicks ignored (rail is display). Chat revote still allowed.

`Vote` stays engine-generic. CS2 registers the rail presenter; prelude does not import hudkit.

### Player hub (`!menu` / `sm_menu`)

Same TopMenu registry, same center HUD sheet, different filter. This is not a second widget.

**`TopMenuSheet`:** `"admin"` | `"menu"`. Stored on the **item** (plugins opt in where they already `addItem`). Default `["admin"]` — existing Kick/Ban/Slap registrations do not appear on `!menu`.

```ts
topmenu.addItem("Maps", {
  id: "nominations:open",
  name: "Nominate",
  flags: 0,                 // everyone
  sheets: ["menu"],         // player hub only
  onSelect: (slot) => nominateMenu(slot),
});
```

An item may pass `sheets: ["admin", "menu"]` to show on both hubs.

**Access:** an item is visible to `slot` when `flags === 0`, or the player is ROOT, or they have every bit in `flags` (same test as today’s `adminmenu`). A category is listed when it has ≥1 visible item **on that sheet**.

**Commands** (owned by `plugins/adminmenu`, the TopMenu renderer):

| Command | Sheet | Who |
|---|---|---|
| `sm_admin` | `"admin"` | in-game admins only (unchanged deny for non-admins) |
| `sm_menu` / `!menu` | `"menu"` | any in-game player |

`!menu` is the chat form of `sm_menu` (command system already maps `!` to `sm_`). Cookbook currently owns `sm_menu` — rename that demo to `sm_menudemo` in the hub PR.

**Empty hub:** if the player has zero visible `"menu"` items, reply (do not open a sheet). Same idea as adminmenu’s “No Admin Actions Available”.

**Render:** extract `showSheet(slot, sheet)` from today’s category → item drill-down. Immediate HUD, freeze like `sm_admin` (they asked). `TopMenu.select` on pick, unchanged.

**Core:** `__s2_topmenu_add_item` grows an optional sheets payload (prelude default `"admin"`). `snapshot().items[]` includes `sheets: TopMenuSheet[]`. Isolate tests: default admin; menu-only item absent from `sm_admin` filter; flags 0 visible to a non-admin on the menu sheet.

**Dogfood:** nominations registers `Maps` / Nominate with `sheets: ["menu"]`, `flags: 0`, `onSelect` → existing `nominateMenu`. Do not put Slap on `!menu`.

### Plugin cutover

Because the renderer swap is CS2-global, **most plugins need no logic change**:

| Caller | Today | After |
|---|---|---|
| `adminmenu` | Center + freeze; `sm_admin` only | same + `showSheet`; `!menu` / `sm_menu` player hub |
| `basecommands` map picker | Center + freeze | same |
| `basebans` duration | Center + freeze | same |
| `pickPlayer` | Center + freeze | same |
| `nominations` | Chat, disabled recent | HUD immediate; also opted into `!menu` as Maps / Nominate |
| cookbook `sm_menu` | Center demo | renamed `sm_menudemo` (hub takes `sm_menu`) |
| hud-lab demos | Center or Chat | HUD |
| `basevotes` / RTV / funvotes | `Vote.start` chat + optional center HTML tally | right-side rail; Tab until cast, then live counts |

Comment-only edits where they say “WASD” / “type the number”. Nominations must keep `{ disabled: true }` on cooldown maps.

---

## Tab / scoreboard risk

`third_party/hl2sdk/game/shared/in_buttons.h`: `IN_SCORE (1 << 16) // Used by client.dll for when scoreboard is held down`.

CS2’s scoreboard may be client-predicted. The live gate is:

1. Start a vote on a real client (rail visible, waiting).
2. Press Tab: cursor on the **right-side rail**, scoreboard does **not**.
3. After they vote (or the menu closes), Tab opens the scoreboard again. The vote rail may still be on the right showing counts.
4. While the rail is already cursor-active (Tabbed, not yet voted), further Tab may open the scoreboard (locked: we stop swallowing after activate).

If step 2 still shows the scoreboard, record it and ship activate-anyway. Do not bind a different key in this stack.

---

## Testing

**Offline (required per implementation PR):**

- Menu HUD renderer: `open` paints title + rows; disabled row is not in click handlers; `onSelect` does not fire for it.
- `activation: "tab"` does not call `cursor(true)` on open; `HudInput.isArmed`.
- Simulated `IN_SCORE` while armed: `onActivate` once, bit cleared, `isActive`, second tick with held Tab still cleared until release; after activate, bit left intact.
- New menu on the same slot: previous disarmed (`NewMenu`).
- Disconnect: disarm.
- Vote rail: `show` paints right-side panel, no cursor, `HudInput.isArmed`; click/digit → `choice` set, counts appear, disarmed, panel still shown; later `show` with new counts updates numbers; `clear` hides **and** disarms a slot that never voted.
- TopMenu `sheets`: default `["admin"]`; menu-only item is omitted from the admin filter; `flags === 0` is visible to a non-admin on the menu sheet.
- `!menu` with no opted-in items replies; does not open a sheet.

**Live CS2 (cloud VM, after renderer PR):**

- `sm_admin` → sheet + cursor + freeze; click a category; Tab is scoreboard (immediate, not armed).
- `sm_nominate` (no arg, plugin enabled) → sheet with recent maps grey; click refuses them; click an available map nominates.
- `sm_votekick` as a non-admin victim: right-side rail visible, can move/shoot, crosshair free; Tab captures mouse instead of scoreboard; click Yes; rail stays with counts ticking; Tab is scoreboard; chat `1` still casts without Tab.
- `!menu` as a non-admin: only Maps/Nominate (once opted in); click opens nominate HUD. `sm_admin` still denies that player.
- `!menu` with nothing opted in: chat reply, no sheet.

---

## Boundary

- `Menu.activation` is engine-generic (a string on the Menu model).
- `HudInput`, IN_SCORE, cursor, freeze, hudkit paint are CS2 (`games/cs2/js`, `@s2script/cs2`).
- `Vote` remains engine-generic; CS2 registers the rail presenter. `VoteTally.choice` is a generic additive field.
- TopMenu `sheets` + snapshot field are engine-generic (core registry). `sm_menu` / `!menu` rendering stays in `adminmenu` (CS2 plugin).
