# hud-lab — test harness for the custom-HUD CS2 update

A single plugin that exercises everything the update added, and reports precisely what it cannot
reach. Start the server, run `sm_hud_status`, and read the three tiers.

## The thing to understand first

The update's headline APIs — `CustomHudLayout.SetHasClass`, `Instance.OnCustomHudClicked`,
`CSPlayerPawn.GetCamera` — are **cs_script** APIs. cs_script is Valve's per-map JavaScript VM.
s2script is a different V8 host, so none of those functions are callable from a plugin. This
harness reimplements what is reachable from the server side and names what is not.

Everything sorts into three tiers:

| Tier | Meaning | How to fix |
|---|---|---|
| **A** | Reachable today | — |
| **B** | Needs a byte signature | Derive it, drop it into `gamedata/hud-lab.gamedata.jsonc` |
| **C** | Needs a core change | A framework slice, not a plugin change |

## Tier A — works now

No RE, no codegen, no new core capability.

| Patch-note API | Reached via |
|---|---|
| `CustomHudLayout.SetInputCaptureEnabled` / `IsInputCaptureEnabled` | `m_bInputCaptureEnabled` is a plain bool embedded at a fixed offset — `writeBool` + `notifyStateChanged` |
| `custom_hud_layout` entity / pawn-owned camera | `CustomHudLayout.create` / `Pawn.getCustomCamera` |
| `SetHUDVisibility` | entity-IO `acceptInput` — a Valve datadesc input on `CBasePlayerPawn` |
| `Instance.OnBombPlantStart` / `OnBombPlantAbort` / `OnBombDefuseStart` / `OnBombDefuseAbort` | `bomb_beginplant` / `bomb_abortplant` / `bomb_begindefuse` / `bomb_abortdefuse` — already in the 272-event catalog |
| `Entity.GetMoveType` / `SetMoveType`, `CSMoveType` | `Pawn.moveType` + the generated `MoveType_t` |
| `CSPlayerPawn.IsBuyMenuOpen` | generated `isBuyMenuOpen` |
| `C4.GetPlantFinishTime`, `CSPlayerPawn.GetC4` | `wrapEntity("CC4", …)` + held-weapon scan |
| `Instance.Delay`, `Instance.IsDedicatedServer` | `delay()`, `Server` |
| SourceMod menus, buy menu | `@s2script/sdk/menu` + `giveNamedItem` |

## Tier B — withdrawn (it crashed the server)

`SetHasClass`, `SetHasClassForPlayer`, `SetDialogVariableString`,
`SetDialogVariableStringForPlayer`.

**These were armed, and the first in-game `sm_hud_class` segfaulted the server.** They are now
withdrawn from the gamedata; the commands refuse and explain why.

The signatures were *correct*. The ABI was not. `SetHasClassForPlayer` takes `CUtlString`
arguments, and the name-lookup helper it calls (`0x130ae00`) opens with:

```
mov rcx, QWORD PTR [rsi]    ; rsi is the panelId argument — dereferenced
test rcx,rcx
je   ...                    ; NULL is handled; garbage is not
```

It treats the argument as a `CUtlString*` — a `char**`. s2script's `"string"` arg kind marshals a
raw `char*`, so the engine read the first eight bytes of the ASCII text `"panel1"` as a pointer and
followed it. Non-NULL, so the helper's own null guard didn't save it.

This is **not fixable in gamedata**: the arg vocabulary is closed (`bool | int | float | string |
vector | entity`), none of it expresses "pointer to a CUtlString", and there's no 64-bit int kind to
smuggle an address through. An armed call that segfaults is strictly worse than an absent one.

Unblocking needs a **core** change — a `utlstring` arg kind that wraps the marshalled `char*` in a
temporary before the call, or a first-class HUD capability in the game package.

The addresses are kept in the gamedata as inert documentation, because they were expensive and they
are right:

| Call | Address | State |
|---|---|---|
| `SetHasClassForPlayer` | `0x1312b30` | verified, not callable |
| `SetDialogVariableStringForPlayer` | `0x1312120` | verified, not callable |
| `SetHasClass` (global) | — | not located |
| `SetDialogVariableString` (global) | — | not located |

