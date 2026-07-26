---
'@s2script/cs2': patch
---

Ship `weapon.d.ts`. `Weapon` was resolving to `any` for every consumer.

`index.d.ts` has always done `export { Weapon } from "./weapon"`, but `weapon.d.ts` was missing from
the package's `files` array, so it never reached the tarball. A dangling relative specifier does not
error — TypeScript resolves it to `any` — so `pawn.activeWeapon` and everything reached through it
silently lost its type and code that should not have compiled compiled fine. A downstream plugin hit
this and hand-wrote the interface plus a runtime property probe to work around it.

Adds a test that walks every `.d.ts` in each publishable package's REAL tarball (via
`npm pack --dry-run --json`, so it cannot disagree with npm's own packing rules) and asserts each
relative re-export target ships too, plus a companion check that every declared `exports` subpath
points at a shipped file. Verified against the bug: reverting the one-line fix fails the test with the
offending specifier named.
