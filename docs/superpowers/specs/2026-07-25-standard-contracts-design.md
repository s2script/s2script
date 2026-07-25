# Standard interface contracts (econ / workshop) — design

**Status:** Implemented (2026-07-25).
**Audience:** plugin authors implementing or consuming a community capability; maintainers.

---

## 1. Goal

ModSharp exposes econ/inventory and Steam/workshop APIs. s2script should let plugins *interoperate*
on those, without the framework implementing either.

## 2. Why contracts and not features

Applying weapon skins means driving CS2's economy item model; workshop means Steam's UGC services.
Neither is a Source 2 engine touchpoint, and implementing them would pull the core toward one game's
economy and Valve's backend — against *the core is engine-generic* and *dependencies point one way:
game → core*.

Publishing the **contract** gets the ecosystem benefit (consumers depend on one agreed shape instead
of a different ad-hoc interface per plugin) with none of that surface. This is a deliberate,
recorded scope decision, not an omission.

## 3. Where they live, and why not as SDK capability modules

| Contract | Home | Why |
|---|---|---|
| `WorkshopService` | `@s2script/sdk/contracts/workshop` | engine-generic — every Source 2 title has UGC |
| `EconService`, `WeaponSkin`, `Loadout` | `@s2script/cs2/econ` | CS2-specific game content |

Deliberately **not** `packages/sdk/<cap>.d.ts`. That namespace is shipped *capability modules*, and
`check-examples-coverage` requires each to have a consumer — which for a contract would mean the
framework shipping the very implementation this slice exists to avoid. The gate globs at
`-maxdepth 1`, so `contracts/` sits outside it by construction rather than by exemption.

## 4. Contract rules (they cross the plugin boundary)

- Arguments and results cross by **structured copy as JSON**. A `BigInt` throws and silently drops
  the whole payload, so every 64-bit value — workshop published-file IDs, SteamIDs — is typed as a
  **decimal string**. This is the single most common way a contract like this goes wrong.
- Everything Steam-facing in `WorkshopService` is `Promise`-returning: an implementation must not
  block the game frame.
- Methods degrade (`null`/`false`) rather than throwing, matching the framework's posture.
- Consumers should use `ctx.tryUse` (optional) rather than `ctx.use` (hard) — these are third-party
  by definition, so `null` is the normal state on a stock server.

`EconService` stays deliberately thin because the framework already supplies the primitives an
implementer needs: the generated schema exposes `fallbackPaintKit`/`Seed`/`Wear`/`StatTrak` on the
weapon entity, and `@s2script/entity` gives serial-gated refs and item enumeration. What is missing
is agreement on shape, which is exactly what a contract is.

## 5. Making sure they compile

A contract nothing imports rots silently. The cookbook's `contracts` recipe is the **consumer** side
— `ctx.tryUse` for both, reporting "not installed" when absent — so both contracts are typechecked
by `check-plugins-typecheck` on every CI run, and the intended usage is demonstrated without
shipping an implementation.

That surfaced a real gap: the typecheck harness mapped `"@s2script/*" -> "*/index.d.ts"`, which
**cannot express a subpath**, so `@s2script/cs2/econ` was `TS2307`. Fixed with an explicit
longer-prefix `"@s2script/cs2/*" -> "cs2/*.d.ts"` entry, plus `exports` maps on both packages.

## 6. Testing

- `build resolves game-package and nested SDK contract subpaths` — a fixture plugin importing both
  subpath forms builds. **Mutation-verified**: deleting the `@s2script/cs2/*` mapping makes it fail.
- `check-plugins-typecheck` compiles both contracts through the cookbook consumer.
- No live gate: this slice ships no runtime. Deliberate — there is nothing on a server to observe.

## 7. Out of scope

Any implementation of either contract; per-item inventory queries beyond a loadout; and workshop
collection management. All of that belongs in a community plugin, which is the point.
