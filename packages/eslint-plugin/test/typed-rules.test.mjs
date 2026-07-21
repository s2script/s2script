import * as nodeTest from "node:test";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { RuleTester } from "@typescript-eslint/rule-tester";
import { noFloatingPromiseInFactory } from "../src/rules/no-floating-promise-in-factory.ts";
import { noAwaitInRawView } from "../src/rules/no-await-in-raw-view.ts";

// @typescript-eslint/rule-tester needs a test-framework hookup; wire it to node:test.
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

ruleTester.run("no-floating-promise-in-factory", noFloatingPromiseInFactory, {
  valid: [
    // awaited — the load window covers it.
    `import { plugin } from "@s2script/sdk/plugin";
     import { Database } from "@s2script/sdk/db";
     export default plugin(async (ctx) => {
       const db = await Database.open("prefs");
       ctx.events.on("round_start", () => { void db.query("x"); });
     });`,
    // explicit void — the author opted out, visibly.
    `import { plugin } from "@s2script/sdk/plugin";
     import { Database } from "@s2script/sdk/db";
     export default plugin((ctx) => {
       void Database.open("prefs");
       ctx.events.on("round_start", () => {});
     });`,
    // floating promise inside a HANDLER is not this rule's business.
    `import { plugin } from "@s2script/sdk/plugin";
     import { Database } from "@s2script/sdk/db";
     export default plugin((ctx) => {
       ctx.events.on("round_start", () => { Database.open("prefs"); });
     });`,
  ],
  invalid: [
    {
      code: `import { plugin } from "@s2script/sdk/plugin";
             import { Database } from "@s2script/sdk/db";
             export default plugin(async (ctx) => {
               Database.open("prefs");
               ctx.events.on("round_start", () => {});
             });`,
      errors: [{ messageId: "floating" }],
    },
    {
      // .then() chains are still thenables when discarded as a statement.
      code: `import { plugin } from "@s2script/sdk/plugin";
             import { Database } from "@s2script/sdk/db";
             export default plugin((ctx) => {
               Database.open("prefs").then((db) => db.query("x"));
               ctx.events.on("round_start", () => {});
             });`,
      errors: [{ messageId: "floating" }],
    },
  ],
});

ruleTester.run("no-await-in-raw-view", noAwaitInRawView, {
  valid: [
    // Synchronous handler use — the only sound pattern.
    `import { plugin } from "@s2script/sdk/plugin";
     export default plugin((ctx) => {
       ctx.clients.onRunCmd((view) => {
         if (view.forwardMove > 0) return 1;
       });
     });`,
    // Copy-then-async: plain values may cross awaits freely.
    `import { plugin } from "@s2script/sdk/plugin";
     export default plugin((ctx) => {
       ctx.clients.onRunCmd((view) => {
         const buttons = String(view.buttons);
         void (async () => { await Promise.resolve(); console.log(buttons); })();
       });
     });`,
  ],
  invalid: [
    {
      // The view itself dragged into async code — dead after the tick.
      code: `import { plugin } from "@s2script/sdk/plugin";
             export default plugin((ctx) => {
               ctx.clients.onRunCmd((view) => {
                 void (async () => { await Promise.resolve(); console.log(view.forwardMove); })();
               });
             });`,
      errors: [{ messageId: "rawViewInAsync" }],
    },
    {
      // Async helper PARAMETER typed as the raw view. LOCKED at 2 reports (calibrated against the
      // traversal): the UserCmdView-typed identifier appears twice inside the async fn — the param
      // binding `v` in its own annotation (the identifier itself types as UserCmdView) and the
      // `v.forwardMove` read. Both are genuine raw-view-in-async sites; do NOT loosen to shrink this.
      code: `import { plugin } from "@s2script/sdk/plugin";
             import type { UserCmdView } from "@s2script/sdk/usercmd";
             async function log(v: UserCmdView): Promise<void> {
               await Promise.resolve();
               console.log(v.forwardMove);
             }
             export default plugin((ctx) => {
               ctx.clients.onRunCmd((view) => { void log(view); });
             });`,
      errors: [{ messageId: "rawViewInAsync" }, { messageId: "rawViewInAsync" }],
    },
  ],
});
