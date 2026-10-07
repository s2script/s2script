---
"@s2script/sdk": minor
---

Bundles: a workspace root with `s2script.kind: "bundle"` publishes its plugins as one registry entry. `s2s deploy` publishes the members (each claiming the bundle) and then the bundle, pinning every member's exact version, and refuses an already-published bundle version whose pins differ. `s2s version` bumps the bundle by the largest member bump. `s2s install` takes `--with <member>` (and `{ "range", "with" }` manifest entries) to add a bundle's optional members, and `s2s add` refuses a bundle.
