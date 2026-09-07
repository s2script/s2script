# Final 60-minute live soak

The loader-aware mixed soak passed on the isolated `s2script-cs2-hardening` server using exact
production source `ba6c7c19548fdad46d14bb5c2aa342ce94804ae1`. The collector SHA-256 was
`843f6ec6c7834ae87cd016c7ab0f5cd3b55b014744c7ed8379cb0c68af0a19b3`; the installed mixed-live
fixture SHA-256 was `ef594c964e512438c05d0875a4f71c12902d7d46c269c9047fb69d8c66b06c94`.

- [Final report](report.json) — 57/57 cycles, result `PASS`, zero errors.
- [Cycle checkpoints](checkpoints.json) — per-cycle status, limits, gauges and observations.
- [Complete raw evidence](raw-evidence.tar.gz) — command output, logs, stats and RSS captures.
- [Run manifest](manifest.json) and [final plugin list](plugins-final.txt).
- Key snapshots: [baseline](stats-baseline-sample-001.json),
  [pressure before](stats-pressure-before-sample-001.json),
  [pressure after](stats-pressure-after-sample-001.json), and
  [final restored state](stats-final-restored-sample-001.json).

The run recorded 58 reload acknowledgements and 58 matching Active transitions. Final status
checkpoints recorded 48 actual bot-churn attempts and five actual same-slot reuses. The last ten
cycles skipped churn because no eligible bot was available; those skips received no attempt or
reuse proof credit. Ten retained test handles were cleaned at shutdown.

Pressure settled all 256 requests: 64 succeeded, 192 received the expected named capacity
rejection, and zero had another outcome. The fixture restored the original 94-byte JSONC config
exactly. Loader accounting returned from the 395-byte measured-config plateau to the 472-byte
original-config plateau.

The `lastNs` measurements are one sampled async-drain value per measured cycle, not percentiles of
engine frame time. RSS is a whole-container process observation and is not an asserted memory
bound. Human authenticated reconnect, rendered HUD/chat/console behavior, and actual SetTransmit
callbacks from a signed-on viewer remain pending. The pre-existing unavailable EndTouch descriptor
and the development-only js-yaml advisory remain documented in
[the Linux acceptance record](../linux-native-acceptance.md).
