# Plan — CS2 UI module (`Ui` / `Hud` on `@s2script/cs2`)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Status:** Phase 1 is the only executable slice in this document. Phases 2–3 and the schema-dump
retirement are named stubs / an appendix. Do not start them in the Phase 1 PR.

**Repaired (2026-08-26).** The first draft of this file was a design essay. Four reviews against the
Claude-created hud-lab artifacts found it unimplementable as written. This rewrite is the slice
plan. Locked decisions are below; do not re-litigate them in the PR.

**Prereq proven live:** CS2 build 24934554, server `nebula.gkh.dev:27019`, workshop addon
`3790153369` (Unlisted). Engine work (`utlstring`, `this_i64_i64_i64`) already landed. Phase 1 is
gamedata promotion + game-package wrap. **No core. No shim.**

---

## Goal

A plugin that depends on `@s2script/cs2` can, with MultiAddonManager already forcing workshop addon
`3790153369`:

1. find-or-create a module-owned `custom_hud_layout` **after** the map is up
2. bind the one shipped descriptor for the committed `s2script_hud.xml`
3. `show` / `setText` / `setPool` / `setMeter` / `onClick`
4. receive `(slot, buttonId)` from the promoted hook

without `@s2script/sdk/unsafe`, without plugin gamedata, and without `engine:calls` /
`engine:hooks`.

That is the exit. “Add one dependency and get a working HUD” is false: the operator still needs MAM
+ the published VPK. Say so in the boot banner.

---

## Architecture

hud-lab talks to the engine through **plugin** `Engine.call` / `Engine.hook`. The game package talks
through `__s2_game_call_*` and `__s2_hook_on("@s2script/cs2", …)` (`games/cs2/js/pawn.js`). Copying
`hudlib.ts` does not produce `@s2script/cs2`.

Phase 1 is therefore:

1. Promote the two `ForPlayer` calls and `onCustomHudClicked` from
   `examples/hud-lab/gamedata/hud-lab.gamedata.jsonc` into `gamedata/cs2/game.cs2.jsonc`.
2. Author `games/cs2/js/ui.js` as a new concatenated IIFE (the weapon.js / pawn.js pattern), porting
   behavior from **hudlib + tierb + hudstate + layout + the click subscribe in plugin.ts**.
3. Export `Ui` / `Hud` / `LayoutDescriptor` from the `@s2script/cs2` **barrel** (`packages/cs2` is
   types only; the engine injects the implementation).
4. Thin hud-lab to the live-gate demo.

`packages/cs2/ui.d.ts` is types. There is no runtime `@s2script/cs2/ui` module. That path is the
econ types-only subpath; a `require("@s2script/cs2/ui")` would become `__s2pkg_cs2/ui` and be
`undefined`.

---

## Locked decisions

| Question | Call |
|---|---|
| Import | `import { Ui, Hud } from "@s2script/cs2"` — **not** `@s2script/cs2/ui` |
| Clicks | Generated `ctx.hud.onCustomHudClicked` is the raw mux. `Hud.onClick` / `dispatchClick` is the typed router. `ui.js` subscribes once and fans in. |
| How many layouts | **One.** The committed `examples/hud-lab/workshop/panorama/layout/custom_game/s2script_hud.xml`. The published VPK’s other six layouts are not in git. Do not hand-write them from memory. |
| `id == varName` | **False** for the committed kit. `Hud.set()` stays as a helper for layouts that adopt the convention. The default descriptor uses an explicit `text` map and `setText`. |
| Meter classes | Align JS to committed CSS: `s2-w0` … `s2-w10` at **10%** steps. hudlib’s `s2-w-0` … `s2-w-100` (5%) is a contract bug against the stylesheet that exists. |
| `addonId` | New field on `LayoutDescriptor` (hudlib does not have it). Registry reads it for `requiredAddons()`. |
| Entity lifecycle | Explicit find-or-create **after** the world is up. **Never** spawn in `OnMapStart` (segfault, proven). Owned entity: reserved `targetname`, `origin: "0 0 0"`, `layout` ending in `.xml`. Default layouts never go through `preferred()` (that drives a map-authored zoo panel). |
| Client mount detection | **We cannot detect it.** Delete the “per-player mount state / no-op unmounted players” claim. Always drive. Unsubscribed clients already render nothing. `requiredAddons()` is an operator banner, not a detector. `panelIds === 0` is interned names, not “addon missing”. |
| Input capture | Still a probe-gated raw offset write. No `SetInputCaptureEnabled` signature. Port `offsets.ts` + `probeLayout` + **per-player** `setPlayerInputCapture` into `ui.js`. The global bool does not give a player a cursor. |
| Schema dump | **Follow-up**, not a Phase 1 prereq. Keep the probe-gated path and say it is temporary. |
| hud-lab after | Keep as the live-gate demo + probe harness. Delete duplicated setters once unused. Keep `sm_hud_probe` / `sm_hud_diff` / `offsets.ts` until the schema-dump stub. |
| Global setters | Still unlocated. Phase 1 is per-player only. |
| Core / shim | None. `utlstring` and `this_i64_i64_i64` already landed. |
| Branch | `games/cs2-ui`. One squash PR. |
| Phase 2.1 / 2.2 / 3 | Not this PR. Stubs / appendix below. |

