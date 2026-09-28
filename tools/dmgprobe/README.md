# dmgprobe

Live-gate fixture for `SDKHook(OnTakeDamage / OnTakeDamagePost)` on the CS2 package's trusted
`takeDamageOld` function. Not shipped.

Prefix: `[DMGPROBE]`. Drive with `python3 scripts/rcon.py <cmd>`:

- `dmg_hook` — hooks Pre + Post on every current pawn.
- `dmg_hurt <amount>` — spawns a `point_hurt` aimed at slot 0 through `!activator` and fires `Hurt`,
  so the damage goes through the engine's own damage path. Logs health and counters 1s later.
- `dmg_scale <factor>` — the Pre hook multiplies `info.damage` (`0` = block).
- `dmg_report` — counters, last delivery, and every pawn's index and health.
- `dmg_fall` — teleport-drop. Kept for reference; teleport did not produce fall damage on the
  2026-09-25 server, so use `dmg_hurt`.

Expected: `dmg_hurt 30` takes 30 health off; after `dmg_scale 0.5` it takes 15; after `dmg_scale 0`
health does not change. Each hit bumps pre and post by one, with `hookedOn == victim`. A radius
`point_hurt` (no target) falls off with distance, so CS2 rounds its damage down to 0 health lost.
