/**
 * The CCSCustomHudLayout / CCSCustomHudLayoutState memory map, in one place.
 *
 * PROVENANCE — read this before trusting a number below.
 * ------------------------------------------------------
 * These offsets were transcribed from a schema dump of the CS2 build that introduced
 * `custom_hud_layout`. They are NOT self-resolved by this repo's own tooling:
 * `games/cs2/gamedata/schema-catalog.json` still predates the update and contains no
 * `CCSCustomHudLayout` entry, so `tools/schema-dump` (itself a plugin — it needs a RUNNING server)
 * has not confirmed them.
 *
 * TWO OF THEM ARE NOW CONFIRMED against build 24916958's libserver.so by disassembly, as a side
 * effect of resolving the Tier-B signatures in gamedata/hud-lab.gamedata.jsonc:
 *
 *   1936  m_vecPlayerLayoutStates  — `CCSCustomHudLayout::SetHasClassForPlayer` (va 0x1312b30)
 *                                    opens with `cmp esi,[rdi+0x790]`, bounds-checking its player
 *                                    slot against this vector's count. 0x790 == 1936.
 *   2040  m_globalLayoutState      — the global setter variants reach their state with
 *                                    `lea rdi,[reg+0x7f8]`. 0x7f8 == 2040.
 *   2480  m_vecClassNames          — interned via `mov ebx,[r12+0x9b0]` / `mov [r12+0x9b0],eax`
 *                                    in the class-setting path. 0x9b0 == 2480.
 *
 * The rest — notably the STATE-relative offsets and the two remaining name-table vectors — are
 * still unconfirmed borrowed constants.
 *
 * That makes every constant here a BORROWED CONSTANT, which `docs/re-strategy.md` forbids relying
 * on unvalidated. The mitigation is `probeLayout()` below: a cheap load-time plausibility gate that
 * every write path in this plugin consults first. It cannot prove the offsets are right — only that
 * they are not obviously wrong — so treat a green probe as "safe to try", never as "verified".
 *
 * THE FIX, once the treadmill has run against the new build:
 *   1. `tools/schema-dump` against the updated libserver.so
 *   2. confirm these numbers, then add CCSCustomHudLayout to games/cs2/codegen-classes.json
 *   3. delete this file and use generated accessors instead
 *
 * A field-offset change must never require a code change — that is exactly why this file exists as
 * data and nothing below it hardcodes a number.
 */

/** Total `sizeof(CCSCustomHudLayout)` per the dump. Used as the probe's bounds check. */
export const LAYOUT_SIZE = 2768;
/** Total `sizeof(CCSCustomHudLayoutState)` per the dump. */
export const STATE_SIZE = 416;

/**
 * Field offsets on the CCSCustomHudLayout ENTITY itself.
 *
 * `CBaseEntity` is CCSCustomHudLayout's only declared base at offset 0, so these are absolute
 * offsets from the entity pointer — no base adjustment.
 */
export const LAYOUT = {
  /** `CUtlSymbolLarge m_strLayout` — an 8-byte POINTER to the layout name, not inline chars.
   *  There is no `readStringVia` on EntityRef, so this plugin reads the pointer VALUE only:
   *  non-zero means a layout name is set, zero means none. The text is not reachable from JS. */
  strLayout: 1928,
  /** `CUtlVectorEmbeddedNetworkVar<CCSCustomHudLayoutState> m_vecPlayerLayoutStates` (size 104).
   *  An embedded-network-var container, NOT a plain CUtlVector — its internal layout is unverified,
   *  so this plugin READS its leading count for diagnostics and never writes into it. */
  vecPlayerLayoutStates: 1936,
  /** `CCSCustomHudLayoutState m_globalLayoutState` — embedded BY VALUE (size 416), so state field
   *  offsets add directly to this. The only state this plugin writes. */
  globalLayoutState: 2040,
  /** `CNetworkUtlVectorBase<CUtlString> m_vecPanelIds` (size 24). */
  vecPanelIds: 2456,
  /** `CNetworkUtlVectorBase<CUtlString> m_vecClassNames` (size 24). */
  vecClassNames: 2480,
  /** `CNetworkUtlVectorBase<CUtlString> m_vecDialogVariableNames` (size 24). */
  vecDialogVariableNames: 2504,
} as const;

/**
 * Field offsets WITHIN a CCSCustomHudLayoutState. Add to a state's own base — for the global state
 * that is `LAYOUT.globalLayoutState`; see {@link globalStateField}.
 */
export const STATE = {
  /** `bool m_bInputCaptureEnabled` — the one directly writable scalar in the whole HUD surface. */
  inputCaptureEnabled: 48,
  /** `CNetworkUtlVectorBase<HUDPanelHasClass_t> m_vecHasClasses` (size 24). Read-only here:
   *  growing a networked CUtlVector needs the engine's own setter (see the gamedata's Tier-B calls). */
  vecHasClasses: 56,
  /** `CNetworkUtlVectorBase<HUDPanelDialogVariableString_t> m_vecDialogVariableStrings` (size 24). */
  vecDialogVariableStrings: 152,
  /** `CPlayerSlot m_playerSlot` — which player this state belongs to (-1 on the global state). */
  playerSlot: 408,
} as const;

/** Absolute entity offset of a field inside the embedded GLOBAL layout state. */
export function globalStateField(stateOffset: number): number {
  return LAYOUT.globalLayoutState + stateOffset;
}

/**
 * CUtlVector element layout, as this repo already models it.
 *
 * `EntityRef.readHandleVector` documents the engine's container as `count@+0 / elements@+8`, and
 * `CNetworkUtlVectorBase`'s dumped size of 24 is consistent with that. Only the COUNT is read here —
 * the elements are `CUtlString`/struct pointers this plugin has no way to deref into text.
 */
export const VEC = { count: 0, elements: 8 } as const;

/** `EHudPanelClassStatus_t` (size 4) — the dumped enum, mirrored for the Tier-B class setter. */
export const HudPanelClassStatus = {
  Undefined: -1,
  DoesNotHaveClass: 0,
  HasClass: 1,
} as const;
export type HudPanelClassStatusValue =
  (typeof HudPanelClassStatus)[keyof typeof HudPanelClassStatus];
