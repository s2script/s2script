---
"@s2script/sdk": patch
---

`s2s create` resolves non-`sdk` dependency versions live from the registry

The scaffolder pinned `@s2script/cs2` to the CLI's own (`@s2script/sdk`) version, which
is wrong once the two packages diverge — it emitted an unsatisfiable `^0.1.0` for a
`0.5.0` package and `npm install` failed. `@s2script/sdk` still pins to the CLI version
(the CLI *is* that artifact); every other package is now resolved from the registry at
scaffold time (`npm view`, respecting `.npmrc`), degrading to `latest` only when the
registry is unreachable, npm is absent, or the package is unpublished. The in-monorepo
`file:` path is unchanged.
