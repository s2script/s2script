# cvarprobe

Live-gate fixture for `Server.setCvar` on numeric cvars. Not shipped.

Prefix: `[CVARPROBE]`. Build with `npx @s2script/sdk build tools/cvarprobe`, copy
`dist/_cvarprobe_a.s2sp` into `addons/s2script/plugins/`, then drive it with `python3 scripts/rcon.py`:

- `cvar_probe` sets `sv_infinite_ammo` to 1, then 2, then 0.
- `cvar_probe <name> <value>...` does the same for any cvar and values.

After each `setCvar` the probe runs the cvar name as a console command through `Server.commandEx`
and reads the value from the engine's own reply. It does not trust `Server.getCvar`, because that
reads the slot through the same layout `setCvar` writes. A step is `PASS` when the engine prints
exactly the value just set and the default it printed before the run has not changed.

Expected: three `PASS` lines and `DONE all 3 steps PASS`. Before the fix, the default value took
stack bytes on the first write, so the default line changed, and the next map load or cheat revert
put the garbage into the value.

If the `before:` line shows `engine value=null`, the console reply didn't match the parser. The
raw text is printed on that line, so adjust `engineReport` to it.
