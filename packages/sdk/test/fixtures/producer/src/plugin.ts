import { plugin } from "@s2script/sdk/plugin";
import type { Greeter } from "../api";

export default plugin((ctx) => {
  const impl: Greeter = { greet: (n: number) => `hi ${n}` };
  ctx.publish("@demo/greeter", impl);
});
