import { publish } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  publish("@demo/derived-self", {
    ping: () => 1,
  });
}
