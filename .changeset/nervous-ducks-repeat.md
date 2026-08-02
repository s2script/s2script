---
"@s2script/cs2": patch
---

Correct the `Player.respawn()` / `GameRules.terminateRound()` doc comments to describe the mechanism
that now backs them. Both were retired from bespoke shim C++ into declarative gamedata descriptors
(A5b) with their next-frame drains reimplemented in the game package, so "executes outside the JS
isolate borrow" is no longer how the delivery guarantee is obtained — the deferred-dispatch queue is.
Two behaviours that were always true but undocumented are now stated: `respawn()` is idempotent for
the same player within one frame (both calls return `true`), and `terminateRound()` is single-slot
latest-wins. No signature, argument or return type changes.
