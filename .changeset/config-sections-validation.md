---
"@s2script/sdk": minor
---

config: sectioned config blocks + enriched validation. `s2script.config` entries may now nest into
sections (any entry without a string-valued `type` key is a section, recursed), and decls gain
`min`/`max` (int/float, mutually exclusive with `enum`), `enum` (string/int), `group`/`label`, and
`sensitive` (masked in display, still written to the file). `validateConfigBlock` enforces all of
these plus the ban on `.` in key names. The `@s2script/config` `Config` type widens to a recursive
`Record<string, ConfigValue>` so nested sections type-check; this is an additive `.d.ts` widening
(no apiVersion bump — plugins that used the flat scalar shape still type-check).
