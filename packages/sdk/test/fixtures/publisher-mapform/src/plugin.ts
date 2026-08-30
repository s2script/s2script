import type { Contract } from "../api";
import { publish } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const impl: Contract = {
    ping(): boolean {
      return true;
    },
  };
  // Code publishes exactly the authored interface name, so reconciliation passes; the authored
  // "^1.0.0" RANGE is what the build rejects (a range needs the registry — spec §4.6, §10).
  publish("@community/contract", impl);
}
