---
"@s2script/sdk": minor
---

Look up sound events by name. `Sound.duration(name)` in `@s2script/sdk/sound` returns an event's
length in seconds, or null when the engine has no such event, and `Sound.isValid(name)` says whether
a loaded soundevents file defines it. This matches ModSharp's GetSoundDuration / IsSoundEventValid.
