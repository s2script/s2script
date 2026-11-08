---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

Add SourceMod's admin-activity layer as `Activity` on `@s2script/sdk`: `show`/`showT` (ShowActivity2, governed by the `sm_show_activity` cvar), `log` (LogAction), `describe` (`%L`), `targetName`, `tag` and `flags`. It is engine-generic, so it moved out of `@s2script/cs2`; `@s2script/cs2`'s `Activity` is now a deprecated re-export of the same object, and `formatSource` reads `sm_show_activity` instead of always using the default.
