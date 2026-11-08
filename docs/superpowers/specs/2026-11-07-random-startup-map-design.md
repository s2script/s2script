# Configurable startup maps and round-boundary rotation

When `@s2script/nextmap` first observes a live map after loading, read `nextmap_random_startup` (bool, default `false`). With the default, leave startup alone and run normal rotation, matching SourceMod. When enabled, choose a random usable entry from the existing `maplist.txt`. Allow the configured server default to load first. Use the existing `nextmap_change_delay`, announcement, validated stock/workshop command, and stale-map cancellation. If the chosen map is already running, leave it running. If no usable entries exist, log once and leave the current map alone.

Keep normal sequential rotation, round/time limits, and admin overrides. Consume the startup choice at the first live-map observation whether the flag is enabled or disabled. Turning the flag on afterward must not trigger a random switch on a later poll or map. A fresh plugin load gets one startup choice, including manual loading/reloading. No new native engine API is required.

## Approaches

Use the existing first-map poll and delayed change path: the smallest change, compatible with plugins loaded before or after the initial map. A new map-start hook would duplicate the existing state reset and need a late-load fallback. Altering server launch configuration cannot consult the plugin-owned list and is outside this request. Use the existing poll.

Filter malformed console names/IDs and unavailable stock maps before choosing. Workshop entries use their numeric ID even before their BSP is installed. An absent file retains the plugin's current template-generation behavior. Live engine acceptance remains separate from the offline command/lifecycle tests.

## Round-boundary follow-up

`nextmap_wait_for_round` (bool, default `true`) controls normal rotation. `mp_timelimit` and `mp_maxrounds` queue and announce the target. When enabled, the next `round_prestart` after `nextmap_change_delay` elapses applies it once. A countdown that expires after prestart waits for a later prestart rather than interrupting the new round. A final `cs_win_panel_match` also allows a ready change, since there may be no following round; if the countdown is still running at match end, it completes before applying the change. When disabled, switch as soon as the delay expires, preserving the old timing. Clear queued work on a detected map transition and recheck the source map at execution in either mode.

RTV winners apply at `round_prestart` or final match end rather than immediately on `round_end`, preserving stock/workshop commands and clearing stale winners. Enabled random startup selection keeps its prompt delayed switch regardless of the round-wait flag, so an empty server can enter its configured maplist without waiting for a round. Explicit admin map commands retain their existing timing. Keep `nextmap_change_delay`'s key/default and describe both timing modes: it is a minimum before a round boundary when round waiting is enabled, and the exact countdown otherwise. Use `Changing After Round` only for round-waiting rotation; use `Changing To` for immediate rotation and startup.

Both plugins may be ready to switch. Nextmap publishes protocol-2 `OnAutoMapChange` (`@s2script/nextmap` contract `1.0.0`); RTV optionally subscribes and returns `Handled` while it owns a winner for that source map, including after its command was queued. Dispatch this hook for normal rotation in both timing modes, independently of the round-wait flag. This gives a voted map priority regardless of handler order and emits exactly one engine command. Each plugin remains standalone; startup switches bypass the hook.
