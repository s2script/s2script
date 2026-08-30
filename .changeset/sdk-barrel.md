---
"@s2script/sdk": minor
"@s2script/eslint-plugin": patch
---

Root import `from "@s2script/sdk"` is a valid authoring barrel (engine-generic names only; `Player` stays on `@s2script/cs2`). `plugin()` imported from the barrel is still visible to `no-ctx-escape`.
