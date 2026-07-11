# Admin groups, immunity & overrides — design

**Date:** 2026-07-11
**Status:** approved (design)
**Slice:** admin-groups (SourceMod `admin_groups.cfg` parity, JSON-shaped)

## Goal

Bring SourceMod's group/immunity/override admin model to s2script: named **groups** (a reusable
flags + immunity bundle admins can join), **immunity levels** (a lower-immunity admin cannot
kick/slap/ban a higher-immunity one), and command **overrides** (change the flag a command
requires, globally or per-group). Faithful to *what SM does*, expressed in JSON rather than SM's
KeyValues `.cfg` — no KeyValues parser, no `STEAM_2/3`↔SteamID64 conversion (we stay SteamID64
everywhere, consistent with `admins.json`/`bans.json`/configs).

## Background — the current admin system (Slice 6.2)

- `admins.json` = `{ "<steamid64>": ["kick","ban"] }` — a flat SteamID64 → flag-name array.
- A host-global two-tier cache in **core** (`ADMIN_FILE` ⊕ `ADMIN_RUNTIME`), each `SteamID64 → u64 mask`.
  It must be core because V8 plugin contexts are isolated: a runtime `Admin.add` in plugin A has to
  be visible to plugin B's gating.
- `@s2script/admin` (engine-generic module, in the core prelude): `ADMFLAG` (SM bit values),
  `Admin.add/remove/get/forSlot/reload`, `AdminInfo.hasFlags(req)` (ROOT ⇒ all, else `(flags&req)===req`),
  and the `admins.json` parser/auto-generate. It installs `globalThis.__s2_admin_check(slot, mask)`,
  which `@s2script/commands`' `registerAdmin` consults to gate a command.
- **No groups, no immunity, no command overrides.** Every "admin immunity check" is currently deferred
  across the base plugins (`Player.target` returns all matches unfiltered).

The `ADMFLAG` bit order already matches SM's flag letters 1:1 — `a`=RESERVATION(1<<0), `b`=GENERIC,
`c`=KICK, `d`=BAN, `e`=UNBAN, `f`=SLAY, `g`=CHANGEMAP, `h`=CONVARS, `i`=CONFIG, `j`=CHAT, `k`=VOTE,
`l`=PASSWORD, `m`=RCON, `n`=CHEATS, `z`=ROOT(1<<14) — so single-letter flags need no remapping.

## Scope decisions (locked)

- **Format:** JSON (not `.cfg`). SM semantics, not SM's file format.
- **Immunity:** full — parse/store + enforce targeting in the base commands.
- **Overrides:** included — global (`admin_overrides.json`) *and* per-group (`overrides` block).
- **Deferred:** immunity *groups* (group-A-immune-to-group-B), `sm_immunity_mode` variants, and a
  KeyValues/`STEAM_`-format import path. v1 is the single "can't punch up in immunity level" rule.

## A. Files

All optional, all read through the existing config-file bridge (`__s2_config_read_raw("<name>")` →
`addons/s2script/configs/<name>.json`; `__s2_config_write_raw` to auto-generate). Each auto-generates a
**valid-JSON self-documenting template** when absent (a `_help` string key, the pattern `admins.json`
already uses — a `//`-commented template would fail the next-restart `JSON.parse`). No new engine op.

### `admin_groups.json`

Named groups, each a reusable flags + immunity bundle with an optional per-group override block.

```json
{
  "Full Admins": { "flags": "bcdefgjk", "immunity": 50, "overrides": { "sm_kick": "" } },
  "Root":        { "flags": ["root"],   "immunity": 100 }
}
```

- `flags`: a compact SM letter-string (`"bcdefgjk"`) **or** an array of tokens, where each token is a
  flag name (`"kick"`) **or** a single SM letter (`"c"`).
- `immunity`: integer, default `0`.
- `overrides` (optional): `{ "<command>": "<flag-token>|\"\"" }` — for members of this group, running
  `<command>` requires the override flag instead of the command's default; `""` = the group is always
  allowed that command.

### `admin_overrides.json`

Global command → required-flag remaps (apply to everyone). `""` = anyone may run it.

```json
{ "sm_slap": "generic", "sm_who": "" }
```

Value is a single flag token (name or letter) or `""`.

### `admins.json` (enriched; legacy form preserved)

