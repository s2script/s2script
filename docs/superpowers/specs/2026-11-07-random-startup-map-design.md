# Random startup map

When `@s2script/nextmap` first observes a live map after loading, choose a random usable entry from its existing `maplist.txt`. Allow the configured server default to load first. Use the existing `nextmap_change_delay`, announcement, validated stock/workshop command, and stale-map cancellation. If the chosen map is already running, leave it running. If no usable entries exist, log once and leave the current map alone.

Keep the normal sequential rotation, round/time limits, and admin override after this one startup choice. Plugins persist across map changes, so consume the startup choice before scheduling it; later maps must never start a random-switch loop. A fresh plugin load gets one startup choice, including manual loading/reloading. No new setting or native engine API is required.

## Approaches

Use the existing first-map poll and delayed change path: the smallest change, compatible with plugins loaded before or after the initial map. A new map-start hook would duplicate the existing state reset and need a late-load fallback. Altering server launch configuration cannot consult the plugin-owned list and is outside this request. Use the existing poll.

Filter malformed console names/IDs and unavailable stock maps before choosing. Workshop entries use their numeric ID even before their BSP is installed. An absent file retains the plugin's current template-generation behavior. Live engine acceptance remains separate from the offline command/lifecycle tests.
