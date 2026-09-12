# CS2 camera and HUD update — September 9, 2026

Sources: Valve's [release notes](https://store.steampowered.com/news/posts/?appids=730),
the engine-shipped [script declarations](https://github.com/SteamDatabase/GameTracking-CS2/blob/master/content/csgo_addons/cs_script_demo/maps/scripts/point_script.d.ts)
and [FGD](https://github.com/SteamDatabase/GameTracking-CS2/blob/master/game/csgo/csgo.fgd).
The zoo map's declaration copy still has the old camera API; use cs_script_demo.

## HUD migration

`CustomHudLayout.create({ addons, resource, observable: true })` opts into showing
the spectated player's version. Omit the option or use false to retain the viewer's
own UI. It is a spawn keyvalue, fixed for the entity lifetime. Conflicting resource
reuse in one plugin throws; separate plugins with opposite policies use distinct
entity names. Shared hudkit explicitly uses false. Use a separate custom resource
for observable UI. This behavior requires the updated CS2 engine.

Observation controls rendering, not network confidentiality or click authorization.
Do not put secrets in HUD state. The normal spectator presentation problem no longer
requires one entity per recipient; transmit filtering remains a separate mechanism.

Valve applies `HUD_BUYMENU_VISIBLE` and `HUD_SCOREBOARD_VISIBLE` to an ancestor panel.
Use descendant selectors such as `.HUD_SCOREBOARD_VISIBLE .my-overlay`, not server
class writes. The library CSS hides passive toasts, badges, banners and callouts
while either base panel is open. Interactive panels stay visible because CSS cannot
release server-owned cursor capture. Custom layouts always draw above the base HUD;
internal z-index values only order custom content.

The visibility class does not expose the base buy menu's controls or purchase logic.
It supports reacting to or visually covering that menu, not replacing its behavior.

Compile and republish the workshop stylesheet to deliver the CSS changes. Updating
the server bundle alone does not update clients' workshop assets.

Keep entity-bound paint caches, connection identities, capture ownership and cleanup.
Valve fixed client spectator state; those server lifecycle protections address
different problems. No speculative per-frame spectator repaint was added.

## Camera binding status

`CustomPlayerCamera` is a script class, not a verified replacement entity classname
or a set of entity inputs. A pawn's `GetCustomCamera()` creates its single camera on
demand. `GetPlayer()` returns the owner. `GetMode()` / `SetMode()` use these modes:

- `DISABLED` (0): player eye position and angles.
- `CONTROLLED` (1): camera origin and angles.
- `CONTROLLED_POSITION` (2): camera origin, player-controlled angles.
- `FOLLOW_POSITION` (3): offset from a followed position, player-controlled angles.

`SetFollowConfig()` requires `followEntity`; optional fields are `followEyes`,
`followOffset`, `cameraOffset`, `clipCameraOffset`, `cameraOffsetReturnStrength`.
Camera offset axes are forward/left/up, rotated by player eye angles. Return strength
defaults to 1 (instant). Configure following before entering follow mode.
`GetCamera`, `CSPlayerCamera`, `IsEnabled`, `SetEnabled`, and
`SetIsControllingAngles` are deprecated upstream.

The s2script API now exposes `pawn.getCustomCamera()` and `CustomPlayerCamera` with
`getPlayer()`, `getMode()`, `setMode()` and `setFollowConfig()`. It uses the engine's
pawn-owned camera; it does not delete that entity on plugin unload. State is shared
with other scripts controlling the same pawn, so disable your camera when finished.
The old hud-lab I/O probe is replaced by native `sm_cam_info`, `sm_cam_mode` and
`sm_cam_follow` commands. Legacy enable/angles/remove commands map to modes; remove
now disables instead of deleting an engine-owned entity.

Nebula's isolated `s2script-cs2-hardening` installation was restarted with the user's
approval. SteamCMD reported successful installation; it now runs build `2000908`,
patch `1.41.8.1`, source revision `10981323`, dated September 9. Metamod reports
s2script loaded and plugins active. The separate stopped hudlab installation was
not restarted. The updated server library SHA-256 is
`a4c54f83bb487b90d2d018a292e104c5b173c0dda542aa2f61dce16dc1e4c6a3`.

Native binding evidence on that binary:

- `GetCustomCamera`: Valve's named script wrapper calls `0x1310960` at `0xb52c69`.
  Its deprecated counterpart has identical native instruction bytes. The new
  engine-generic `validated-call` resolver filters call-site pattern matches using
  the exact `GetCustomCamera` string xref at call-site +127, then follows E8 rel32.
  Zero or multiple validated sites, non-call opcodes, or out-of-text targets degrade.
  For this resolver only, validator offsets refer to the **call site**, not the target.
- `SetMode`: script wrapper call at `0xb46137` targets `0x130e450`, taking camera
  in rdi and mode in esi. The native updates both network state and camera services.
- `SetFollowConfig`: script wrapper call at `0xb422b6` targets `0x1310240`.
  Arguments: camera rdi, entity rsi, followEyes edx, followOffset rcx, cameraOffset r8,
  clip r9d, returnStrength xmm0. Defaults verified in wrapper: false, zero vectors,
  false, 1. Existing engine-call marshalling handles all of these.
- The live schema dump confirms `CCSCustomPlayerCamera.m_hPawn` and one-byte
  `m_nCameraMode`. Getters resolve offsets at runtime; setters call natives.

Addresses above are RE evidence, never shipped constants. Gamedata masks relocations
and field displacements. The `validated-call` strategy requires the updated shim;
older shims degrade camera acquisition instead of calling an ambiguous function.

## Verification recorded

- After integration onto current main, the JS gate passed all 740 SDK tests and its remaining JavaScript/typecheck checks;
  the final Docker-only gate could not run because Docker is unavailable locally.
- The shipped native resolver unit tests passed on Linux and under x86-64 emulation
  on macOS, with address and undefined-behavior sanitizers enabled.
- The complete shim built with GCC in Debian bullseye; its highest required glibc
  version is 2.17, within CS2's 2.31 runtime. All 47 required core exports are present
  in the existing server core.
- The shipped C++ resolver also resolved all three descriptors against a mapped
  copy of the updated server ELF, with the same sanitizers. The acquisition pattern
  pins the LEA opcode at +127 as well as checking the exact referenced string.
- Independent code/binary review confirmed acquisition ownership, mode width,
  setter argument order and synchronous vector copying.

The updated shim, game-package JS and gamedata were installed on the isolated
hardening server on September 11. CS2 remains build 2000908 with the ELF hash above.
The live bot probe passed camera owner lookup, stable acquisition, all four modes,
and default/custom follow configuration readbacks (entity, eyes, both vectors,
clipping and return strength). Cleanup returned the camera to mode 0.

The startup gate also caught three HUD setters whose old signatures pinned the
previous player-state-vector offset. Re-resolved through Valve's script registration:

| Setter | Wrapper call | Native target |
| --- | --- | --- |
| SetHasClassForPlayer | `0xb44d1c` | `0x1319320` |
| SetDialogVariableStringForPlayer | `0xb446fa` | `0x1318cc0` |
| SetInputCaptureEnabled | `0xb440b8` | `0x130f9a0` |

All three match uniquely, retain their previous argument ABI, and are armed on the
live server. Field offsets and state stride are masked in their patterns. The live
HUD probe passed for both observable values: spawn readback, nonempty class/dialog
state, capture enable, and capture release. No camera/HUD call is degraded.

The two probe HUD entities were removed, input capture released, and both temporary
probe archives removed. The temporary build container was removed. Build outputs,
logs and the original runtime backup remain under
`/home/ghirakawa/s2script-camera-build` on Nebula.

Deployed hashes:

- Shim: `941f5af1bfc2f26ec6bd11748e279066bc029c8fc6b443594d8bdbe9f2d467ef`
- Game gamedata: `29c4e25302642430fcbf19617fe500d2e3e4c05deb8ae9119c53de8ba492f439`
- Game JS bundle: `3b3cf41888d716edfe6c4cab1ea128b1e83b9c6b1a9de036b0cb225041aa1478`

Hud-lab diagnostics now resolve their field offsets from live schema, rejecting
missing fields before writes. State stride is derived from the embedded global
state's span (408 bytes). This also corrects the old borrowed player-slot offset
of +408 to the live field at +48.

## Client acceptance (pending)

Offline tests verify descriptor validation, spawn arguments, policy reuse and
lifecycle behavior. The live bot checks cover native state, but cannot prove client rendering or spectator behavior.

1. Verify existing HUD gamedata against the updated test server. Record game build,
   server and workshop asset hashes.
2. In separate cleaned probe runs, use `s2_hudprivacy create A 0` and `create A 1`.
   Set audience to all to isolate observation from transmit filtering. Paint distinct
   non-secret markers for each human slot.
3. With two players and an observer, switch A → B → A, first-person → chase →
   roaming, death → respawn, and reconnect. Check the viewer's own UI with false and
   the watched player's with true; no stale classes, values or visibility. Check that
   observers cannot activate the target player's actions.
4. Open/close buy menu and scoreboard. Passive overlays yield and restore; interactive
   panels must not become invisible while holding capture. Verify base-HUD stacking.
5. Reload plugins and change maps. Repeat with two plugins sharing the same policy,
   then opposite policies; check for duplicate rendering from separate entities.
6. Verify all four camera modes, owner lookup, origin/eye
   following, offsets, clipping/return, disconnect, respawn, map change and unload.
   Automated state checks do not substitute for observing a human client view.
