import type { Contract } from "../api";
import { publish } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const impl: Contract = {
    ping(): boolean {
      return true;
    },
  };
  // B1 reordered the build: the typecheck gate now runs FIRST (it feeds the publishes/use
  // derivation), so the old "range rejected BEFORE typecheck" fail-fast ordering no longer exists.
  // This fixture typechecks clean and publishes exactly the authored interface name, so the RANGE
  // "^1.0.0" is what the build rejects — surfaced as "is a RANGE", never as "typecheck failed".
  publish("@community/contract", impl);
}
