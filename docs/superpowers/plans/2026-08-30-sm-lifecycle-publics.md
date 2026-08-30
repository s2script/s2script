# SM lifecycle publics + hook.event — implementation plan

> GitHub-native stacked PR. Base: `cursor/basecommands-dogfood-a8c9`. Branch: `cursor/sm-lifecycle-publics-a8c9`.

**Goal:** Host-subscribe the remaining SM-named publics; expand `hook`; ship `createScope` + `Command` alias so remaining plugins can drop `ctx`.

## Files

- Modify: `core/js/prelude.js` (`__s2_run_factory`, `hook`, `createScope`, `__s2_fire_all_plugins_loaded`)
- Modify: `core/src/v8host.rs` (`finalize_loading_plugins` quiet-fire; isolate tests). Do **not** rustfmt the whole file.
- Modify: `core/src/loader.rs` (`has_waiting`)
- Modify: `packages/sdk/plugin.d.ts`, `commands.d.ts`, `config.d.ts`
- Modify: `packages/sdk/test/fixtures/authoring-publics/src/plugin.ts`
- Create: `.changeset/sm-lifecycle-publics.md` (`@s2script/sdk` minor)

## Verify

- `cargo +stable test -p s2script-core plugin_start`
- `cargo +stable test -p s2script-core sm_publics`
- `bash scripts/check-core-js-lint.sh`
- `bash scripts/check-plugins-typecheck.sh`
- `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/build.test.mjs`
