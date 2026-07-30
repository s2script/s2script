# library-consumer

A plugin that bundles `@example/base64` (`examples/library-package`) and actually calls it:
`sm_b64 <text>` / `sm_unb64 <text>` round-trip through the library's `encode`/`decode`. Build it
and the library's code is compiled straight into `plugin.js` — there is no `require("@example/
base64")` left for the host to resolve at load time; see the comment at the top of
`src/plugin.ts`.

## Which resolution path this uses, and why

`s2script.libraries` resolves a declared library from two places, tried in order (see
`packages/sdk/src/libraries.ts`): a vendored copy committed at `.s2script/libs/<name>/`, or —
inside an npm workspace — a sibling package that opts in with `s2script.kind: "library"`,
resolved straight from its own source with no copy anywhere.

This example commits the **vendored** copy, at `.s2script/libs/@example/base64/`. Two reasons:

- It is the path every real consumer of a *registry-published* library actually takes — `s2s add
  @example/base64` runs exactly this vendor step against a published `.s2lib`. `examples/
  library-package` and `examples/library-consumer` are siblings under `examples/`, but neither is
  a *workspace* — each is a standalone package, the same relationship `examples/greeter-plugin`
  has to `examples/greeter-consumer` (see that pair's READMEs for the parallel).
- It is what lets this example build in CI with no registry reachable. The vendored tree was
  produced once, by building the library and extracting its `.s2lib` (`lib-extract.ts`) — not
  hand-written — and committed so `s2s build examples/library-consumer` needs nothing but this
  checkout.

The workspace-sibling path is exercised elsewhere: `packages/sdk/test/libraries.test.mjs` and
`packages/sdk/test/workspace-build-all.test.mjs` cover `resolveLibrarySibling` directly, and
`examples/monorepo` already demonstrates the general "workspace members resolve from source, not
a copy" shape for a plugin's sibling *interface* (`plugins/producer`/`plugins/consumer`). Adding a
second, `s2script.libraries`-flavored demonstration of that same shape there was judged not to
teach anything this pair plus that test coverage doesn't already show, so it was left out of this
task.

## Regenerating the vendored copy

```bash
node --experimental-strip-types packages/sdk/src/cli.ts build examples/library-package
node --experimental-strip-types -e "
import('./packages/sdk/src/registry/lib-extract.ts').then(({extractLibArchive}) => {
  const fs = require('fs');
  extractLibArchive(
    fs.readFileSync('examples/library-package/dist/_example_base64.s2lib'),
    'examples/library-consumer/.s2script/libs/@example/base64',
    { name: '@example/base64', version: '0.1.0' },
  );
});
"
```
