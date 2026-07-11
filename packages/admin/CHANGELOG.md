# @s2script/admin

## 0.2.0

### Minor Changes

- 0da49f2: Admin groups, immunity levels, and command overrides (SourceMod `admin_groups.cfg` parity, JSON-shaped).

  - New config files: `admin_groups.json` (named groups = flags + immunity + optional per-group command overrides) and `admin_overrides.json` (global command → required-flag remaps). `admins.json` is enriched to an object form (`{ groups, flags, immunity }`); the legacy flag-array form still works. Flag tokens accept names, SM single letters, or compact letter-strings.
  - `@s2script/admin`: `AdminInfo` gains `immunity` and `groups`; `Admin` gains `canTarget(callerSlot, targetSlot)` and `getGroup(name)`; `Admin.add` takes an optional `immunity`.
  - `@s2script/cs2`: `Player.target` gains an optional `filterImmunity` argument. The destructive base commands (kick / slap / slay / ban / gag / mute / gravity / noclip / freeze / blind / votekick) now refuse targets with higher immunity than the calling admin.

### Patch Changes

- 5fcc41f: Initial public npm release of the `@s2script/*` types packages and CLI (Changesets pipeline).
