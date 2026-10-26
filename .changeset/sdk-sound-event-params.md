---
"@s2script/sdk": minor
---

Change a sound while it plays. `Sound.start(name, opts)` in `@s2script/sdk/sound` emits like
`Sound.emit` and returns a `SoundEvent` handle (or null). `event.setParam(name, value)` sets a
soundevent parameter such as `volume`, `pitch`, `position` or a `public.*` field (a number or a
Vector), and `event.stop()` stops that one instance. Both take optional `recipients`. This matches
ModSharp's StartSoundEvent / SetSoundEventParam / StopSoundEvent.