---

## What is already proven (do not re-litigate)

| | |
|---|---|
| Custom layout renders from a mounted workshop addon on a stock map | ✅ de_dust2 |
| `SetHasClassForPlayer` / `SetDialogVariableStringForPlayer` | ✅ armed via `utlstring` |
| HUD clicks → `(player, buttonId)` | ✅ via `this_i64_i64_i64` — **as a plugin hook today**, not yet as `ctx.hud` |
| Per-player input capture | ✅ raw write of `m_bInputCaptureEnabled` on the **per-player** state |
| A panel is invisible until that player has per-player state | ✅ `show` is two calls (clear hide class **and** cursor) |
| `m_strLayout` must use the `.xml` **source** extension | ✅ `.vxml` / `.vxml_c` rejected |
| Spawned entity needs `origin: "0 0 0"` or the client never builds it | ✅ |
| `OnMapStart` spawn segfaults | ✅ |
| `AllowCustomGameUI` is `1` on build 24934554; addons may only add layouts under `panorama/layout/custom_game/` | ✅ |

---

## The constraint that shapes the API

**Panels cannot be created at runtime.** No `appendChild`, no templating, no client scripting
(Valve's `point_script.d.ts`). A layout declares a FIXED POOL of slots; the server can only toggle
classes and set dialog-variable strings.

This is a **pool manager with a typed face**. Every collection has a hard maximum baked into markup.
`setPool` **refuses** overflow rather than truncating.

Further traps, already handled in `hudlib.ts` and required in the port:

- Classes **accumulate**. Meter steps and themes must clear the previous class first.
- `disabled` is cosmetic. `.s2-btn-disabled` (add to CSS if missing) greys a button; the click still
  arrives. `setDisabled` + `dispatchClick` enforce it server-side.
- Animated hide is two-phase (`visibility: collapse` cannot be transitioned) and needs a generation
  guard (or a killed timer) so a re-fire does not yank the previous animation.
- The root `<Panel>` may not have an `id`. A theme class goes on the first child.

---

## Global constraints

- **Branch:** `games/cs2-ui`. Never commit to `main`. One slice, one PR, squash-merged.
- **No `core/` or `shim/`.** If a descriptor cannot express a rule, the rule lives in `ui.js` (same
  as pawn.js). Do not add a hook shape or an arg kind.
- **Do not spawn `custom_hud_layout` from `Server.onMapStart` / `OnMapStart`.**
- **`layout` keyvalue is `.xml` only.** Reject `.vxml` / `.vxml_c` in `Ui` before `createEntity`.
- **Do not edit operator-facing comments in `game.cs2.jsonc` as if they were optional.** The
  “DO NOT EDIT” banner is for **operators** on a live install (upgrades overwrite;
  `gamedata/cs2/custom/` is the overlay). Maintainers **do** edit this file in-repo. Copy the live
  signature comments from hud-lab **verbatim** (the crash story is the treadmill recipe). Do **not**
  copy the stale “ALL FOUR CALLS ARE WITHDRAWN” banner — the body re-armed them.
- **Promote only the three live signatures.** Leave the `DE AD BE EF` global-setter stubs in
  hud-lab (or delete them). They are not confirmed.
- **Every promoted signature gets a `validate` block.** Game-package descriptors carry one. Add a
  `prologue` (prefix of the existing pattern) to both `ForPlayer` signatures. The click receiver
  already has one — keep it.
- **`scripts/package-addon.sh` concat is load-bearing.** `cs2-addon.mjs` and
  `games/cs2/js/eslint.config.mjs` **parse** that `cat` line. Miss `ui.js` and offline tests plus
  lint never see it. Append `ui.js` **after** `pawn.js` (needs `__s2pkg_cs2_calls`; pawn.js assigns
  `__s2pkg_game_ctx` and does not merge — `ui.js` must `Object.assign` a `hud` factory onto it).
- **ES5 IIFE in `games/cs2/js/ui.js`.** `var`, `__s2require`, `globalThis.__s2pkg_cs2`. No TypeScript
  in that directory. No `Engine.call` / `Engine.hook`.
- **Do not run** docker, `scripts/build-sniper.sh`, or the live CS2 gate unless the human asks.
  Offline tests and `CI=1 make ci-js` (non-Docker) are the agent gate. The human runs nebula.
- **Do not invent workshop source.** `s2script_kit.xml`, `s2script_hud_live`, and the other VPK
  layouts are not in this tree. Phase 1 ships the committed `s2script_hud` descriptor only.
- **Do not vendor the compiled VPK** in the s2script tarball. Ship the workshop id. Republishing
  our XML is allowed; bundling Valve UGC is a ToS question — don’t.
- **Do not link `IMultiAddonManager`.** It is GPL-3. Phase 1 stays at the `mm_extra_addons` **cvar**.
- **Changeset required.** `@s2script/cs2` is published (`0.13.0`). `scripts/check-changeset.sh` fails
  without one.

---

## File structure

| File | Responsibility |
|---|---|
| `gamedata/cs2/game.cs2.jsonc` | Promoted signatures + `calls` + `onCustomHudClicked` |
| `games/cs2/js/ui.js` | Runtime `Ui` / `Hud` IIFE |
| `scripts/package-addon.sh` | Concat `ui.js` after `pawn.js` |
| `packages/cs2/ui.d.ts` | Types for `Ui`, `Hud`, `LayoutDescriptor`, `SlotPool`, `HudResult` |
| `packages/cs2/index.d.ts` | Barrel re-export |
| `packages/cs2/package.json` | `files` allow-list (+ `ui.d.ts`). **No** `exports["./ui"]` runtime |
| `packages/cs2/hooks.generated.d.ts` | Regenerated by `s2s gen-hooks` (`CtxHud`) |
| `games/cs2/js/pawn.js` | `__s2pkg_cs2_calls` already exists; do **not** overwrite `__s2pkg_game_ctx` |
| `packages/sdk/test/cs2-engine-calls.test.mjs` | Offline wrapper tests (recording fakes, real concat bundle) |
| `examples/hud-lab/**` | Live-gate consumer; drop promoted plugin gamedata |
| `.changeset/*.md` | `@s2script/cs2` minor |

---

## Public API (Phase 1)

```ts
interface SlotPool {
  ids: readonly string[];
  vars: readonly string[];
}

interface LayoutDescriptor {
  addonId: string;                 // workshop id; new field
  resource: string;                // `.xml` source path; reject any other extension
  hideClass: string;
  text: Readonly<Record<string, string>>;  // panelId -> dialog var
  buttons: readonly string[];
  pools: Readonly<Record<string, SlotPool>>;
  meters: Readonly<Record<string, string>>; // name -> fill panel id
}

type HudResult = string | null;    // null = success

interface Ui {
  register(name: string, desc: LayoutDescriptor): void;
  setDefault(name: string): void;
  requiredAddons(): string[];      // unique addonId values; boot-banner this
  /** Find-or-create the module-owned entity. Null if the world is not up. Never OnMapStart. */
  ensure(): EntityRef | null;
  hud(): Hud;                      // default descriptor, owned entity
  hud(name: string): Hud;
  bind(entity: EntityRef, name: string): Hud;  // map-authored
}

interface Hud {
  readonly layout: LayoutDescriptor;
  show(slot: number, panelId: string, withCursor?: boolean): HudResult;
  hide(slot: number, panelId: string): HudResult;
  cursor(slot: number, on: boolean): HudResult;
  set(slot: number, id: string, value: string | number): HudResult;      // id == varName only
  setText(slot: number, panelId: string, value: string): HudResult;      // uses layout.text
  setClass(slot: number, panelId: string, className: string, on: boolean): HudResult;
  setMeter(slot: number, meterName: string, percent: number): HudResult; // → s2-w0..s2-w10
  capacity(poolName: string): number;
  setPool(slot: number, poolName: string, entries: readonly string[][]): HudResult;
  setTheme(slot: number, rootPanelId: string, themeClass: string | null): HudResult;
  onClick(buttonId: string, handler: (slot: number) => void): void;
  setDisabled(slot: number, buttonId: string, off: boolean): HudResult;
  dispatchClick(slot: number, buttonId: string): boolean;
  forget(slot: number): void;
}
```

`showAnimated` / `hideAnimated` / `flash` may be ported internally. Do **not** advertise them on the
Phase 1 type surface unless `.s2-fade` is in the committed CSS **and** the published VPK (a CSS add
needs a workshop republish; do not gate the slice on that). Server-side `setDisabled` enforcement
ships even if `.s2-btn-disabled` is missing from the published stylesheet.

Default descriptor name: `"s2script_hud"`.

```
addonId:  "3790153369"
resource: "panorama/layout/custom_game/s2script_hud.xml"
hideClass: "s2-hidden"
buttons:  ["s2_btn_0", "s2_btn_1", "s2_btn_2", "s2_btn_3"]
meters:   { meter: "s2_meter_fill" }
pools.rows.ids:  s2_row_0 .. s2_row_7
pools.rows.vars: row0 .. row7
text: {
  s2_dialog_kicker: kicker, s2_dialog_title: title, s2_dialog_body: body,
  s2_btn_0_text: btn0, s2_btn_1_text: btn1, s2_btn_2_text: btn2, s2_btn_3_text: btn3,
  s2_hud_tl_head: tl_head, s2_hud_tl_body: tl_body,
  s2_hud_tr_head: tr_head, s2_hud_tr_body: tr_body,
  s2_hud_bl_head: bl_head, s2_hud_bl_body: bl_body,
  s2_hud_br_head: br_head, s2_hud_br_body: br_body,
  s2_banner_text: banner,
  s2_list_title: list_title, s2_row_0: row0, … s2_row_7: row7, s2_list_foot: list_foot,
  s2_meter_label: meter_label
}
```

Owned entity `targetname`: `s2_ui_layout` (do not reuse hud-lab’s `s2_hudlab_layout` so the demo
can still tell them apart if both exist).

---

## Operator path (print this at boot)

The module does **not** download, mount, or inject workshop addons.

```
[cs2/ui] required workshop addons: 3790153369
[cs2/ui] set mm_extra_addons to that list in game/csgo/cfg/multiaddonmanager/multiaddonmanager.cfg
[cs2/ui] MAM: present (3790153369 listed) | present (3790153369 MISSING) | ABSENT
```

Presence: `Server.getCvar("mm_extra_addons")`. Missing/throwing cvar → `ABSENT`.

If absent:

```
[cs2/ui] MultiAddonManager not detected. Clients must already be subscribed to 3790153369.
[cs2/ui] +host_workshop_map will not deliver this content addon (IsPlayable=false, no map inside).
```

Exact MAM value: `mm_extra_addons "3790153369"` (decimal id, quoted, no `workshop/` prefix).
`mm_client_extra_addons` does **not** download or mount on the server — do not tell operators to
use it. MAM reloads the map after the first download; the first join is N reconnects (one addon
per reconnect). Keep the item **Unlisted or Public** (Private → `Access Denied`).

`docker/docker-compose.yml` does not run this path. Live proof is a Steam-authed dedicated server
(nebula), not `make docker-test`.

---

### Task 1 — Promote HUD descriptors into the CS2 owner

**Files:**
- Modify: `gamedata/cs2/game.cs2.jsonc`
- Modify: `packages/cs2/hooks.generated.d.ts` (via codegen, not by hand)
- Later (Task 5): delete the promoted entries from `examples/hud-lab/gamedata/hud-lab.gamedata.jsonc`

**Copy from hud-lab (live body, not the withdrawn banner):**

- Signatures: `CCSCustomHudLayout_SetHasClassForPlayer`,
  `CCSCustomHudLayout_SetDialogVariableStringForPlayer`,
  `CCSCustomHudLayout_CustomHudClickedReceiver`
- Calls: `setHasClassForPlayer` (`args: ["int", "utlstring", "utlstring", "int"]`),
  `setDialogVariableStringForPlayer` (`args: ["int", "utlstring", "utlstring", "utlstring"]`)
- Hook: `onCustomHudClicked` — keep `shape: "this_i64_i64_i64"`, `params: ["buttonId"]`,
  `receiver: { kind: "entity", as: "player" }`, `expose: { ctx: "hud" }`. **No `bypassWith`.**
  This is an observer. Suppressing it would stop the zoo map’s own Dismiss handler.

Add a `prologue` validator to both `ForPlayer` signatures (they have none today). Keep the click
receiver’s existing prologue.

Update the “eight descriptors” comment in `game.cs2.jsonc` and the matching comment in
`games/cs2/js/pawn.js` (~line 57).

- [ ] **Step 1:** Append the three signatures + two calls + one hook to `game.cs2.jsonc`.
- [ ] **Step 2:** Run `node packages/sdk/dist/cli.js gen-hooks` (or `s2s gen-hooks`).
- [ ] **Step 3:** Confirm `packages/cs2/hooks.generated.d.ts` contains `CtxHud` and
      `OnCustomHudClickedView` with `buttonId: string` and `player: EntityRef | null`.

**Verification:**

```bash
node packages/sdk/dist/cli.js gen-hooks --check
bash scripts/check-call-descriptors.sh
make check-boundary
```

**Exit:** A game-package plugin can subscribe to clicks without plugin gamedata **once Task 3
wires `ctx.hud`**. Codegen alone does not install the factory.

---

### Task 2 — `games/cs2/js/ui.js` (rewrite, not a file move)

**Files:**
- Create: `games/cs2/js/ui.js`
- Modify: `scripts/package-addon.sh` (append `games/cs2/js/ui.js` to the `cat` line **after**
  `pawn.js`; update the comment block)

**Port from, by file:**

| Source | What to take |
|---|---|
| `examples/hud-lab/src/tierb.ts` | Null-if-unarmed wrappers; slot `< 0` refuses (global variants unlocated) |
| `examples/hud-lab/src/layout.ts` | `create` / `findOwned` / `removeOwned`; `origin: "0 0 0"`; `.xml` only |
| `examples/hud-lab/src/hudstate.ts` + `offsets.ts` | `probeLayout`; **per-player** `setPlayerInputCapture`; copy the caveat |
| `examples/hud-lab/src/hudlib.ts` | Pools (refuse overflow), meters (now `s2-w0`..`s2-w10`), theme clear-previous, `onClick` + `dispatchClick` + `forget` |
| `examples/hud-lab/src/plugin.ts:90-121` | Click → `Player` by `index` **and** `id` (no `Player.fromEntity`); fan in to `dispatchClick` |

Resolve calls via `globalThis.__s2pkg_cs2_calls.call` / `.status` (published by pawn.js). Capture at
eval time is fine **because `ui.js` is concatenated after pawn.js**.

```js
var calls = globalThis.__s2pkg_cs2_calls;
var setHasClassForPlayer = calls && calls.call("setHasClassForPlayer");
var setDialogVariableStringForPlayer = calls && calls.call("setDialogVariableStringForPlayer");
```

Subscribe to the promoted hook once at IIFE eval and route into every live `Hud`:

```js
// Resolve the clicker the same way plugin.ts does: index+id, never index alone.
// Then hud.dispatchClick(slot, buttonId) on each bound instance.
```

Install the raw mux on ctx **without overwriting pawn.js’s object:**

```js
globalThis.__s2pkg_game_ctx = Object.assign({}, globalThis.__s2pkg_game_ctx, {
  hud: function (reg, viaId) {
    return {
      onCustomHudClicked: function (h) {
        reg(viaId(function () { return __s2_hook_on("@s2script/cs2", "onCustomHudClicked", h); }));
      },
    };
  },
});
```

Meter helper (committed CSS, not hudlib’s 5% family):

```js
function meterClassFor(percent) {
  var clamped = Math.max(0, Math.min(100, percent));
  var stepped = Math.round(clamped / 10);   // 0..10
  return "s2-w" + stepped;
}
```

Timers for any animated helper: `__s2require("@s2script/sdk/timers").after`, not `async/await`.
Kill the previous timer on re-fire (stricter than hudlib’s generation counter, same outcome).

Boot banner: `Ui.requiredAddons()` + `Server.getCvar("mm_extra_addons")` as specified above. Fire
once at IIFE eval.

Merge onto the barrel:

```js
globalThis.__s2pkg_cs2 = Object.assign({}, globalThis.__s2pkg_cs2, { Ui: Ui, Hud: Hud });
```

- [ ] **Step 1:** Write `ui.js` IIFE with `ensure` / `create` / `hud` / `bind` / default descriptor.
- [ ] **Step 2:** Append it to `package-addon.sh`. Confirm `cs2-addon.mjs` picks it up
      (`node -e "import('./packages/sdk/test/cs2-addon.mjs').then(m => console.log(m.cs2AddonBundle.includes('required workshop addons')))"`
      after a distinctive string exists).
- [ ] **Step 3:** `cd games/cs2/js && npx --no-install eslint .` (or `scripts/check-core-js-lint.sh`).

**Exit:** Packaged `dist/addons/s2script/js/pawn.js` contains the ui IIFE;
`globalThis.__s2pkg_cs2.Ui` survives pawn.js’s `Object.assign`; `ctx.hud` exists.

---

### Task 3 — Types on the barrel

**Files:**
- Create: `packages/cs2/ui.d.ts`
- Modify: `packages/cs2/index.d.ts` — `export { Ui, Hud } from "./ui"` plus the types
- Modify: `packages/cs2/package.json` — add `ui.d.ts` to `files`. Do **not** add
  `exports["./ui"]` unless it is types-only sugar **and** you document that
  `require("@s2script/cs2/ui")` is not a runtime module. Prefer no subpath.

Follow Weapon (`export { Weapon } from "./weapon"`), not econ.

`OnCustomHudClickedView.player` stays `EntityRef | null` (generated). Do **not** mark the hook
`handwritten: true` in Phase 1. `Hud.onClick` is the `Player`-slot surface.

- [ ] **Step 1:** Author `ui.d.ts` to match the Public API section.
- [ ] **Step 2:** Re-export from `index.d.ts`; add to `files`.
- [ ] **Step 3:** Scratch typecheck: a file with `import { Ui } from "@s2script/cs2"` compiles.

**Exit:** `import { Ui } from "@s2script/cs2"` typechecks. `Ui.hud` / `ctx.hud` are different
objects and both are documented.

---

### Task 4 — Offline tests

**Files:**
- Modify: `packages/sdk/test/cs2-engine-calls.test.mjs`
- Possibly: the `makeHost` fakes (need `createEntity` / `Entity.findByClass` / `writeBoolVia` /
  `__s2_hook_on` if you exercise those paths)

Add the two HUD call names to the host’s `ready` set (today `ALL_CALLS` is the original eight;
`getPlayerMaxSpeed` is already a pawn.js call that file does not list — do not “fix” that in this
PR unless a test you add requires it).

Required cases:

1. `setHasClassForPlayer` invoked as `(entity, slot, panelId, className, status)` — status is the
   enum int, not a bool.
2. `setDialogVariableStringForPlayer` invoked as `(entity, slot, panelId, varName, value)`.
3. `setPool` overflow **refuses** and does **not** invoke.
4. `setMeter(50)` applies `s2-w5` (and clears a previous step if one was set).
5. Degraded descriptor (call omitted from `ready`) returns a named reason and does not throw.
6. `Ui.requiredAddons()` is `["3790153369"]`.
7. `Ui.ensure` does not call `createEntity` when a find-by-class already returns the owned
   targetname (if you can fake `Entity.findByClass`).

```bash
node --test packages/sdk/test/cs2-engine-calls.test.mjs
```

**Exit:** The real concat bundle is what ran. A wrapper rule cannot drift without going red.

---

### Task 5 — Thin hud-lab; fix the stale lab

**Files:**
- Modify: `examples/hud-lab/src/plugin.ts` — `sm_kit` / show / hide / click drive `Ui`
- Modify: `examples/hud-lab/src/panelctl.ts` — `S2_KIT.layout` becomes
  `panorama/layout/custom_game/s2script_hud.xml` (the file that exists). `S2_PROBE.panelId` becomes
  `s2_dialog`, not zoo `"dialog"`
- Modify: `examples/hud-lab/src/tierb.ts` — delete once unused
- Modify: `examples/hud-lab/package.json` — drop `engine:calls` / `engine:hooks` and the
  `s2script.gamedata` block once nothing in the plugin needs them
- Delete or empty: `examples/hud-lab/gamedata/hud-lab.gamedata.jsonc` promoted entries (do not
  leave a second copy of the signatures to drift)
- Modify: `examples/hud-lab/src/hudlib.ts` — either delete (game package owns it) or leave a
  one-line “moved to `@s2script/cs2`” pointer. Do **not** keep a second `Hud` implementation
- Keep: `offsets.ts`, `hudstate.ts` probe/diff helpers used by `sm_hud_probe` / `sm_hud_diff`,
  camera / bomb / movetype / demohud harness
- Stale comments (fix in the same PR; they are currently lying):

| File | Lie |
|---|---|
| `gamedata/hud-lab.gamedata.jsonc` header | “ALL FOUR CALLS ARE WITHDRAWN” |
| `src/plugin.ts:5-12`, `:160-164` | Tier C UNREACHABLE / no `this_i64` |
| `src/plugin.ts` COMMANDS list | omits `sm_kit` / show / hide / cursor / demo |
| `workshop/README.md` | clicks missing; `sm_hud_create …s2script_hud.vxml`; `s2-w7`; cites missing `docker/docker-compose.hudlab.yml` |
| `examples/hud-lab/README.md` | Tier B withdrawn; custom HUD unusable |
| `src/demohud.ts`, `src/panel.ts` | setters / clicks unreachable |

- [ ] **Step 1:** Retarget `sm_kit` to `Ui.hud()` + the shipped descriptor. Keep the command.
- [ ] **Step 2:** Drop plugin gamedata / unsafe permissions once unused.
- [ ] **Step 3:** Rewrite the stale banners so the next update does not re-learn a solved crash.

**Verification:**

```bash
bash scripts/check-plugins-typecheck.sh
```

**Exit:** hud-lab typechecks with `import { Ui } from "@s2script/cs2"` and is no longer a second
implementation of the setters.

---

### Task 6 — Changeset, docs, agent gate

**Files:**
- Create: `.changeset/cs2-ui.md` — `@s2script/cs2` minor
- Short `docs/PROGRESS.md` entry if that file is where slices land
- Do not invent an ARCHITECTURE chapter

```md
---
"@s2script/cs2": minor
---

Add `Ui` / `Hud` — a first-class custom_hud_layout driver. Plugins import `{ Ui }` from
`@s2script/cs2` (no unsafe, no plugin gamedata). Operator still mounts workshop addon 3790153369
via MultiAddonManager (`mm_extra_addons`).
```

- [ ] **Step 1:** Changeset.
- [ ] **Step 2:** Agent gate:

```bash
node --test packages/sdk/test/cs2-engine-calls.test.mjs
bash scripts/check-plugins-typecheck.sh
bash scripts/check-call-descriptors.sh
node packages/sdk/dist/cli.js gen-hooks --check
make check-boundary
CI=1 make ci-js
```

**Exit:** Mergeable Phase 1 PR. Live checklist is for the human (below).

---

## Live gate (human, nebula.gkh.dev:27019)

Addon `3790153369` mounted via MAM, stock map (`de_dust2`). First: confirm the VPK still contains
`panorama/layout/custom_game/s2script_hud.vxml_c`. If the published item drifted from git, either
retarget the resource string after inspection or republish the committed workshop source. Do not
guess.

| Command | Pass |
|---|---|
| Boot log | `[cs2/ui] required workshop addons: 3790153369` and a truthful MAM line |
| `sm_hud_create` / `Ui.ensure` | entity spawned, `origin` set, `layout` ends in `.xml`, probe OK |
| `sm_kit` | dialog vars land **on the client**; cursor on |
| click `s2_btn_0`…`s2_btn_3` | server log + `Hud.onClick` / `ctx.hud.onCustomHudClicked` both see it |
| `setPool` overflow | refuse string, no silent truncate |
| `setMeter(50)` | fill uses `s2-w5` (10% family) |
| Second client without the addon | no crash; their panel is blank; server still drove |

A green RCON “OK” with a blank client is a **fail**. The client load dialog is the CSS/XML
validator; `resourcecompiler` is not.

---

## Later slices (do not start)

### Stub A — `s2s ui-gen` from source XML

**Do not start until** Phase 1 is merged and the hand-written `s2script_hud` descriptor is the
golden comparison.

Walk `id`, `{s:var}`, `<Button id>`. Emit `text` + `buttons` + `resource` (force `.xml`).
**Do not guess** `hideClass`, `pools`, `meters`, or `addonId` — those are flags or `TODO` fields.

Verification: fixture XML → golden descriptor; rename a panel id → typecheck fails.

### Stub B — `s2s ui-gen <workshop-id>`

**Own plan. Not a footnote.** Needs a VPK v2 reader, `RED2`/`DATA`/`LaCo`, LZ4 **block** decoder,
KV3 **v5** (`05 33 56 4B` + 16-byte GUID) with back-references. Raw string scrape is wrong.
None of that is in this repo.

**Call:** do not implement a Source 2 decompiler in `packages/sdk`. If this is ever needed, shell
out to ValveResourceFormat / Source2Viewer-CLI, then run Stub A on the XML. Public/unlisted items
only (Private → `Access Denied`).

**Do not start until** Stub A exists.

### Stub C — schema dump, retire `offsets.ts`

**Own PR. Not a Phase 1 prereq.**

`tools/schema-dump` needs a running server. `games/cs2/gamedata/schema-catalog.json` predates the
HUD classes. `CCSCustomHudLayout` is **not** in `games/cs2/codegen-classes.json`. Two offsets are
structurally confirmed (`m_vecPlayerLayoutStates` = 1936, `m_globalLayoutState` = 2040); the rest
are borrowed.

Done: dump on nebula’s build → catalog contains `CCSCustomHudLayout` /
`CCSCustomHudLayoutState` → add them to `codegen-classes.json` → regen → delete the copied
offsets from `ui.js` → generated accessors for the capture bool.

### Recovering the other six layouts (optional, not a task here)

The published VPK has `s2script_showcase`, `s2script_kit`, `s2script_hud_live`, `s2script_game`,
`s2script_dash`, `s2script_components` plus styles `s2script_lib` / `kit` / `hud`. Those sources
were authored in Workshop Tools and **never committed**. Until they are decompiled and checked in
under `examples/hud-lab/workshop/panorama/`, they are not ours to describe. Default stays the
committed `s2script_hud.xml`.

---

## Appendix — Phase 3 (MAM-in-process). Out of scope.

Do not start until MAM is operationally painful **and** this is the only change in its PR.

- `AddSearchPath` of `steamapps/workshop/content/730/{ID}/{ID}_dir.vpk` is the easy half. It does
  not deliver to clients.
- Client delivery is MAM’s reconnect loop: `ReplyConnection` assigns the list;
  `CServerSideClient::SendNetMessage` intercepts `SIGNONSTATE_CHANGELEVEL` and injects one addon
  via `pMsg->set_addons(...)`; client downloads, reconnects, repeat. Also `CHLTVClient` and
  `SetPendingHostStateRequest`. A wrong hook breaks **every** connection, not UI.
- `docs/re-strategy.md` applies. MAM source is a hint, never a signature. Facts must be
  self-resolved on **our** binaries and fail loud. “Hook shapes are a small add” is false here.
- `m_sRequiredAddons` is not how MAM delivers; do not plan around it.
- Behind a default-off flag. Never in the same change as the UI module.
- Do not link `IMultiAddonManager` from the shim (GPL-3).

---

## What this plan no longer claims

- That `hudlib.ts` “becomes” `games/cs2/js/ui.js`. It is a rewrite across a types-only package and
  a concatenated IIFE, and hudlib is not even the live path (`sm_kit` uses panelctl/tierb).
- That there are seven in-repo layouts / that `s2script_hud_live` has 44 bindings we can type.
- That `Ui.hud(slot)` already exists or that `LayoutDescriptor.addonId` was moved from hudlib.
- That we can track per-player addon mount and no-op the unmounted.
- That `@s2script/cs2/ui` is a runtime module.
- That Phase 1 retires `offsets.ts`.
- That Phase 3 is “four signatures and a small hook-shape add.”
- That `panelIds === 0` means the layout asset failed to load.
- That clicks are still unreachable (plugin hook is live; first-class `ctx.hud` is this slice).
