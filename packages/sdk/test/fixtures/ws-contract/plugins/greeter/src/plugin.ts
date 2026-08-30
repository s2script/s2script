import type { Greeter } from "../api";
import { publish } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const impl: Greeter = {
    greet(name: string): string {
      return `hello ${name}`;
    },
  };
  publish("@fixture/ws-greeter", impl);
}
