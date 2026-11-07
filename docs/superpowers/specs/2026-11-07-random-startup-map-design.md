# Random startup map

When `@s2script/nextmap` first observes a live map after loading, choose a random usable entry from its existing `maplist.txt`. Allow the configured server default to load first. Use the existing `nextmap_change_delay`, announcement, validated stock/workshop command, and stale-map cancellation. If the chosen map is already running, leave it running. If no usable entries exist, log once and leave the current map alone.

Keep the normal sequential rotation, round/time limits, and admin override after this one startup choice. Plugins persist across map changes, so consume the startup choice before scheduling it; later maps must never start a random-switch loop. A fresh plugin load gets one startup choice, including manual loading/reloading. No new setting or native engine API is required.

## Approaches

Use the existing first-map poll and delayed change path: the smallest change, compatible with plugins loaded before or after the initial map. A new map-start hook would duplicate the existing state reset and need a late-load fallback. Altering server launch configuration cannot consult the plugin-owned list and is outside this request. Use the existing poll.

Filter malformed console names/IDs and unavailable stock maps before choosing. Workshop entries use their numeric ID even before their BSP is installed. An absent file retains the plugin's current template-generation behavior. Live engine acceptance remains separate from the offline command/lifecycle tests.

## Round-boundary follow-up

Normal nextmap rotation should finish the current round before switching. `mp_timelimit` and `mp_maxrounds` queue and announce the target; after `nextmap_change_delay` elapses, the next `round_prestart` applies it once. A countdown that expires after prestart waits for a later prestart rather than interrupting the new round. A final `cs_win_panel_match` also allows a ready change, since there may be no following round; if the countdown is still running at match end, it completes before applying the change. Clear queued work on a detected map transition and recheck the source map at execution.

RTV winners likewise apply at `round_prestart` or final match end rather than immediately on `round_end`, preserving stock/workshop commands and clearing stale winners. The one-time random startup selection keeps its prompt delayed switch so an empty server can enter its configured maplist without waiting for a round. Explicit admin map commands retain their existing timing. Keep the existing config key/default and describe its delay as a minimum for rotation. Extend the current PR with this related map-switch timing behavior.

Both plugins may be ready on the same boundary. Nextmap publishes protocol-2 `OnAutoMapChange` (`@s2script/nextmap` contract `1.0.0`); RTV optionally subscribes and returns `Handled` while it owns a winner for that source map, including after its command was queued. This gives a voted map priority regardless of handler order and emits exactly one engine command. Each plugin remains standalone; startup switches bypass the hook.
