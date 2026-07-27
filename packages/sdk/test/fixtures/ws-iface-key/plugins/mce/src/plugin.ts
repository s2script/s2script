import { plugin } from "@s2script/sdk/plugin";
import type { Api } from "../api";

export default plugin((ctx) => {
  const impl: Api = {
    nominate(_map: string): void {},
  };
  ctx.publish("@fixture/ik-mapchooser", impl);
});
