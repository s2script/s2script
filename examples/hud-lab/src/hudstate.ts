/**
 * Reading and writing CCSCustomHudLayout state through EntityRef's raw-offset surface.
 *
 * This is the half of the new HUD API that needs NO engine signature and NO codegen: the global
 * layout state is embedded by value at a known offset, so its `m_bInputCaptureEnabled` bool is a
 * plain `writeBool` + `notifyStateChanged`. Everything that has to GROW a networked CUtlVector
 * (`SetHasClass`, `SetDialogVariableString`) is not reachable this way and lives in `tierb.ts`.
 *
 * Every write here is gated on {@link probeLayout} first — see `offsets.ts` for why that matters.
 */
import type { EntityRef } from "@s2script/sdk/entity";
import { LAYOUT, STATE, VEC, LAYOUT_SIZE, STATE_SIZE, globalStateField } from "./offsets";

/** Highest element count we will believe from a raw vector read. A real layout declares tens of
 *  panels, not thousands; anything larger means we are reading the wrong bytes. */
const MAX_PLAUSIBLE_COUNT = 4096;
/** Lowest address we will accept as a real heap/rodata pointer (below this is a small integer). */
const MIN_USERSPACE_PTR = 0x10000n;
/** x86-64 Linux userspace ceiling — canonical lower half. */
const MAX_USERSPACE_PTR = 0x800000000000n;

/** The outcome of a plausibility probe against one candidate layout entity. */
export interface ProbeResult {
  /** True only if every check below passed. Gate every WRITE on this. */
  ok: boolean;
  /** Human-readable reasons the probe failed, in the order checked. Empty when `ok`. */
  reasons: string[];
}

function plausibleCount(n: number | null): boolean {
  return n !== null && n >= 0 && n <= MAX_PLAUSIBLE_COUNT;
}

/**
 * Cheap sanity gate for the borrowed offsets in `offsets.ts`.
 *
 * It cannot prove the offsets are correct — only that the bytes at them are not obviously garbage.
 * A green probe means "safe to try a write", never "verified". The checks are deliberately weak in
 * the direction of false-negatives: every one of them would also pass on a correctly-mapped entity
 * whose vectors happen to be empty, which is the expected state on a map with no HUD layout.
 */
export function probeLayout(ref: EntityRef): ProbeResult {
  const reasons: string[] = [];

  if (!ref.isValid()) return { ok: false, reasons: ["entity ref is stale"] };

  // The last byte of the struct must be readable, or the entity is smaller than the dump claims
  // and every offset in offsets.ts is describing some other class.
  if (ref.readUInt8(LAYOUT_SIZE - 1) === null) {
    reasons.push(`cannot read entity+${LAYOUT_SIZE - 1} (entity smaller than the dumped ${LAYOUT_SIZE} bytes?)`);
  }

  // The global state's bool must at least be readable.
  if (ref.readBool(globalStateField(STATE.inputCaptureEnabled)) === null) {
    reasons.push(`cannot read m_bInputCaptureEnabled at +${globalStateField(STATE.inputCaptureEnabled)}`);
  }

  // Every declared CUtlVector count must be a small non-negative number.
  const vectors: Array<[string, number]> = [
    ["m_vecPanelIds", LAYOUT.vecPanelIds],
    ["m_vecClassNames", LAYOUT.vecClassNames],
    ["m_vecDialogVariableNames", LAYOUT.vecDialogVariableNames],
    ["m_globalLayoutState.m_vecHasClasses", globalStateField(STATE.vecHasClasses)],
    ["m_globalLayoutState.m_vecDialogVariableStrings", globalStateField(STATE.vecDialogVariableStrings)],
  ];
  for (const [name, off] of vectors) {
    const n = ref.readInt32(off + VEC.count);
    if (!plausibleCount(n)) reasons.push(`${name} count at +${off} reads ${n} (implausible)`);
  }

  // m_strLayout is a pointer: either null, or inside canonical userspace.
  const ptr = ref.readUInt64(LAYOUT.strLayout);
  if (ptr === null) {
    reasons.push(`cannot read m_strLayout at +${LAYOUT.strLayout}`);
  } else if (ptr !== 0n && (ptr < MIN_USERSPACE_PTR || ptr >= MAX_USERSPACE_PTR)) {
    reasons.push(`m_strLayout at +${LAYOUT.strLayout} is 0x${ptr.toString(16)} (not a plausible pointer)`);
  }

  // The global state's player slot should be the "no particular player" sentinel or a real slot.
  const slot = ref.readInt32(globalStateField(STATE.playerSlot));
  if (slot === null || slot < -1 || slot > 64) {
    reasons.push(`m_globalLayoutState.m_playerSlot at +${globalStateField(STATE.playerSlot)} reads ${slot} (expected -1..64)`);
  }

  return { ok: reasons.length === 0, reasons };
}

