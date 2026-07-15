# @s2script/usercmd

## 0.1.0

### Minor Changes

- b25b980: Per-tick user-command hook (SourceMod `OnPlayerRunCmd` parity).

  - `@s2script/usercmd`: `UserCmd.onRun(handler)` delivers a block-scoped `Cmd` for each processed player command — read **and** modify `forwardMove`/`sideMove`/`upMove` (normalized ±1), `impulse`, `buttons` (a `bigint` mask, crosses the native boundary as a real bigint), and `viewAngles` (`QAngle`); return a `HookResult >= Handled` to block the input. The runtime detours `CCSPlayer_MovementServices::ProcessUserCmd` (the batch entry) and reads the live `CSGOUserCmdPB` protobuf via reflection — the CS2 hook function, protobuf field offsets, and slot derivation (pawn → `m_hController` → controller index) live in the shim + gamedata, so the module stays engine-generic. Unblocks input-based movement styles (sideways/backwards/W-only).
