import type { Api } from "../api";
import { publish } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const impl: Api = {
    nominate(_map: string): void {},
  };
  publish("@fixture/ik-mapchooser", impl);
}
