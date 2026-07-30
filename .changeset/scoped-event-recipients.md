---
"@s2script/sdk": minor
---

Add `Events.setRecipients` and an `onGameFrame` phase, and stop swallowing a throwing event pre-hook

`HookResult.Handled` on a game event has always been all-or-nothing. CS2 hands an event to clients
as one `CMsgSource1LegacyGameEvent` **per client**, so suppression either hides it from everybody or
from nobody — there was no way to express "only these players see this". A hidden-role gamemode
needs exactly that: the kill feed must reach the killer and their team-mates and no one else.

`setRecipients(slots)` names the viewers for the event currently being pre-dispatched. Paired with a
`Handled` return it means "do not broadcast this normally — deliver it to exactly these viewers".
Filtering the per-client posts keeps every field the engine populated, which re-firing a rebuilt
copy per viewer does not. Setting no mask changes nothing, so silence is never read as consent.

```ts
ctx.events.onPre("player_death", () => {
  Events.setRecipients(traitorSlots);   // only Traitors see the kill
  return HookResult.Handled;
});
```

`onGameFrame` now takes a `phase`. It defaults to `"pre"` (before simulation, unchanged); `"post"`
runs after the engine's own per-frame writes, which is what a netvar the engine re-derives during
simulation needs — written in `"pre"` it is overwritten every frame.

Separately, a pre-hook that THREW was dropped in complete silence: its `HookResult` never reached
the collapse, so the chain came out a vote short and the engine broadcast an event the plugin
believed it had suppressed. That failure mode cost days on a real plugin whose handler threw on its
final `return HookResult.Handled` statement — everything looked correct from the outside. It now
logs, matching what `dispatch_usercmd` has always done.
