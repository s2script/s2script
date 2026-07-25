---
'@s2script/sdk': minor
---

Add cancellable, repeating timers — SourceMod `CreateTimer` / `KillTimer`.

`after(ms, fn)` and `every(ms, fn)` return a `Timer` handle with `kill()` and `alive`. The existing
`delay`/`nextTick`/`nextFrame` are Promises and cannot be cancelled: "cancelling" one means leaving
it forever unresolved, which leaks the continuation.

Every timer is ledgered against the creating plugin, so unload kills it whether or not `kill()` was
called — a repeating timer can never outlive its plugin and fire into a dead context. A throwing
callback is contained and a repeater keeps repeating; `kill()` is idempotent and safe from inside
the callback itself; `every(0, …)` throws rather than starving the frame.
