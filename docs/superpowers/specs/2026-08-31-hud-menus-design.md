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
4. **Default `activation` is `immediate`.** Nominations (user-requested, currently Chat/non-freeze) become `immediate` HUD — they typed the command. Votes are not a `Menu`; they use the rail + Tab until cast.
5. **`freezePlayer` stays independent of activation.** Admin menus keep freeze. Nominations do not. Votes never freeze and never use a center sheet.
6. **Disabled items stay unselectable.** Nominations’ “played recently” rows stay visible, grey, and must not fire `onSelect`. Do not rely on `hudkit`’s cosmetic `disabled` (its `onPick` still fires). The Menu HUD renderer omits those rows from click routing.
7. **One host-lifetime modal claim for all `Menu` displays.** `hudkit.modal()` is a 2-slot *plugin* pool. The Menu renderer claims one **center** sheet at first display and never releases it. Votes do **not** take a modal slot.
8. **Votes are a right-side rail, not a Menu and not a center sheet.** Unsolicited, so they must not cover the crosshair or steal the mouse until Tab. After the player casts (HUD click or chat digit), the rail **stays up** but is no longer waiting: `HudInput.disarm`, `cursor(false)`, that player’s pick highlighted, **counts revealed and ticking**. Before they cast, option labels are visible and counts are hidden (no bandwagon). Chat digits still cast. One chat line points at the rail; do not print the option list. HTML `show_survival_respawn_status` tally is retired on CS2. One vote at a time already — one rail panel, per-player `ForPlayer`.
9. **Tab intercept is a CS2 host primitive (`HudInput`), not per-plugin `onRunCmd`.** `IN_SCORE` (`1 << 16`) is cleared on the live usercmd while a slot is armed. Live-gate must prove CS2’s scoreboard actually respects a server-side clear; if it does not, HUD still activates and the caveat is documented — do not invent a second key.
10. **GitHub stack via `gh stack`.** Bottom: `cursor/cs2-custom-hud-layout`. This spec/plan is the next layer. Implementation layers `gh stack add` on top. Not Graphite / not `gt`.

## Explicit non-goals

- A new plugin-facing widget type named MOTD / VoteHud / AdminHud.
- Rewriting plugins onto `hudkit.modal()` / `hudkit.badge()`.
- Publishing a third **modal** sheet. The vote rail is a new panel family on `s2script_lib.xml`, not another center modal.
- Removing `Menu` / `MenuStyle` / `freezePlayer`.
- Engine SUPERCEDE of `+showscores` as a ConCommand (usercmd bit only).
- Replacing toasts/badges; they are not menus.
- Making `hudkit` disabled rows refuse `onPick` globally (Menu renderer handles it; a later hudkit change may follow).
- Re-arming Tab after a player has voted so they can HUD-revote. Chat-digit revote still updates the highlight and counts.

---

## Authoring shape

```ts
import { Menu, MenuStyle, Vote } from "@s2script/sdk";

// User-requested — admin / !nominate / top menu. Cursor on immediately.
const m = new Menu("Nominate a map");
m.style = MenuStyle.Chat;          // ignored on CS2; HUD sheet either way
m.activation = "immediate";        // default; may be omitted
m.freezePlayer = false;
m.addItem("de_dust2", "de_dust2");
m.addItem("de_mirage", "de_mirage (played recently)", { disabled: true });
m.onSelect((e) => nominate(e.slot, e.info));
m.display(slot, 30);

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

plugin  →  Vote.start(...)             unsolicited ballot
                │
                ▼
         CS2 vote rail presenter  ── right-side panel (s2_vote)
                ├── waiting  → visible, no cursor, HudInput.arm
                │                 Tab → cursor; click/digit casts
                └── voted    → still visible, disarm, cursor off
                                  pick highlighted; counts tick
                                  Tab is scoreboard again
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
- Hiding is not implied. The vote rail **disarms on first cast** and leaves the panel up. Menu close still disarms **and** hides.

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

| | Waiting | Voted |
|---|---|---|
| Rail | shown | shown |
| Counts | hidden | shown, refresh on every tally tick / cast |
| `HudInput` | armed | disarmed |
| Cursor | off until Tab | off |
| Highlight | none | their option |
| Chat digit | casts (then → Voted) | revote updates highlight + counts; does not re-arm |

Vote end / `clear` hides the rail. Do not keep it up after the vote is over.

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
- Vote rail: `show` paints right-side panel, no cursor, `HudInput.isArmed`; click/digit → `choice` set, counts appear, disarmed, panel still shown; later `show` with new counts updates numbers; `clear` hides.
- Voted-state clicks do not recast via HUD.

**Live CS2 (cloud VM, after renderer PR):**

- `sm_admin` → sheet + cursor + freeze; click a category; Tab is scoreboard (immediate, not armed).
- `sm_nominate` (no arg, plugin enabled) → sheet with recent maps grey; click refuses them; click an available map nominates.
- `sm_votekick` as a non-admin victim: right-side rail visible, can move/shoot, crosshair free; Tab captures mouse instead of scoreboard; click Yes; rail stays with counts ticking; Tab is scoreboard; chat `1` still casts without Tab.
- After vote ends the rail hides.

---

## Boundary

- `Menu.activation` is engine-generic (a string on the Menu model).
- `HudInput`, IN_SCORE, cursor, freeze, hudkit paint are CS2 (`games/cs2/js`, `@s2script/cs2`).
- `Vote` remains engine-generic; CS2 registers the rail presenter. `VoteTally.choice` is a generic additive field.
- Workshop addon `3790153369` / `s2script_lib.xml` **gains** the `s2_vote*` family + right-dock CSS. Center modal pool is unchanged.
