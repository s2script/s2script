import { publish } from "@s2script/sdk/plugin";

const NAME = ["@demo", "dynamic"].join("/");

export function OnPluginStart(): void {
  publish(NAME, {
    ping: () => 1,
  });
}
