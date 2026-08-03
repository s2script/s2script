---
"@s2script/cs2": minor
---

`ctx.gameRules.onTerminateRound`'s event view no longer carries `_unused3` / `_unused4`.

Those two were typed `readonly number` and were never readable — the underlying parameters are not
32-bit integers. The engine's third argument to `CCSGameRules::TerminateRound` is a **pointer**
(`mov %rdx,-0xe0(%rbp)`, a full 64-bit store), so declaring it `int` truncated whatever the engine
passed: the hook handed the original function half a pointer, and a later dereference segfaulted a
live server.

The descriptor now uses a shape that relays both trailing arguments at full register width as opaque
pass-through — they reach the original bit-for-bit and are deliberately not exposed to plugins, since
there is nothing a plugin could correctly do with them. `delay` and `reason` are unchanged and remain
writable.

Marked `minor` rather than `patch` because the two properties disappear from the emitted type. Any
plugin that referenced them was reading `undefined` behind a `number`, so nothing that worked stops
working — but it is a compile-visible change and should not arrive silently in a patch.
