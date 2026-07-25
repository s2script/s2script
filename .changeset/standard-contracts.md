---
'@s2script/sdk': minor
'@s2script/cs2': minor
---

Publish standard interface contracts for econ/skins and workshop — types only, no implementation.

New subpaths: `@s2script/sdk/contracts/workshop` (`WorkshopService`, engine-generic) and
`@s2script/cs2/econ` (`EconService`, `WeaponSkin`, `Loadout`, CS2-specific). A community plugin
implements one and publishes it; consumers depend on the agreed shape via `ctx.tryUse` instead of a
different ad-hoc interface per plugin.

The framework ships no implementation deliberately: applying skins means driving CS2's economy item
model and workshop means Steam's UGC services, neither of which is a Source 2 engine touchpoint.

Every 64-bit value in these contracts — workshop published-file IDs, SteamIDs — is typed as a
decimal **string**, because a `BigInt` throws crossing the plugin boundary and silently drops the
whole payload. Everything Steam-facing is `Promise`-returning so an implementation cannot block the
game frame.

Also fixes game-package subpath resolution in the plugin typecheck: `@s2script/*` mapped to
`<pkg>/index.d.ts`, which cannot express a subpath, so `@s2script/cs2/econ` was `TS2307`.
