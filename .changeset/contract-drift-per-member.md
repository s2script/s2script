---
"@s2script/sdk": patch
---

Docs: a consumer built against an older copy of a producer's types now loads. The host warns about
the drift and fails only the members that were removed or retyped, when they're used.
