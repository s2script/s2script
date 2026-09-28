---
"@s2script/sdk": minor
---

Retire the `this_i64_i32_i64` and `this_i64_i64_i64` declarative hook shapes. They existed only for
the CS2 pickup gate and custom-HUD click, which now run as trusted engine functions of the CS2
package. The gamedata validator rejects them by name, and the runtime removes the matching thunks
and the POST path they alone used. Plugin gamedata that declares a hook with either shape degrades
to a named unknown-shape error. Requires the matching runtime.
