---
"@s2script/sound": minor
"@s2script/cs2": minor
---

Sound slice: new `@s2script/sound` module — `Sound.emit(name, { entity?, recipients?, volume? })`
plays a named CS2 SoundEvent (engine GUID or 0; serial-gated source, bot recipients skipped) and
`Sound.onPrecache(ctx => ctx.add(path))` registers custom resources into the session manifest at
map load. CS2 sugar: `pawn.emitSound(name, opts)` + the curated `Sounds` constants.
