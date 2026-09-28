# S3 live acceptance receipt (2026-09-25)

**Verdict: live criteria met.** The HUD click passed with a human client on 2026-09-28. Several
runtime cases still have only unit or fixture evidence; every row says which.

## Artifact

Fresh install of the published distribution: the release zip was extracted over an empty
`addons/s2script` on the owned test server `s2script-cs2-hardening` (Nebula, stock Metamod +
MultiAddonManager), after a backup of the previous addons. `data/` and `configs/` were
chowned to the server uid (1000); nothing else was edited.

| Item | Value |
|---|---|
| Commit | `30b113aa` (PR #226 on #225 `63ba47c5`) |
| Release zip | `s2script-cs2-linux-0.0.0-s2s3.30b113aa.zip` sha256 `78fe8703…2327a` |
| `libs2script_core.so` | `1f3f7811…01f9f8` |
| `s2script.so` | `16101907…15f9a0` |
| `game-packages.json` | `cbdc7fdc…93dd51` |
| `game-packages/cs2/index.js` | `f982054d…9f1042f` |
| `game-packages/cs2/gamedata.json` | `85bd6fb5…21e24c1de3` |
| `game-packages/cs2/trusted-functions.json` | `dffaccdc…4de5a5` |
| `meta list` | `s2script (0.0.0-s2s3.30b113aa)` |

Fixtures were dropped into `plugins/` beside the shipped base plugins: `tools/pickupgate`,
`tools/reentrygate` (detourgate), `tools/dmgprobe`.

## Selection and bootstrap

| Criterion | Result | Evidence |
|---|---|---|
| Status names `@s2script/cs2`, owner `cs2`, manifest path, hashes | PASS | `game-package-status` `code:"active"`; hashes match the table above |
| All trusted functions available | PASS | `canAcquire`, `customHudClicked`, `takeDamageOld` all `binding:"available"` |
| Offsets from the live schema only | PASS | five offsets, each `source:"live-schema"` |
| No `pawn.js` fallback | PASS | the install has no `js/` directory; 0 `pawn.js` log lines since install |
| No operator repairs | PASS | `operatorRepairs: []`, `customRepairs: []` |

## Runtime cases

| Case | Result | Evidence |
|---|---|---|
| Acquire: handler fires with item + player | PASS | pickupgate `defIndex=28`, `player=0` via `hiddenReferencedBy` |
| Acquire: deny | PASS | Negev withheld; POST `result=1 skipped=true` |
| Acquire: plugin's own command-triggered give is gated | PASS | after target-scoped `ParentBusy` (`63ba47c5`) |
| Acquire: nested give from inside the gate | PASS | only the causing plugin is skipped for the nested call |
| Acquire: vote fold across plugins | unit only | `acquire.test.js`: Handled/Stop outrank Changed; first deny wins across contexts. Not driven live |
| HUD click, callback before original | PASS (human, 2026-09-28) | see "HUD click" below |
| Damage Pre observe (victim, attacker, damage) | PASS | `dmg_hurt 30`: `hookedOn=470 victim=470 damage=30`, health 100→70 |
| Damage Pre mutate | PASS | `dmg_scale 0.5`: health 70→55 |
| Damage block | PASS | `dmg_scale 0`: health stays 55 |
| Damage Post observe | PASS | post counter equals pre on every hit (3/3) |
| Damage per-victim filter | PASS | 8 pawns hooked; only the hit pawn's hook fired |
| Map change | PASS | `changelevel de_dust2`: package active, damage on the new pawn 753 (100→75), pickupgate still dispatching (2500 pre and 2500 post) |
| Repeated `.s2sp` hot reload | PASS | dmgprobe replaced five times this run; each reload re-registered and hooked cleanly |
| Ammo write | unit only | `damage.test.js` `Weapon.setAmmo` (schema write, stale guard). Not driven live |
| Community function sharing a physical binding | not driven live | stock bridge + production V8 proof on Linux |
| Stale borrowed view / stale closure | unit/Linux proof | `borrowed_proof` foreign-context and retire steps |
| Unload requested inside a callback | not driven live | |
| Crashes or restarts during runtime cases | none | `restarts=0`, empty `crashes/` |

## Known, outside S3

- This CS2 build has stale core signatures (`CollisionUpdatePartition`, `DispatchTraceAttack`,
  `EndTouch`, `FireOutputInternal`, `IGameSystem_InitAllSystems_pFirst`, `PostThink`, …). Each
  degrades only its own descriptor, as designed. They need a gamedata regeneration, not S3 work.
- A `point_hurt` with only a radius falls off with distance, so small hits floor to 0 health lost.
  That is engine behavior, not an adapter fault.

## HUD click (human client, 2026-09-28)

CS2 build 25537370, a fresh install of this stack's release (with #227's gamedata), and a human
client with workshop addon 3790153369. Two blockers outside S3 had to be cleared first:
- **MultiAddonManager v1.6 was stale for the Sep 24 update.** It wrote the client addon list at
  CNetworkGameServer+344, but the engine now reads +376, so clients were sent `addons:''` and never
  mounted the layouts (`ERROR_FILEOPEN` on every `.vxml_c`). Upstream v1.6.1 fixes the offset, and
  it is installed on the test server.
- **#227 had mapped `SetDialogVariableStringForPlayer` to the wrapper's revert branch**, so the
  panels drew with blank text. Corrected in #227 `be9e9535`.

hud-lab's `sm_kit` (a hudkit modal) then drew with text. Clicking each footer button logged, in
this order:

```
[hud-lab] kit button "Bravo" clicked by slot 2
[hud-lab] RAW CLICK button="s2_m0_f1" by gkh (slot 2)
[hud-lab] kit button "Alpha" clicked by slot 2
[hud-lab] RAW CLICK button="s2_m0_f0" by gkh (slot 2)
[hud-lab] kit button "Charlie" clicked by slot 2
[hud-lab] RAW CLICK button="s2_m0_f2" by gkh (slot 2)
[hud-lab] RAW CLICK button="s2_m0_r2" by gkh (slot 2)
```

The path: client click → `CS_UM_CustomHudClicked` → `CustomHudClickedReceiver` (the trusted
`customHudClicked` function) → the `legacy.hud-click.v1` adapter → the plugin's `onClick` and the
raw `CustomHudLayout.onClicked`. Each id maps to the button clicked, and a list-row click
(`s2_m0_r2`) is delivered too. A live engine hook confirmed input capture reached
`SetInputCaptureEnabledForPlayer` for that slot on the layout entity.

## Plugin reload clears the panels it was showing (human client, 2026-09-28)

Found live: hot-reloading hud-lab while its kit modal was open left the sheet on screen with no
handler behind it, so nothing could close it. Cursor capture was already released by its
owner-swept switch lease, but the non-focused modal held no host-owned record of its visibility.
Fixed in `e55ae36b`: a non-focused modal now holds an owned, hide-only surface lease while open,
and the host's owner sweep on unload/reload retires it.

Retest on CS2 25588766, fresh install: the player ran `sm_kit` (22:46:05) and left it open, and
hud-lab was hot-reloaded 8 s later (22:46:13, no warnings). The player confirmed the sheet
disappeared on its own and the mouse returned to normal.