Identity was established structurally: reached from the cs_script thunk at `0xb41cd0`, bounds-checks
its slot against `m_vecPlayerLayoutStates` (`cmp esi,[rdi+0x790]`), and indexes it as
`slot*0x1a0 + [rbx+0x798]` — which also confirms offset 1936 and, **on build 24916958**,
`sizeof(state) == 416`. Build **24957633 changed the stride to 408 (`0x198`)** — read straight out
of the engine's own `imul rbx,rbx,0x198` in the same setters — and moved `m_bInputCaptureEnabled`
from 48 to 52. `src/offsets.ts` (`STATE_SIZE`) and the gamedata's
`CCSCustomHudLayout_SetInputCaptureEnabledForPlayer` entry carry the current numbers; anywhere you
still see 416/`0x1a0` it is the pre-update constant.

## Per-slot state is storage, not delivery

`m_vecPlayerLayoutStates` is a `CUtlVectorEmbeddedNetworkVar<CCSCustomHudLayoutState>` — a
**networked** container of per-slot states on **one entity** (see `src/offsets.ts`). That shape
decides a property every consumer of the `*ForPlayer` calls needs to know: **"ForPlayer" is
per-slot STORAGE, not per-recipient DELIVERY.** The engine files your write under slot N's state;
it does not promise that only slot N's client ever sees it.

Before September 9, 2026, live testing showed spectators rendering the watched player's
panels. The updated engine adds the `observable` spawn keyvalue: false (the engine and
s2script default) keeps the viewer's own UI; true opts into the spectated player's UI.
`CustomHudLayout.create({ ..., observable: true })` opts in for a dedicated layout.
The shared hudkit stays non-observable so menus follow their owner. A plugin cannot
reuse one resource with conflicting observation policies.

This fixes the normal spectator presentation problem without a per-recipient entity
workaround. It does **not** establish per-recipient network confidentiality; authorize
clicks independently and never send secrets through HUD state. Transmit filtering is
still a separate mechanism with a separate live gate.

The update also fixes stale layout state on spectator target changes. Keep our
entity-replacement and connection-lifetime caches: those protect server-side state,
not the client spectator bugs. See [the update acceptance checklist](../../docs/CS2-UI-UPDATE.md).

## The HUD that works — `sm_hud_demo`

The early fallback `src/demohud.ts` renders on the surface CS2
already gives a server: the **centre-screen HTML panel**, via the `show_survival_respawn_status`
event. That's the same mechanism `MenuStyle.Center` already uses, so it's proven on this build, and
it's what SourceMod's `PrintToCenterHtml` does.

It shows team, name, a health bar, armour, active weapon, round number, round clock, and BUY /
BOMB / NOCLIP / FROZEN flags — repainted every tick.

```
sm_hud_demo          toggle it for yourself
sm_hud_demo on|off   explicit
sm_hud_demo_all      turn it on for everyone (root)
sm_hud_say <html>    one-shot centre-panel message, raw markup (root)
```

Two non-obvious constraints, both handled:

- CS2 paints `loc_token` for **one frame**, so the panel must be re-sent every tick or it vanishes.
- The client **filters the event on `userid`**, so every send carries the target's real `userId` —
  sending without it silently shows nothing.

Markup that works: `<font color='#rrggbb'>`, the size classes `fontSize-l/m/sm/s`, `<br>`, and HTML
entities. It's one flowing text block with no reserved regions, so layout is line-budgeted.

## Tier C — needs a core change

Button clicks arrive as `CS_UM_CustomHudClicked` (id 390), an **inbound** user message. No plugin can
observe it today:

* `UserMessages.onPre` intercepts **outbound** messages only.
* A declarative inbound hook cannot express the handler either — `HOOK_SHAPES` is a closed
  vocabulary (`this_void`, `this_f32_i32_i32_i32`, `this_f32_i32_i64_i64`, `this_i64_i32_i64`) and a
  message handler's `(this, const Msg&)` is a two-slot `this_i64`, which is not in it.

So a HUD built with this plugin can be *displayed* and *styled*, but cannot be *interacted with*.
Closing that gap is a core slice — ideally an inbound-usermessage capability rather than a raw
detour.

