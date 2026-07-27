import { plugin } from "@s2script/sdk/plugin";
import type { Greeter } from "../api";

export default plugin((ctx) => {
  const impl: Greeter = {
    greet(name: string): string {
      return `hello ${name}`;
    },
  };
  ctx.publish("@fixture/ws-greeter", impl);
});
