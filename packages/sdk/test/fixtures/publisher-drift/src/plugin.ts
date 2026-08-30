import { publish } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  publish("@demo/other-name", {
    ping: () => 1,
  });
}
