---
"@s2script/cs2": minor
---

`chat.onCanHear` in `@s2script/cs2`: decide whether one player sees another's chat message. It hooks
`CCSGameRules::PlayerCanHearChat`, which the engine asks once per recipient. A handler writes `canHear`
and returns `Handled` to override the engine's rule. Any `false` vote wins, and `Continue` leaves the
decision to the engine. This is the hook dead-chat and team-chat rules need. Before it, plugins could
only drop a whole message and re-send it.
