# adminmenu (TopMenu framework) — Design

**Status:** Approved (brainstorm — core-registry TopMenu, item set scoped), ready for the plan.
**Slice:** the SourceMod `adminmenu`/TopMenu framework — an extensible, flag-gated admin menu (`sm_admin`) built on `@s2script/menu`, that command plugins register their actions into.

## Goal

Give server admins a navigable in-game menu (`sm_admin`) of admin actions, and give plugins an extensible way to add items to it — SourceMod's TopMenu model. An admin opens `sm_admin`, sees the categories/items their flags permit, picks one, and (for player actions) picks a target — driving the same primitives the text commands use.

## Motivation & context

`adminmenu` is the flagship consumer of the just-shipped menu primitive and a core piece of SourceMod parity (the base-plugin suite is the std lib's acceptance test). Its defining feature is **extensibility**: it is not a fixed menu but a **TopMenu** framework that any plugin registers items into (categories + items + per-item admin flags), so third-party plugins extend the admin menu without editing it.

## Scope

**In scope:** a core TopMenu registry (categories + items, owner-tracked, select-dispatch-to-owner); the `@s2script/topmenu` engine-generic module; the `adminmenu` CS2 plugin (`sm_admin`, flag-filtered rendering via `@s2script/menu`); a reusable `pickPlayer` helper in `@s2script/cs2`; and a representative item set registered across the existing command plugins as the live proof — **Player Commands: Kick, Ban (→ duration), Slap, Slay, Gag; Server Commands: Change Map (→ map picker)**.

**Deferred (named follow-ons):** the long tail of items (freeze/gravity/noclip/rename/mute/unmute/unsilence/unban/say/psay/cvar/exec/rcon/who) — mechanical additions; the **Voting** category (waits on `basevotes`); per-admin **immunity** checks on targets; SM's item-order weighting within a category (insertion order for the MVP); a generic (non-player) target picker beyond `pickPlayer`.

## Approach (decided)

**A core TopMenu registry**, not an inter-plugin interface. The boot loader loads plugins in **plain alphabetical order** (`loader.rs` `out.sort()`), with no dependency ordering — so an interface-based registry (a plugin registers into `adminmenu`'s context) is fragile: a consumer loading before `adminmenu` can't register (it works for the base suite only by the accident that `adminmenu` sorts before `base*`/`funcommands`/`playercommands`, and breaks for arbitrarily-named third-party plugins — defeating "extensible"). A **host-global core registry** (like `CONCOMMANDS`/cookies/admin) sidesteps load-order entirely: any plugin registers whenever it loads, and `adminmenu` reads the registry at `sm_admin` time (all plugins are up by then). It also matches the closest existing analog — a TopMenu item is "a command, but as a categorized, flag-gated menu entry," and commands are already a core owner-dispatched registry.

## Architecture

One-way deps (game → core). The TopMenu registry + `@s2script/topmenu` are engine-generic; `adminmenu` + `pickPlayer` are CS2.

### Core TopMenu registry (`core/src/v8host.rs`, engine-generic)

A host-global `thread_local`, mirroring `CONCOMMANDS`:
- **Categories:** an ordered list of names, deduped (adding an existing name is a no-op). Display order = insertion order.
- **Items:** `{ id, category, name, flags, owner, generation, onSelect }` where `onSelect` is a `v8::Global<v8::Function>` held exactly like a command handler — **not marshalled across contexts**; core invokes it in the owner's context on dispatch. `id` is a plugin-namespaced unique string (e.g. `"playercommands:slap"`); a duplicate `id` replaces (reload-safe).
- **Owner-scoped teardown:** on plugin unload, drop that plugin's items and any categories it added, alongside the existing `CONCOMMANDS` per-plugin cleanup; cleared on `shutdown`.

Natives (`set_native`, no new `S2EngineOps` op, no shim change — the cookies pattern):
- `__s2_topmenu_add_category(name)` — register a category (idempotent).
- `__s2_topmenu_add_item(category, id, name, flags, onSelectFn)` — register/replace an item owned by `current_plugin`; auto-creates `category` if absent.
- `__s2_topmenu_snapshot()` — return `{ categories: string[], items: [{id, category, name, flags}] }` (metadata only, no functions) for rendering.
- `__s2_topmenu_select(id, adminSlot)` — dispatch the select to the item's owner: `dispatch_topmenu_select` enters the owner's context and calls its `onSelect(adminSlot)`, a faithful `dispatch_concommand` mirror (owner-liveness check, `try_borrow_mut` re-entrancy guard, per-call `TryCatch`). A stale/removed `id` is a no-op.

### `@s2script/topmenu` (engine-generic module, core prelude)

- `TopMenu.addCategory(name)` → `__s2_topmenu_add_category`.
- `TopMenu.addItem(category, { id, name, flags, onSelect })` → `__s2_topmenu_add_item` (passes the `onSelect` function).
- `TopMenu.snapshot()` → `__s2_topmenu_snapshot()` (`{ categories, items }`).
- `TopMenu.select(id, slot)` → `__s2_topmenu_select`.

Types-only package `packages/topmenu/{package.json,index.d.ts}`.

### The `adminmenu` plugin (`plugins/adminmenu`, CS2)

- On load: `TopMenu.addCategory("Player Commands"); addCategory("Server Commands"); addCategory("Voting Commands");` (fixes the standard order).
- Registers `sm_admin` via `Commands.register` (anyone may type it), and in the handler **denies non-admins**: `const admin = Admin.forSlot(slot); if (!admin) { reply/chat "No access"; return; }`.
- Renders with `@s2script/menu` (Center style): reads `TopMenu.snapshot()`, keeps items where `admin.hasFlags(item.flags)`, hides categories with zero visible items → a category `Menu` → on select, an item `Menu` for that category → on select, `TopMenu.select(item.id, slot)` (which dispatches to the item's owner). The server console (`slot < 0`) cannot open a menu (no pawn/HUD) → reply telling them to run it in-game.

### `pickPlayer` helper (`@s2script/cs2`)

`pickPlayer(adminSlot, onPicked)` — a `@s2script/menu` (Center) over `Player.allConnected()`, each item labelled `name` (info = the userid) ; on select resolves the target via `Player.fromUserId` and calls `onPicked(targetPlayer)` (guarding a target that left in the interim → a "player no longer available" reply). Engine-generic-ish but uses `Player`, so it lives in the CS2 game layer.

### Item registration (the command plugins)

Each plugin, on load, registers its items with an `onSelect` that runs in its own context — target-pick then the existing primitive:
- `playercommands`: **Slap** (`SLAY`, → pick → `pawn.health -= 5`, min 1), **Slay** (`SLAY`, → pick → `pawn.slay()`, the existing `sm_slay` primitive).
- `basebans`: **Kick** (`KICK`, → pick → `player.kick()`), **Ban** (`BAN`, → pick → a small duration `Menu` [0/5/30/60 min] → `Bans.add(steamId, minutes, reason)` + kick).
- `basecomm`: **Gag** (`CHAT`, → pick → the existing gag path, SteamID-keyed).
- `basecommands`: **Change Map** (`CHANGEMAP`, → a map `Menu` [a short curated list or `Server.isMapValid`-checked entries] → `Server.command("changelevel …")`).

Each item's `flags` is the same `ADMFLAG` as the corresponding text command, so the flag filter matches the command's gating.

## Testing & live gate

- **Core unit tests** (`v8host.rs` `frame_tests`, in-isolate — the `CONCOMMANDS`/cookies pattern): add category + item then `snapshot` returns them; a duplicate `id` replaces; `select` dispatches to the owner's `onSelect` with the slot (assert via a captured value); a second plugin's items don't leak into the first; owner-scoped removal on unload drops that plugin's items + `snapshot` reflects it; `select` on a stale id is a no-op.
- **Live gate (human client — the menu ceiling)**: `sm_admin` opens; the category + item menus render **flag-filtered** (a full admin sees all, a KICK-only admin sees only Kick, a non-admin is denied); **Slap → picker → pick a bot → its hp drops**; **Kick → picker → the bot disconnects**; **Ban → picker → duration → the bot is kicked + `bans.json` gains the entry**; **Change Map → map picker → the map changes**. RestartCount=0, no crash.
- **Gates**: core-boundary (the TopMenu registry + `@s2script/topmenu` carry no game identifiers), name-leak, typecheck, full `cargo test`. One sniper rebuild (core registry + natives).

## Boundary & safety summary

The TopMenu registry and `@s2script/topmenu` are engine-generic (categories/items/ids/flags are plain data; the owner-dispatch mirrors `CONCOMMANDS`). `adminmenu`, `pickPlayer`, and the per-item handlers (which name CS2 primitives) are CS2/game-layer. `onSelect` functions never cross contexts (held as `v8::Global`, invoked in-owner-context like command handlers). Items are owner-scoped and torn down on unload. Both boundary gates stay green.
