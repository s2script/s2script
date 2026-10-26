---
"@s2script/cs2": minor
---

Center-screen text helpers in `@s2script/cs2`: `CenterText.to/toAll` (SourceMod's `PrintCenterText`),
`AlertText.to/toAll` (CS2's alert line) and `CenterHtml.to/toAll/clear`, the HTML center panel. The
package re-sends `CenterHtml` every frame until its duration runs out, so plugins no longer hand-roll
a `show_survival_respawn_status` frame loop.
