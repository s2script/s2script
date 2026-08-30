// Imports a GAME-PACKAGE subpath and a nested SDK subpath. Both resolve only via the `paths`
// entries in typecheck.ts; a miss is TS2307, never a silent `any` — which is the point of the test.
import type { EconService } from "@s2script/cs2/econ";
import type { WorkshopService } from "@s2script/sdk/contracts/workshop";
import { tryUse } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const econ = tryUse<EconService>("econ");
  const ws = tryUse<WorkshopService>("workshop");
  // Exercise the types so a resolution regression is a type error, not just an unused import.
  const applied: boolean = econ ? econ.applySkin(1) : false;
  void applied; void ws;
}
