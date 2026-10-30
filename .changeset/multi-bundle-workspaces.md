---
"@s2script/sdk": minor
---

`s2script.workspace.bundles` lets one workspace hold several bundles. Each listed directory keeps a
package.json with `s2script.kind: "bundle"` naming its bundle, and its members are the non-private
plugins under it. `s2s version` bumps each bundle by its own members' largest bump, and `s2s deploy`
publishes each bundle after its members, so several bundled plugin monorepos can share one
`.changeset/` and one release while staying separate registry bundles.
