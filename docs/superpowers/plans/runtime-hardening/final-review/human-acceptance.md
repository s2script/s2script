# Human acceptance runbook

Status: pending; the user will join later. The complete automated soak passed. The test server currently runs 19 plugins: the 18 soak plugins plus the temporary viewer fixture. Remote joining is enabled with runtime `sv_lan 0`; existing client-side MAM delivery requests addon `3790153369`. No human assertion has been performed or counted as passing.

Use only the isolated `s2script-cs2-hardening` container at `nebula.gkh.dev:27016`. Finish and archive the mixed soak before changing its workload, configuration or plugin set. The production `s2script-hudlab` instance is outside this procedure.

## Join and HUD preparation

Read-only preparation found TCP and UDP 27016 published on the host with Docker forwarding rules, UFW inactive, and successful anonymous Steam login in the owned server's logs. Actual off-host reachability and acceptance still require a client attempt.

The owned MAM config is `.gate/cs2-data/game/csgo/cfg/multiaddonmanager/multiaddonmanager.cfg`. **Do not repeat the server-side mount setup below during the human session.** It was attempted after the soak and rolled back after a startup failure and map-reload crash; see [the incident record](mam-startup-incident.md). The currently restored settings are:

```cfg
mm_extra_addons ""
mm_client_extra_addons "3790153369"
```

The [MultiAddonManager documentation](https://github.com/Source2ZE/MultiAddonManager/blob/main/README.md) distinguishes the server download/mount field from the client-only field. The latter does not mount the addon server-side, and the runtime's UI startup check reads `mm_extra_addons`. Client delivery and HUD rendering therefore require direct observation; the server startup banner alone cannot establish them.

Recovery restarted only the owned container, verified all 19 plugins active and both fixture commands available, then set and queried `sv_lan 0` for the temporary human test. Keep this change runtime-only; the container's next boot restores LAN mode. Do not copy credentials from another server. Anonymous direct connection is an attempt, not a proven guarantee; if Steam requires a token, use a fresh token dedicated to this test server.

The user connects through the CS2 developer console:

```text
connect nebula.gkh.dev:27016
```

Allow MAM's initial addon-delivery reconnect to finish before collecting lifecycle evidence.

## Connection generation and vote rail

1. Through RCON, run `status` and `sm_clive_status`. Require a fully active human, a nonzero Steam ID and `bot=false`. Record zero-based slot S and user ID U1.
2. Run `sm_clive_arm S`, then `sm_vote "HUD generation proof?" Yes No`.
3. Have the user verify that the right-side rail renders, movement/shooting/crosshair remain usable, and Tab captures the mouse for the vote.
4. The user disconnects without voting while the rail is active. Wait for the vote's configured duration to expire, then reconnect the same account.
5. Require the same slot S and Steam ID, with a different user ID U2. A different slot does not prove slot reuse; repeat a controlled reconnect if needed. Verify the disconnect snapshot reports `mutationValid=false`.
6. Before sending a new vote, the user verifies no old rail or cursor capture remains, movement/shooting works, and Tab opens the scoreboard.
7. Run `sm_clive_assert_stale S`. Require PASS, false stale command/fakeCommand returns, replacement validity and unchanged replacement voice state. The user must observe no stale chat/console marker and no stale kick.
8. Run `sm_clive_assert_fresh S`. Require voice toggled/restored, accepted command/fakeCommand and a valid fresh connection. The user confirms fresh chat and both fresh console messages; the fixture's PASS label alone does not prove visual delivery.
9. Send the identical vote again. Verify the same-value rail repaints, Tab captures input, clicking Yes updates/reveals the count, and Tab returns to the scoreboard after casting.

Record user observations separately from automated server assertions. Missing or blank HUD is not a passing result; verify the published workshop content and client delivery before diagnosing runtime behavior.

## Actual viewer-dependent hook execution

The soak registers and tears down hooks but does not prove viewer callbacks. Build the existing `tools/s2bench` fixture using `node packages/sdk/dist/cli.js build tools/s2bench`, temporarily install it after the soak, and verify its activation.

With the human fully signed on:

```text
sm_s2bench_hookload 1
sm_s2bench_hookstats
```

Record the initial callback and snapshot counters, wait five seconds, and query `sm_s2bench_hookstats` again. Require one installed hook and positive callback and shim snapshot deltas. Zero cleared bits is expected because this callback returns Continue.

Finally run `sm_s2bench_hookreset`, remove the temporary fixture and verify the original 18 plugins remain active. Restore `sv_lan 1` after the human session. Preserve configuration backups and record any intentional persistent test-server configuration change.

## Limits

This bounded session does not establish every authentication mode, a full map-transition matrix, or hook filtering semantics. The pre-existing unavailable EndTouch descriptor remains a separate gamedata limitation. Any unperformed item remains pending in the final acceptance report.
