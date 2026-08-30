---
"@s2script/sdk": patch
---

Raise plugin typecheck and esbuild target to ES2024 so authors can use `Object.groupBy` and other ES2024 lib APIs. CLI compile (`packages/sdk/tsconfig.json`) stays ES2020.