/** Everything this plugin can learn about a layout entity without an engine call. */
export interface LayoutInfo {
  index: number;
  targetname: string | null;
  /** Whether `m_strLayout` holds a non-null pointer. The NAME itself is not reachable from JS. */
  hasLayoutString: boolean;
  /** Raw `m_strLayout` pointer, hex — useful when eyeballing a probe failure. */
  layoutPtr: string;
  /** Declared panel ids in the layout asset. 0 means the layout registered nothing (or none loaded). */
  panelIds: number | null;
  /** Declared class names in the layout asset. */
  classNames: number | null;
  /** Declared dialog-variable names in the layout asset. */
  dialogVariableNames: number | null;
  /** Per-player state entries. Read from the embedded-network-var's leading count — UNVERIFIED
   *  container layout, so treat a surprising number here as a probe failure, not a finding. */
  playerLayoutStates: number | null;
  /** Global `m_bInputCaptureEnabled`. */
  inputCapture: boolean | null;
  /** Class entries applied on the global state (grown only by the Tier-B engine call). */
  globalHasClasses: number | null;
  /** Dialog-variable entries applied on the global state (grown only by the Tier-B engine call). */
  globalDialogVars: number | null;
  probe: ProbeResult;
}

/** Read every diagnostic this plugin can reach for one layout entity. */
export function readLayoutInfo(ref: EntityRef): LayoutInfo {
  const ptr = ref.readUInt64(LAYOUT.strLayout);
  return {
    index: ref.index,
    targetname: ref.name,
    hasLayoutString: ptr !== null && ptr !== 0n,
    layoutPtr: ptr === null ? "unreadable" : `0x${ptr.toString(16)}`,
    panelIds: ref.readInt32(LAYOUT.vecPanelIds + VEC.count),
    classNames: ref.readInt32(LAYOUT.vecClassNames + VEC.count),
    dialogVariableNames: ref.readInt32(LAYOUT.vecDialogVariableNames + VEC.count),
    playerLayoutStates: ref.readInt32(LAYOUT.vecPlayerLayoutStates + VEC.count),
    inputCapture: ref.readBool(globalStateField(STATE.inputCaptureEnabled)),
    globalHasClasses: ref.readInt32(globalStateField(STATE.vecHasClasses) + VEC.count),
    globalDialogVars: ref.readInt32(globalStateField(STATE.vecDialogVariableStrings) + VEC.count),
    probe: probeLayout(ref),
  };
}

/** Read the global `m_bInputCaptureEnabled` — cs_script's `CustomHudLayout.IsInputCaptureEnabled`. */
export function isInputCaptureEnabled(ref: EntityRef): boolean | null {
  return ref.readBool(globalStateField(STATE.inputCaptureEnabled));
}

/**
 * Write the global `m_bInputCaptureEnabled` — cs_script's `CustomHudLayout.SetInputCaptureEnabled`.
 *
 * `notifyStateChanged` is REQUIRED: the raw write reaches the server's copy of the field, but
 * without the network-state notification the engine never marks it dirty and the change is not
 * replicated to any client. Returns a reason string on refusal, or null on success.
 */
