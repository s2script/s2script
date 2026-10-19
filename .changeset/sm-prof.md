---
"@s2script/sdk": minor
---

`sm prof start [seconds] [js] | stop | status | dump | clear`: the runtime profiler, as in SourceMod.
It reports each plugin's and callback's time, each frame's interval and s2script's share of it (with
a verdict on the longest frame), and writes a Chrome trace plus, with `js`, a V8 `.cpuprofile`.
Plugin stack traces now name the plugin (`s2s://<id>/plugin.js`) instead of `<anonymous>`.
