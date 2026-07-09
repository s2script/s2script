# Slice: game-event catalog — parity with CounterStrikeSharp

**Date:** 2026-07-09
**Status:** design approved (scope = ALL events, user-chosen) — executing inline
**Related:** Slice 5D.1 (the eventgen codegen + the 12 seed catalog), the CSSharp-parity analysis.

## Goal

Expand `games/cs2/gamedata/event-catalog.json` from the 12 hand-seeded events to the **full ~450-event set
CounterStrikeSharp ships**, and regenerate `packages/cs2/events.generated.d.ts` — bringing the typed
game-event IntelliSense overlay to CSSharp parity. **IntelliSense-only: zero runtime/engine/core change**
(the runtime is already a generic bus that dispatches any event; the catalog only types `Events.on<K>`).

## Why CSSharp as the source

CS2 buries the real event definitions in the VPK (there's no live dump — the reason we only had 12
hand-seeds). CSSharp's `Generated/GameEvents/Event*.g.cs` are the accurate, **maintained** field
definitions, and they're cleanly machine-parseable:

```csharp
[EventName("player_death")]
public class EventPlayerDeath : GameEvent {
    public bool Assistedflash { get => Get<bool>("assistedflash"); ... }
    public CCSPlayerController? Attacker { get => GetPlayer("attacker"); ... }
    public float Distance { get => Get<float>("distance"); ... }
    public int DmgArmor { get => Get<int>("dmg_armor"); ... }
}
```

Events are game **content** (stable across binary patches), so borrowing them carries none of the
offset/signature treadmill risk — and it's IntelliSense-only, so even a stale field just means one
untyped `getInt` call, never a crash.

## Scope

**ALL events CSSharp ships (~450), verbatim** (user decision). No curation/filtering — completeness over
autocomplete-noise (there is no runtime cost; the `GameEvents` type just gets larger). The 12 existing
seed events are subsumed (and cross-checked for agreement).

## Mechanism

A re-runnable offline dev script (`scripts/extract-event-catalog.mjs`, Node, no deps) that:
1. Reads the CSSharp `Generated/GameEvents/*.g.cs` files (from a shallow clone at a **pinned ref**,
   recorded in the script + this spec for reproducibility).
2. Per file: extract `[EventName("<name>")]` and every `get => Get<T>("field")` / `GetPlayer("field")`
   / `GetPlayerPawn("field")` / `GetEntity("field")` accessor.
3. Emit `{ "<event>": { "<field>": "<type>", ... }, ... }`, keys sorted, to `event-catalog.json`.

**Type mapping** (CSSharp accessor → our catalog type → eventgen getter):
| CSSharp | catalog type | getter |
|---|---|---|
| `Get<bool>` | `bool` | `getBool` |
| `Get<int>` | `int` | `getInt` |
| `Get<float>` | `float` | `getFloat` |
| `Get<string>` | `string` | `getString` |
| `Get<long>` / `Get<ulong>` / `Get<uint>` | `uint64` | `getUint64` (→ decimal string) |
| `GetPlayer` / `GetPlayerPawn` | `player` | `getPlayerSlot` |
| `GetEntity` | `int` | `getInt` (entindex) |

Then `s2script gen-events` regenerates `events.generated.d.ts` (deterministic, alphabetically sorted).

## Edge cases (must handle)

- **Zero-field events** (e.g. `round_freeze_end`) → `{}` (already supported).
- **Unknown `Get<T>`** (a type outside the map above) → **skip that field with a logged warning**, never
  emit an unmapped type (would break the generated `.d.ts`).
- **`GetPlayerPawn`** → `player` (still a slot-resolvable actor; our getter set has no separate pawn type).
- **Multi-line property blocks** → the regex keys on the `Get<...>("...")` / `GetPlayer("...")` call, not
  brace matching, so formatting variance is irrelevant.
- **Field name = a JS/TS reserved word or with special chars** → passes through as a quoted string key /
  quoted getter key literal (the emitter already quotes keys), so no identifier collision.

## Non-goals (do NOT build)

- No new getter types / no `float`↔`int` re-inference beyond the map above.
- No auto-resolving convenience (e.g. `ev.player` → a `Player`) — keep the existing getter API.
- No runtime, core, shim, or engine change; no sniper build.
- No per-event documentation strings (CSSharp doesn't ship them either).

## Verification

1. `scripts/check-events-generated.sh` (the freshness gate: regenerate + `git diff --exit-code`) — green.
2. The generated `events.generated.d.ts` compiles under the plugin typecheck gate
   (`scripts/check-plugins-typecheck.sh`) — green.
3. Spot-check: the 12 prior seed events still resolve (e.g. `player_death` keeps its 9 fields; agreement
   with the hand-seed, or a logged diff explaining any CSSharp difference), and a sample of new events
   (`bomb_exploded`, `weapon_reload`, `hostage_rescued`, `player_blind`) type correctly.
4. Report the final event count + any fields skipped (unknown type) so nothing is silently dropped.

## Provenance / treadmill

The catalog (`event-catalog.json`) + the extraction script are committed; the CSSharp source ref is pinned
in the script. Re-run the script on a major CS2 content update that changes event fields (rare). No live
CS2 gate needed (build-time, IntelliSense-only, no `.so` change).
