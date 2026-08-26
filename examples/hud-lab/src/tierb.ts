/**
 * The HUD setters, re-armed on the `utlstring` arg kind.
 *
 * HISTORY, because it is the whole reason this module reads the way it does: these were first
 * declared with `"string"`, which marshals a raw `char*`. The callee dereferences its argument as a
 * `CUtlString*` (`mov rcx,QWORD PTR [rsi]` at va 0x130ae00), so the engine read the first eight
 * ASCII bytes of the text as a pointer and followed it — segfaulting a live server. `utlstring`
 * (added to core + shim for this) marshals the same buffer and passes the address of a call-scoped
 * `{ char* }` temporary instead, which is exactly what a `CUtlString` is.
 *
 * Only the two `ForPlayer` variants exist: their addresses are structurally confirmed. The global
 * variants are not located yet, so anything "for everyone" is a loop over slots here.
 */
import { Engine } from "@s2script/sdk/unsafe";
import type { EngineCalls } from "@s2script/sdk/unsafe";
import type { EntityRef } from "@s2script/sdk/entity";
import { HudPanelClassStatus } from "./offsets";

export const CALL_NAMES = ["setHasClassForPlayer", "setDialogVariableStringForPlayer"] as const;
export type CallName = (typeof CALL_NAMES)[number];
export type ResolvedCalls = { [K in CallName]: EngineCalls[K] | null };

/** Resolve once at load — a descriptor that failed a load-time gate is null forever. */
export function resolveAll(): ResolvedCalls {
  return {
    setHasClassForPlayer: Engine.call("setHasClassForPlayer"),
    setDialogVariableStringForPlayer: Engine.call("setDialogVariableStringForPlayer"),
  };
}

export function statusLines(calls: ResolvedCalls): string[] {
  const lines = CALL_NAMES.map((name) => {
    const armed = calls[name] !== null;
    return `  ${armed ? "ARMED  " : "unavail"} ${name.padEnd(34)} ${Engine.status(name)}`;
  });
  lines.push("  (global SetHasClass / SetDialogVariableString: addresses not located — use a target)");
  return lines;
}

/** Turn a user token into EHudPanelClassStatus_t. */
export function parseClassStatus(token: string): number {
  if (token === "-1" || token.toLowerCase() === "undefined") return HudPanelClassStatus.Undefined;
  const on = token === "1" || token.toLowerCase() === "true" || token.toLowerCase() === "on";
  return on ? HudPanelClassStatus.HasClass : HudPanelClassStatus.DoesNotHaveClass;
}

/** Apply/remove a class on a panel for one player. Returns null on success, or a reason. */
export function setHasClass(
  calls: ResolvedCalls, layout: EntityRef, slot: number, panelId: string, className: string, status: number,
): string | null {
  const fn = calls.setHasClassForPlayer;
  if (!fn) return `unavailable: ${Engine.status("setHasClassForPlayer")}`;
  if (slot < 0) return "needs a player slot (the global variant is not located)";
  fn(layout, slot, panelId, className, status);
  return null;
}

/** Set a dialog variable's string for one player. Returns null on success, or a reason. */
export function setDialogVariable(
  calls: ResolvedCalls, layout: EntityRef, slot: number, panelId: string, variableName: string, value: string,
): string | null {
  const fn = calls.setDialogVariableStringForPlayer;
  if (!fn) return `unavailable: ${Engine.status("setDialogVariableStringForPlayer")}`;
  if (slot < 0) return "needs a player slot (the global variant is not located)";
  fn(layout, slot, panelId, variableName, value);
  return null;
}
