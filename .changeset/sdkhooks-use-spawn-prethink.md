---
"@s2script/sdk": patch
---

`SDKHook` now works for `Use`, `Spawn`, `PreThink` and `GetMaxHealth` (and their `Post` twins) on
CS2, and `PostThink` works again on the Oct 22 2026 build. The `Think` doc now says Source 2 has no
Think virtual, so `SDKHook` returns `false` for it.
