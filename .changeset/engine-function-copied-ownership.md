---
"@s2script/sdk": minor
---

`s2s build` engine-function declarations: copied `string` and `vector` positions now need an explicit
native `ownership`. Parameters take `callee-borrowed`, `callee-retained` or `native-observed`; a
copied return uses the object form `{ "type": "string" | "vector", "ownership": "caller-borrowed" |
"native-observed" }`. A declaration without it fails the build with "copied ownership is required;
rebuild the declaration". `native-observed` positions cannot be called, cannot be PRE-mutable, and a
`native-observed` return needs `"suppression": "none"` for PRE.

Functions may now declare `"suppression": "generic" | "none"` (the default is `generic` when the
function has a PRE surface). With `none`, the generated PRE handler type is `0 | 1 | void`: such a
hook cannot skip the original or supply a return value.