export function setInputCaptureEnabled(ref: EntityRef, on: boolean): string | null {
  const probe = probeLayout(ref);
  if (!probe.ok) return `probe failed: ${probe.reasons.join("; ")}`;

  const off = globalStateField(STATE.inputCaptureEnabled);
  if (!ref.writeBool(off, on)) return "writeBool failed (stale ref)";
  ref.notifyStateChanged(off);

  // Read back rather than trusting the write: a wrong offset can land in padding and report success.
  const after = ref.readBool(off);
  if (after !== on) return `wrote ${on} at +${off} but read back ${after}`;
  return null;
}

/**
 * A raw byte window around the HUD fields, for verifying `offsets.ts` against a schema dump by eye.
 *
 * This is the treadmill tool: after a CS2 update, run it, compare the window against a fresh dump,
 * and you will see an offset shift before it turns into a bad write.
 */
export function dumpWindow(ref: EntityRef, start: number, length: number): string[] {
  const lines: string[] = [];
  for (let row = start; row < start + length; row += 16) {
    const bytes: string[] = [];
    for (let i = 0; i < 16; i++) {
      const b = ref.readUInt8(row + i);
      bytes.push(b === null ? "??" : b.toString(16).padStart(2, "0"));
    }
    lines.push(`+${row.toString().padStart(4, " ")}  ${bytes.join(" ")}`);
  }
  return lines;
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// PER-PLAYER LAYOUT STATES — driving a map-authored panel with no engine call at all.
//
// This is the path around the CUtlString ABI wall. The setters are unreachable because ADDING a
// class entry means interning a string and growing a networked vector. But a map whose cs_script
// already applied a class has the entry ALREADY THERE, and `HUDPanelHasClass_t` is:
//
//     { uint16 m_nPanelIdIndex @0, uint16 m_nClassNameIndex @2, EHudPanelClassStatus_t @4 }
//
// — 8 bytes, and the status is a plain int32. Flipping it between DoesNotHaveClass(0) and
// HasClass(1) shows/hides that panel for that player, and that is a raw write.
//
// The reach is two pointer hops, both of which EntityRef already does in-core:
//     entity +1944                       -> m_vecPlayerLayoutStates data     (P)
//     P + slot*416 + 56                  -> that state's m_vecHasClasses     (count @+0, data @+8)
//     P + slot*416 + 64                  -> the entries array                (Q)
//     Q + i*8 + 4                        -> entry i's status
// ─────────────────────────────────────────────────────────────────────────────────────────────

/** Data pointer of `m_vecPlayerLayoutStates` (count is at +0, elements at +8). */
const STATES_DATA = LAYOUT.vecPlayerLayoutStates + VEC.elements;
/** `m_vecHasClasses` within a state; its data pointer sits 8 further on. */
const HAS_CLASSES = STATE.vecHasClasses;
/** sizeof(HUDPanelHasClass_t) — two uint16s then an int32. */
const HAS_CLASS_STRIDE = 8;
/** Offset of `m_eClassStatus` within a HUDPanelHasClass_t. */
const CLASS_STATUS = 4;

/** Byte offset of slot `slot`'s state inside the states array. */
function stateBase(slot: number): number {
  return slot * STATE_SIZE;
}

/** One applied class entry, as read through the chain. */
export interface ClassEntry {
  index: number;
  panelIdIndex: number | null;
  classNameIndex: number | null;
  /** EHudPanelClassStatus_t: -1 Undefined, 0 DoesNotHaveClass, 1 HasClass. */
  status: number | null;
}

/** How many per-player states the layout allocated (one per player slot). */
export function playerStateCount(ref: EntityRef): number | null {
  return ref.readInt32(LAYOUT.vecPlayerLayoutStates + VEC.count);
}

/** Class entries applied to `slot`'s state. Empty when the map's script has applied none. */
export function readPlayerClasses(ref: EntityRef, slot: number): ClassEntry[] {
  const count = ref.readInt32Via([STATES_DATA], stateBase(slot) + HAS_CLASSES + VEC.count);
  if (count === null || count <= 0 || count > 64) return [];
  const entries: ClassEntry[] = [];
  const arrayPtrOff = stateBase(slot) + HAS_CLASSES + VEC.elements;
  for (let i = 0; i < count; i++) {
    entries.push({
      index: i,
      panelIdIndex: ref.readUInt16Via([STATES_DATA, arrayPtrOff], i * HAS_CLASS_STRIDE + 0),
      classNameIndex: ref.readUInt16Via([STATES_DATA, arrayPtrOff], i * HAS_CLASS_STRIDE + 2),
      status: ref.readInt32Via([STATES_DATA, arrayPtrOff], i * HAS_CLASS_STRIDE + CLASS_STATUS),
    });
  }
  return entries;
}

/**
 * Write one entry's class status for one player — the show/hide switch.
 *
 * `notifyStateChanged` is aimed at `m_vecPlayerLayoutStates` rather than the deep address we wrote:
 * the network-state notification takes an offset into THIS entity, and the vector is the networked
 * member that owns everything below it. Without it the server's copy changes and no client is told.
 *
 * Returns null on success, or the reason it refused.
 */
export function setPlayerClassStatus(
  ref: EntityRef, slot: number, entryIndex: number, status: number,
): string | null {
  const probe = probeLayout(ref);
  if (!probe.ok) return `probe failed: ${probe.reasons.join("; ")}`;

  const entries = readPlayerClasses(ref, slot);
  if (entries.length === 0) {
    return `slot ${slot} has no class entries — this map's script has not applied one, and ADDING ` +
      `one needs the CUtlString setter (see the gamedata note)`;
  }
  const entry = entries[entryIndex];
  if (!entry) return `entry ${entryIndex} out of range (slot ${slot} has ${entries.length})`;

  const arrayPtrOff = stateBase(slot) + HAS_CLASSES + VEC.elements;
  const ok = ref.writeInt32Via(
    [STATES_DATA, arrayPtrOff], entryIndex * HAS_CLASS_STRIDE + CLASS_STATUS, status,
  );
  if (!ok) return "writeInt32Via failed (stale ref or a null hop)";
  ref.notifyStateChanged(LAYOUT.vecPlayerLayoutStates);

  const after = ref.readInt32Via([STATES_DATA, arrayPtrOff], entryIndex * HAS_CLASS_STRIDE + CLASS_STATUS);
  if (after !== status) return `wrote ${status} but read back ${after}`;
  return null;
}

/**
 * Per-player `m_bInputCaptureEnabled` — the mouse-cursor switch.
 *
 * The GLOBAL state's copy (entity+2088, what `setInputCaptureEnabled` writes) is not what gates a
 * given player's cursor: each per-player state carries its own flag at `state+48`, and that is the
 * one the client reads when deciding whether to capture the mouse for this layout. A panel toggled
 * visible with input capture still false renders but cannot be clicked — which is exactly the
 * "the panel came up but Dismiss did nothing" symptom.
 *
 * Same two-hop reach as the class entries, so it needs no engine call either.
 */
export function readPlayerInputCapture(ref: EntityRef, slot: number): boolean | null {
  return ref.readBoolVia([STATES_DATA], stateBase(slot) + STATE.inputCaptureEnabled);
}

/** Write one player's input-capture flag. Returns null on success, or the reason it refused. */
export function setPlayerInputCapture(ref: EntityRef, slot: number, on: boolean): string | null {
  const probe = probeLayout(ref);
  if (!probe.ok) return `probe failed: ${probe.reasons.join("; ")}`;
  if (!ref.writeBoolVia([STATES_DATA], stateBase(slot) + STATE.inputCaptureEnabled, on)) {
    return "writeBoolVia failed (stale ref or a null hop)";
  }
  ref.notifyStateChanged(LAYOUT.vecPlayerLayoutStates);
  const after = readPlayerInputCapture(ref, slot);
  if (after !== on) return `wrote ${on} but read back ${after}`;
  return null;
}
