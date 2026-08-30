import type { Publisher } from "../api";
import { publish } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const impl: Publisher = {
    ping(): boolean {
      return true;
    },
  };
  publish("@demo/publisher", impl);
}
