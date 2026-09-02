---
"@s2script/cs2": patch
---

Do not read `hudkit.layout` during CS2 prelude eval.

`hudkit` methods and the `layout` getter used to close over `__s2_game_ns("ui")`, a load-window proxy. `run_prelude` evaluates `pawn.js` before `__s2_load_ctx` exists, so `menuhud`'s `hudkit.modal()` and the vote rail's eager `hudkit.layout` both threw `ui outside the load window` and aborted the rest of the prelude — `Vote.registerTallyRenderer` never ran.

Resolve the shared kit through the `ui` factory instead. The vote rail holds `hudkit` and reads `.layout` inside functions, same shape as menuhud.
