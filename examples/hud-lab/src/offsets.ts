/** Live-schema HUD field offsets. Missing fields stop this diagnostic plugin before writes. */
declare const __s2_schema_offset: (className: string, field: string) => number;

function field(className: string, name: string): number {
  const value = __s2_schema_offset(className, name);
  if (!Number.isInteger(value) || value < 0) throw new Error(`HUD schema unavailable: ${className}.${name}`);
  return value;
}

export const LAYOUT = {
  strLayout: field("CCSCustomHudLayout", "m_strLayout"),
  vecPlayerLayoutStates: field("CCSCustomHudLayout", "m_vecPlayerLayoutStates"),
  globalLayoutState: field("CCSCustomHudLayout", "m_globalLayoutState"),
  vecPanelIds: field("CCSCustomHudLayout", "m_vecPanelIds"),
  vecClassNames: field("CCSCustomHudLayout", "m_vecClassNames"),
  vecDialogVariableNames: field("CCSCustomHudLayout", "m_vecDialogVariableNames"),
} as const;

export const STATE = {
  inputCaptureEnabled: field("CCSCustomHudLayoutState", "m_bInputCaptureEnabled"),
  vecHasClasses: field("CCSCustomHudLayoutState", "m_vecHasClasses"),
  vecDialogVariableStrings: field("CCSCustomHudLayoutState", "m_vecDialogVariableStrings"),
  playerSlot: field("CCSCustomHudLayoutState", "m_playerSlot"),
} as const;

/** The global state immediately precedes panel IDs; this span is its array stride.
 * Verified against the engine's per-player setters on build 2000908 (408 bytes). */
export const STATE_SIZE = LAYOUT.vecPanelIds - LAYOUT.globalLayoutState;
if (STATE_SIZE <= STATE.playerSlot || STATE_SIZE <= STATE.vecDialogVariableStrings + 24) {
  throw new Error("HUD schema has an invalid embedded-state span");
}
/** Readable diagnostic span through the last known vector, not the entity's total allocation. */
export const LAYOUT_SIZE = LAYOUT.vecDialogVariableNames + 24;

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