```json
{
  "76561198000000001": { "groups": ["Full Admins"], "immunity": 80 },
  "76561198000000002": { "flags": ["kick","ban"], "immunity": 20 },
  "76561198000000003": ["kick","ban"]
}
```

- An entry may be the **legacy array** (`["kick","ban"]` = flags only, immunity 0, no groups — unchanged
  from today) **or an object** `{ groups?, flags?, immunity? }`.
- Effective flags = own `flags` ∪ (union of every referenced group's flags).
- Effective immunity = `max(own immunity, each referenced group's immunity)`.
- An unknown group name → skip that reference + WARN (degrade, never crash). An unknown flag token →
  skip + WARN (today's behavior).

## B. Model resolution + core

**Groups are a pure load-time / JS concept — they never enter the core cache.** The one-shot loader
(runs in the first plugin context that imports `@s2script/admin`, exactly as today) resolves each admin
to a flat effective record and pushes the *results* into the shared core cache.

### Core cache

The per-SteamID cache value grows from `mask` to `{ mask, immunity, overrides }` where `overrides` is a
per-admin map `command → (mask | PUBLIC)` (the merge of the admin's groups' override blocks). A separate
**global** overrides map (`command → (mask | PUBLIC)`) holds `admin_overrides.json`. Everything the
isolated per-context check needs is in core (the existing two-context-isolation rationale).

Precedence at resolve time, per admin: a per-admin (group) override for a command beats the global
override, which beats the command's registered default.

### Natives (extend / add)

- Extend `__s2_admin_set(sid, mask, immunity, isRuntime)` (was `(sid, mask, isRuntime)`).
- `__s2_admin_get_immunity(sid) -> int` (0 if absent).
- `__s2_admin_add_override(sid, cmd, mask, isPublic)` — a per-admin override entry.
- `__s2_admin_set_global_override(cmd, mask, isPublic)` — a global override entry.
- `__s2_admin_override(sid, cmd) -> string` — `""` (none), `"public"`, or a decimal mask. Per-admin
  entry wins over global. Called only on command dispatch (not a hot path), so a small string return is
  fine and avoids a struct ABI.
- `__s2_admin_clear_file()` already clears the file tier on `reload()`; it must now also clear the
  file-tier overrides + global overrides (runtime-tier overrides, if any, persist like runtime admins).

All natives `catch_unwind` + degrade (unknown sid/cmd → zero-value), matching the existing admin natives.

### `@s2script/admin` API additions

- `AdminInfo.immunity: number` and `AdminInfo.groups: string[]`.
- `Admin.canTarget(callerSlot, targetSlot): boolean`:
  - `callerSlot < 0` (server console / rcon) → `true` (infinite immunity).
  - target not an admin (or immunity 0) → `true`.
  - otherwise `true` iff `caller.immunity >= target.immunity` (SM default — **blocked iff
    `target.immunity > caller.immunity`**; equal immunity can target).
- `Admin.getGroup(name): { name, flags, immunity, overrides } | null` (reads the JS group registry).
- `Admin.add(steamId, flags, immunity?)` — optional third arg (default 0).
- The module installs `globalThis.__s2_admin_can_target(callerSlot, targetSlot)` (mirroring the existing
  `__s2_admin_check` hook) so the CS2 layer can immunity-filter without importing the module.

Group definitions live in a JS registry inside the module (populated at load, consulted by `getGroup`
and for resolution). `Admin.reload()` clears + re-resolves everything (file tier, overrides, groups).

## C. Enforcement

### Overrides — thread the command name through the gate

`@s2script/commands` `registerAdmin(name, flags, handler)` wraps the command; today its check is
`check(callerSlot, flags)`. Change to `check(callerSlot, flags, name)`. The `__s2_admin_check` hook
becomes:

```
function __s2_admin_check(slot, requiredMask, cmdName) {
  var a = Admin.forSlot(slot); if (!a) return false;
  if (cmdName) {
    var ov = __s2_admin_override(a.steamId, cmdName);   // per-admin (group) beats global
    if (ov === "public") return true;
    if (ov !== "") return a.hasFlags(parseInt(ov, 10));
  }
  return a.hasFlags(requiredMask);
}
```

Backward-compatible: a 2-arg caller → `cmdName` undefined → no override lookup → today's behavior. (The
`callerSlot < 0` = root short-circuit stays in `registerAdmin`'s wrapper, ahead of the hook, unchanged.)

### Immunity — filter targets in the CS2 layer

`Player.target(pattern, callerSlot)` (in `games/cs2/js/pawn.js`) gains a third param:

```
Player.target(pattern, callerSlot, filterImmunity = false)
```

When `filterImmunity` is true, drop any resolved target `t` where
`globalThis.__s2_admin_can_target(callerSlot, t.slot)` is `false`. If the admin module isn't loaded
(`__s2_admin_can_target` undefined) → no filtering (degrade — everyone targetable). Default `false`
keeps non-destructive callers (and existing call sites) unchanged.

Every **destructive** base command passes `true`:

| Plugin          | Commands |
| --------------- | -------- |
| basecommands    | `sm_kick` |
| playercommands  | `sm_slap`, `sm_slay`, `sm_rename` |
| basebans        | `sm_ban` |
| basecomm        | `sm_gag`, `sm_mute`, `sm_silence` |
| funcommands     | `sm_gravity`, `sm_noclip`, `sm_freeze`, `sm_blind` |
| votes           | `sm_votekick`, `sm_voteslay` (funvotes/basevotes) |

Non-destructive lookups (`sm_who`) stay unfiltered — SM only immunity-filters commands that act.

An immune single target filters to an empty set → the command's existing "No matching players" reply.
Precise "that player is immune" messaging is a noted nice-to-have, not a v1 requirement.

## D. Error handling & degradation

- Malformed `admin_groups.json` / `admin_overrides.json` / `admins.json` → WARN + ignore that file
  (degrade-never-crash; the files may be hand-edited), matching the existing `admins.json`/`bans.json`
  parsers.
- Unknown group reference / unknown flag token → skip that item + WARN, keep the rest.
- Missing file → auto-generate a valid-JSON `_help` template, treat as empty.
- Admin module absent → no immunity filtering, no override remaps (commands gate on their default flag);
  never a crash.
- Bots / unauthenticated clients read SteamID `"0"` → never an admin → immunity 0 (the existing
  `forSlot` `"0"` guard is retained).

## E. Testing

**In-isolate (core `node:test` + the module) — where the logic is proven exhaustively:**

- Flag-token parser: names, single letters, compact letter-strings, unknown-token skip.
- Group resolution: effective mask = own ∪ groups; effective immunity = max(own, groups); unknown-group
  skip; legacy-array entry still resolves.
- Override precedence: per-admin (group) > global > default; `""` and `"public"` handling.
- `canTarget` truth table: console infinite; non-admin target; equal-immunity can target; punch-up
  blocked; admin-module-absent degrade.

**Live gate (de_dust2, bots, rcon):**

- First boot auto-generates the three templates as valid JSON.
- A seeded `admin_groups.json` + `admins.json`: log each resolved admin's mask/immunity/groups.
- A global `admin_overrides.json` entry visibly changes a command's required flag.
- Console/root still runs every command; `RestartCount=0`; no crash.

**Deferred human-client test:** true admin-vs-admin immunity blocking in-game needs two real admins with
SteamIDs (bots read SteamID `"0"` → never admins → immunity 0) — same ceiling as the other human-client
deferrals. The logic is fully covered in-isolate; the live gate proves loading + overrides + no-crash.

## Boundary check

- Core cache + natives are engine-generic (SteamID64 → record; no CS2 symbol). ✓
- `@s2script/admin` is engine-generic (prelude module). ✓
- `player.steamId` and `Player.target` immunity filtering live in the CS2 layer (`pawn.js`). ✓
- Base-command wiring is in the CS2 base plugins. ✓
- No shim change; the natives are `set_native`'d in core → one sniper rebuild (core `.so`) for the
  extended/added natives + prelude.

## Out of scope (do not build ahead)

- Immunity groups (group-A-immune-to-group-B) and `sm_immunity_mode` variants.
- A KeyValues / `admins_simple.ini` / `STEAM_2/3` import path (JSON only).
- SQL admin sources; `admins.cfg` (KeyValues) rich admin blocks; per-admin passwords.
- Precise "target is immune" per-command messaging (empty-result reply suffices for v1).
- Command *groups* in overrides (SM's `@css` category tokens) — only literal command names in v1.
