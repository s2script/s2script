# `@s2script/sdk` root barrel — implementation plan

> GitHub-native stacked PR. Base: `cursor/sm-lifecycle-publics-a8c9`. Branch: `cursor/sdk-barrel-a8c9`.

**Goal:** Bare `@s2script/sdk` typechecks and `__s2require("@s2script/sdk")` returns the authoring object. `Player` stays `@s2script/cs2`.

## Files

- Create: `packages/sdk/index.d.ts` (`export *` from engine-generic caps; skip globals/unsafe/console)
- Modify: `packages/sdk/package.json` (`exports["."]`)
- Modify: `packages/sdk/src/typecheck/typecheck.ts` (exact path + `isAlwaysResolved`)
- Modify: `tsconfig.base.json` (exact `"@s2script/sdk"` path)
- Modify: `core/js/prelude.js` (`__s2pkg_sdk` after command/hook bind)
- Modify: `core/src/v8host.rs` (`s2require` comments + isolate tests). Do **not** rustfmt the whole file.
- Modify: `scripts/check-examples-coverage.sh` (exclude `index`)
- Modify: `packages/sdk/src/create/create.ts` (scaffold uses the barrel)
- Create: `packages/sdk/test/fixtures/authoring-barrel/`
- Modify: `packages/sdk/test/build.test.mjs`, `tsconfig-base-parity.test.mjs`
- Modify: `packages/eslint-plugin/src/plugin-factory.ts` (accept barrel `plugin` import)
- Create: `.changeset/sdk-barrel.md` (`@s2script/sdk` minor, `@s2script/eslint-plugin` patch)

## Verify

- `cargo +stable test -p s2script-core s2require_dual`
- `cargo +stable test -p s2script-core sdk_barrel`
- `bash scripts/check-core-js-lint.sh`
- `bash scripts/check-examples-coverage.sh`
- `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/build.test.mjs test/create-resolve.test.mjs test/tsconfig-base-parity.test.mjs test/typecheck.test.mjs test/package-files-complete.test.mjs`
- `cd packages/eslint-plugin && npm test`
