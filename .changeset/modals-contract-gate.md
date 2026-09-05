---
"@s2script/cs2": patch
---

ui: document the one-handler-table-per-modal contract on `ModalSpec.buttons`, and warn on live divergence in debug mode

A modal's footer click-dispatch table is one array per MODAL, rebuilt on every
paint. Button TEXT is per-player (the *ForPlayer natives), so a per-slot
`buttons(slot)` function LOOKS like it can serve different button sets to
different players — but the last paint's handlers dispatch for every player
with the sheet open. This cost a live incident (a player's shop footer
dispatching an admin's Ban handler) and was documented only in a consumer
plugin.

The contract now lives where authors will see it: `ModalSpec.buttons` in
`ui.d.ts` states that a per-slot function may vary text/variant but must return
the same handlers in the same order for every slot, and that genuinely
different action sets need a modal each. With `globalThis.__s2_ui_debug = true`
the library logs a warning when `buttons(slot)` returns differing lengths or
handler identities for two slots that hold the same sheet concurrently (once
per claimed modal; the library's own pager closures are exempt — they dispatch
on the click-time slot and are safe to share).
