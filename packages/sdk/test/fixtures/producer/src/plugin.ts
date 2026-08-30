import type { Greeter } from "../api";
import { publish } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const impl: Greeter = { greet: (n: number) => `hi ${n}` };
  publish("@demo/greeter", impl);
}