## The open question this harness answers

`m_strLayout` names a layout asset, and the patch notes call `custom_hud_layout` "the entry point
for **scripted maps** to provide custom UI". If that asset must be compiled into the map's VPK, a
server-side plugin cannot invent HUDs on a stock map at all.

`sm_hud_create` settles it in one command. The entity spawning proves nothing; the readout after it
does:

```
panelIds=0 classNames=0 dialogVars=0    -> no layout asset loaded (expected on a stock map)
panelIds>0                              -> a layout loaded and registered panels
```

## HUD offsets come from the live schema

`src/offsets.ts` resolves layout/state fields from the running engine. Missing fields stop
plugin initialization before any diagnostic write. The September 9 update moved the
player-state vector and global state; the live dump also confirms `m_playerSlot` is at
state +48, rather than the old borrowed +408. State stride is derived from the embedded
state's span and verified against the native setter arithmetic (408 bytes on build 2000908).

`probeLayout()` adds plausibility checks before raw writes, followed by readback.
`sm_hud_probe` remains a raw byte-window diagnostic. The native HUD setters use signatures
re-resolved through Valve's named script wrappers; see [migration notes](../../docs/CS2-UI-UPDATE.md).

## Commands

Everything is admin-gated. Writes are `ROOT`; reads are `GENERIC`.

```
sm_hud_status                              tier A/B/C report + live layout readout
sm_hud_probe [start] [len]                 raw byte window, to verify offsets against a dump
sm_hud_create [layout] [observable:0|1]    create + spawn a custom_hud_layout
sm_hud_list                                every custom_hud_layout in the world
sm_hud_remove                              remove the ones this plugin created
sm_hud_capture <0|1>                       SetInputCaptureEnabled (tier A, raw write)
sm_hud_class <panel> <class> <0|1|-1> [t]  SetHasClass / …ForPlayer (tier B)
sm_hud_var <panel> <var> <value> [target]  SetDialogVariableString / …ForPlayer (tier B)
sm_hud_visible <target> <0|1>              SetHUDVisibility entity input
sm_cam_create                              acquire and position the caller's camera
sm_cam_mode <0|1|2|3>                      select native camera mode
sm_cam_follow [distance]                   follow the caller with clipping
sm_cam_info                                show camera, owner and mode
sm_bomb_watch <0|1>                        log the four new plant/defuse callbacks
sm_bomb_info                               C4 field readout
sm_bomb_abort                              probe C4.AbortPlant as an entity input
sm_movetype [target] [type]                GetMoveType / SetMoveType
sm_hud_menu [chat|center]                  SourceMod menu feature test
sm_buymenu [chat|center]                   categorised buy menu
```

### What "queued" means

The legacy `AbortPlant` command reports `queued=true/false`. That is `acceptInput`'s return —
whether the I/O event was **queued**, not whether the target has that input. An unknown input name
is accepted, queued, and dropped later, silently. Those commands are probes: the input names are
guesses at Source convention, and only an observable in-game effect counts as evidence.

Camera commands now use verified native calls. The legacy `sm_cam_enable`,
`sm_cam_angles`, and `sm_cam_remove` aliases set modes; remove disables the pawn-owned
camera instead of deleting it. Use `sm_cam_mode 0` to return to the normal view.

## Build and install

```bash
node packages/sdk/dist/cli.js build examples/hud-lab
# -> examples/hud-lab/dist/_demo_hud-lab.s2sp
cp examples/hud-lab/dist/_demo_hud-lab.s2sp dist/addons/s2script/plugins/
```

Tier B additionally needs the operator allow-list — add `"@demo/hud-lab"` under `"engine:calls"` in
`addons/s2script/configs/permissions.json`. Without it the four calls report as unavailable for that
reason instead of the signature one, and `sm_hud_status` says so.

## Suggested first run

```
sm_hud_status          # what is reachable on this build
sm_hud_create          # does a layout asset load on a stock map?
sm_hud_probe           # inspect the live-schema field window
sm_hud_capture 1       # the one HUD write that works with no signature
sm_bomb_watch 1        # then plant/abort a bomb
sm_hud_menu center     # what menus look like today, for comparison
sm_buymenu center
```
