# @s2script/eslint-plugin

## 0.2.0

### Minor Changes

- cb50b95: B1 (build ⊇ load): `s2s build` now DERIVES the manifest — `apiVersion` is stamped from the SDK's
  host-major constant (authored values ignored with a warning), the `publishes` name-set is derived
  from `ctx.publish` calls (drift is a build error; `"self"` auto-derives), dependency-usage
  advisories warn on declared-vs-used mismatches, and a `.s2script/types/<iface>/index.d.ts`
  verified contract copy gives a consumer REAL dependency types plus a `compiledAgainst` hash that
  the host verifies at load (contract drift now fails fast at load AND per-call).

  B2: new `@s2script/eslint-plugin` — `no-ctx-escape`, `no-floating-promise-in-factory`,
  `no-bigint-in-interface-payloads`, `no-await-in-raw-view` — pinned by the SDK, scaffolded by
  `s2s create` (`eslint.config.mjs`), and executed in-process by `s2s build` after the tsc gate
  against the gate's own `ts.Program`. Lint errors refuse the `.s2sp`.
