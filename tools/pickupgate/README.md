# pickupgate

Live-gate fixture for `ctx.items.onCanAcquire` / `onCanAcquirePost`. Not shipped.

Prefix: `[PICKUPGATE]`. Drive with `python3 scripts/rcon.py pickup_report` / `pickup_deny` / `pickup_give` / `pickup_reenter`.

Checks (spec §8):

1. Natural pickup / round-start loadout fires the handler.
2. `pawn.giveNamedItem` from JS is isolate-re-entrant (skipped + named). Engine-originated acquire fires.
3. `pickup_deny` then `mp_restartgame` with `mp_*_default_primary weapon_negev` withholds that item (`Handled` + `InvalidItem=1`).
4. `pickup_reenter` then a restart: inner `giveNamedItem` is skipped (no nested Pre).
5. Boot log: hook is *armed* but not installed until first subscribe.
6. After a denied Negev, Post logs `skipped=true` `result=1`.

Modes only consume `defIndex=28` (Negev) so the AK AlreadyOwned poll cannot steal them.
