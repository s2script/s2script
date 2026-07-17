---
"@s2script/sdk": minor
---

Voice control: `Client.voiceMuted` (get/set — server-side mute of the client's outgoing voice for all
receivers, enforced by a SetClientListening rewrite hook) and `Clients.onVoice(handler)` (throttled
voice-transmission notification). Degrades to an inert no-op with a named reason if the voice
descriptor fails validation.
