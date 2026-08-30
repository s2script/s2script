import type { OtherName } from "../api";
import { publish } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const impl: OtherName = {
    pong(): boolean {
      return true;
    },
  };
  publish("@demo/other-name", impl);
}
