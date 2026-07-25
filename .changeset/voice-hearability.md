---
'@s2script/sdk': minor
---

Add `@s2script/sdk/voice` — per-(receiver, sender) voice hearability.

`Voice.setAudibleTo(sender, receivers)` restricts who can hear a speaker; rules from multiple plugins
AND-merge so one plugin can only narrow another's, never widen it. Layered under `Client.voiceMuted`,
which still wins, so admin moderation always beats a gameplay rule.

Declarative rather than a callback: the engine's listen matrix is re-asserted continuously and the
underlying hook fires per pair, so a per-pair JS callback would run up to 64x64 times per refresh.
`Voice.stats()?.rewrites` is the effect counter (stats is nullable — null means the running shim predates the capability) — a rule that never rewrites is not taking effect.
