# reentrygate

Live-gate fixture for isolate re-entry (outbound nest). Not shipped.

Prefix: `[REENTRYGATE]`. Drive with `python3 scripts/rcon.py re_give` / `re_respawn` / `re_cvar` / `re_report`.

Checks:

1. `pawn.giveNamedItem(Negev)` runs `ctx.items.onCanAcquire` before it returns (`PASS=true`). Needs the CanAcquire hook from #107.
2. `pawn.slay()` runs `Events.onPre("player_death")` before it returns.
3. `Server.setCvar` applies now: `getCvar` reads the new value and `onCvarChange` fires before return.
4. `player.respawn()` is synchronous. On this custom/warmup box `CCSPlayerController::Respawn` no-ops mid-round (game rules, not isolate skip).
