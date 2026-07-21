import { plugin } from "@s2script/sdk/plugin";

const NAME = ["@demo", "dynamic"].join("/");

export default plugin((ctx) => {
  ctx.publish(NAME, {
    ping: () => 1,
  });
});
