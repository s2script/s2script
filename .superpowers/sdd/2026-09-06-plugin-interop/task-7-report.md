# Task 7 report: explicit named-handler bindings

## Revisions

- Base: `1b71ec228e57d0f4289951a18c10609a42238bec`
- Gated implementation head: `e194de60e02e18d8baa936f5ac49cd1af9ebf270`
- Branch: `codex/interop-07-named-bindings`

The final branch head is a report-only successor of the gated implementation commit above.

## Scope delivered

- Added provider-qualified `bindForwards(name, handlers): Subscription` to both the load context and free plugin API.
- Reused the protocol 2 `on` native, provider identity, subscription IDs, and normal load-window buffering.
- Added exact mapped handler types. They preserve provider-specific payloads and synchronous Hook/Transform responses, reject unknown forward keys, reject async decision handlers, and retain exact union-patch key checks.
- Made a binding map transactional: a partial arm failure disposes every exact ID created by that call before rethrowing. Whole-map disposal is idempotent, pre-arm cancellation creates no native registrations, and callback references are released after arm, cancellation, or failure.
- Restricted binding to hard `pluginDependencies`. Optional integrations continue through `watchOptional` and the scoped `service.on`, including named local handlers; no optional-present shortcut or wider attachment-token window was added.
- Added two-provider fixtures with the same forward name and different payloads, compiler negatives for swapped handlers, unknown keys, direct/aliased optional declarations, async decisions, and union transform patches, plus runtime ownership/rollback/GC tests.
- Updated the SDK README, interop guide, and changeset. There is no export discovery, global callback registry, or native ownership API change.

## Red/green evidence

### SDK/compiler

Command:

```text
cd packages/sdk && node --test test/interop.test.mjs
```

- Initial red: `84 passed; 5 failed` — all new binding cases failed because `bindForwards` was not exported.
- Policy red after adding the typed/runtime API: `90 passed; 2 failed` — direct and aliased optional-dependency binding was still accepted.
- Green after the hard-dependency compiler guard: `92 passed; 0 failed`.

Full SDK command:

```text
cd packages/sdk && npm test
```

Result: `719 passed; 0 failed`.

### Linux runtime

Command:

```text
/private/tmp/s2script-interop-run-native.sh /private/tmp/s2script-interop-01 cargo test -p s2script-core named_forward_bindings
```

- Initial red: `0 passed; 2 failed` — the context API was absent and the rollback marker stayed at zero.
- Green: `2 passed; 0 failed; 894 filtered out`.
- Green again after the ownership self-review refactor: `2 passed; 0 failed; 894 filtered out`.

The runtime tests prove provider isolation for identical forward names, repeated whole-map disposal, immediate rollback of a real partial native registration, pre-arm cancellation, empty native subscription state, post-arm/pre-arm callback collection, and late-call rejection.

## Final gates

JavaScript command:

```text
PATH=/Applications/Docker.app/Contents/Resources/bin:$PATH bash scripts/ci-js.sh
```

Result: exit `0`; `ci-js: all JS gates passed`. The SDK suite reported `719 passed; 0 failed`, and the Docker test gate passed. This prescribed local invocation does not set `CI=1`, so it does not run the clean `npm ci` lockfile guard.

Linux native command:

```text
/private/tmp/s2script-interop-run-native.sh /private/tmp/s2script-interop-01 bash scripts/ci-native.sh
```

Result: exit `0`; core reported `893 passed; 0 failed; 3 ignored`, the extra async stress tests passed, the shim built, and the final line was `ci-native: all native gates passed`.

Repository check:

```text
git diff --check
```

Result: exit `0`, no output.

## Review notes and limits

- Registration uses captured native `on`/dispose functions and stores exact IDs. Rollback attempts every created ID and preserves the original registration error.
- Handler maps are copied from their own enumerable string keys only. Module exports are never scanned.
- No live CS2 gate was required because this slice changes the prelude, declarations, compiler checks, and isolated V8 runtime tests without changing `core` or shim ownership APIs.
- The Linux image does not install `cargo-fmt`; the macOS formatter version wants to reformat unrelated baseline Rust files. Formatting was therefore assessed with the repository's native gate and `git diff --check` rather than applying a cross-version whole-tree rewrite.
