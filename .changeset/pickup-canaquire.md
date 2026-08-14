---
"@s2script/cs2": minor
"@s2script/sdk": patch
---

Add `ctx.items.onCanAcquire` / `onCanAcquirePost` — a first-class pickup gate over `CCSPlayer_ItemServices::CanAcquire`. Plugins can refuse a pickup (or a `giveNamedItem`) with `AcquireResult` + `HookResult`. The item view is block-scoped scalars, never a pointer.
