# Engine function v2: direct scalar call with PRE and POST

This ordinary plugin declares `CBasePlayerPawn_CommitSuicide(this, bool explode, bool force) -> void`
in `gamedata/functions.jsonc`. `s2s build` discovers that file, generates local `Engine.function`
types, and derives `engine:calls` and `engine:hooks` permissions in the archive manifest. There is
no handwritten `s2script.gamedata` field, permission list, or generated-file include.

The signature is for the retained `libserver.so` from installed CS2 build 25472966, SHA-256
`23373cfdb96dee1f2da858274c03346c952faff2942b5e7923525e187366e87f`. Its unique
executable match is VA `0x1867a90`, file offset `0x1866a90`. The complete recipe and checked
entry prologue are in the descriptor, along with base-class vtable membership. Re-audit the
declaration against each installed game binary after an update. The separate `staleEntryProbe`
has a deliberately wrong entry validator; its failure should leave `commitSuicide` available.

Build with `npm run build` here. Install `dist/_demo_engine-function.s2sp` on a test server and
grant this plugin `engine:calls` and `engine:hooks` in the operator permission configuration.
`sm_ef_status` prints each binding's immutable structured status, including its canonical id,
reason, hook observation state, and provenance hashes/override receipts. At load, the example
registers a PRE handler that sets `force`, disposes that subscription, registers a replacement,
and registers a POST observer. It prints each handle's registration status.

From the **server console only**, run `sm_ef_suicide_bot <bot-slot>`. The command rejects a missing,
human, disconnected, unspawned, or already dead target. It checks the real bot's pawn health before
the direct call and reads the engine-facing pawn state afterward, reporting `alive true -> false`
only if that change occurred. Merely building or registering hooks never invokes the lethal call.
The SDK build validates authoring and the scalar ABI contract; only a later live run can establish
the actual alive-to-dead effect.

The direct signature targets the `CBasePlayerPawn` base body. `CCSPlayerPawn` has an overriding
forwarder that writes an extra pawn byte before calling that body; this direct call bypasses that
write. The example makes no claim to reproduce the override's full behavior.
