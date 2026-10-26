---
"@s2script/cs2": minor
---

List the maps in a map group. `MapGroups.maps(name)` in `@s2script/cs2` returns the maps of a
gamemodes map group (`mg_active`, `workshop`, …) as `string[]`, or null for an unknown group, like
ModSharp's GetMapGroupMapList. Map-vote and nominations plugins can offer the server's own map
group instead of a hand-kept list.
