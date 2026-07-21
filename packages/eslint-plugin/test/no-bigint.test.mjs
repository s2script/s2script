import * as nodeTest from "node:test";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { RuleTester } from "@typescript-eslint/rule-tester";
import { noBigintInInterfacePayloads } from "../src/rules/no-bigint-in-interface-payloads.ts";

RuleTester.afterAll = nodeTest.after;
RuleTester.describe = nodeTest.describe;
RuleTester.it = nodeTest.it;
RuleTester.itOnly = nodeTest.it;

const fixtures = join(dirname(fileURLToPath(import.meta.url)), "fixtures");

const ruleTester = new RuleTester({
  languageOptions: {
    parserOptions: {
      project: "./tsconfig.json",
      tsconfigRootDir: fixtures,
    },
  },
});

ruleTester.run("no-bigint-in-interface-payloads", noBigintInInterfacePayloads, {
  valid: [
    // Decimal-string carry — THE documented idiom for 64-bit.
    `import { plugin } from "@s2script/sdk/plugin";
     export default plugin((ctx) => {
       const h = ctx.publish("@demo/prod", { kills: () => 3 });
       ctx.clients.onRunCmd((view) => {
         h.emit("buttons", { mask: String(view.buttons) });
       });
     });`,
    // Plain-number payloads through an InterfaceHandle.
    `import { plugin } from "@s2script/sdk/plugin";
     interface Api { setScore(v: { score: number }): void; }
     export default plugin((ctx) => {
       const api = ctx.use<Api>("@demo/api");
       api.setScore({ score: 12 });
     });`,
  ],
  invalid: [
    {
      // (a) emit payload carrying a bigint property (the usercmd buttons trap).
      code: `import { plugin } from "@s2script/sdk/plugin";
             export default plugin((ctx) => {
               const h = ctx.publish("@demo/prod", { kills: () => 3 });
               ctx.clients.onRunCmd((view) => {
                 h.emit("buttons", { mask: view.buttons });
               });
             });`,
      errors: [{ messageId: "bigintPayload" }],
    },
    {
      // (b) InterfaceHandle method arg with a bigint literal.
      code: `import { plugin } from "@s2script/sdk/plugin";
             interface Api { setMask(v: unknown): void; }
             export default plugin((ctx) => {
               const api = ctx.use<Api>("@demo/api");
               api.setMask({ mask: 1n });
             });`,
      errors: [{ messageId: "bigintPayload" }],
    },
    {
      // (c) producer impl method RETURNING bigint — drops the consumer's whole call result.
      code: `import { plugin } from "@s2script/sdk/plugin";
             export default plugin((ctx) => {
               ctx.publish("@demo/prod", { mask: () => 1n });
             });`,
      errors: [{ messageId: "bigintPayload" }],
    },
  ],
});
