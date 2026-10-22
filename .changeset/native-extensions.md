---
"@s2script/sdk": minor
---

Native extensions: the SDK ships the extension ABI header (`ext/s2ext.h`), and `s2s build` accepts `s2script.extensions` (`name → range`), keeps those imports external, writes them into the manifest, and typechecks them against `.s2script/extensions/<name>/index.d.ts` or the extension's npm package.
