import { test } from "node:test";
import { RuleTester } from "eslint";
import tsParser from "@typescript-eslint/parser";
import { noCtxEscape } from "../src/rules/no-ctx-escape.ts";

const ruleTester = new RuleTester({
  languageOptions: { parser: tsParser, ecmaVersion: 2022, sourceType: "module" },
});

test("no-ctx-escape", () => {
  ruleTester.run("no-ctx-escape", noCtxEscape, {
    valid: [
      // Direct load-window use + a Scope driven later: the sanctioned patterns.
      `import { plugin } from "@s2script/sdk/plugin";
       export default plugin((ctx) => {
         ctx.events.on("player_death", () => {});
         const scope = ctx.createScope();
         ctx.commands.register("edit", () => { scope.clear(); });
       });`,
      // Not a plugin entry at all (no plugin() default export) — rule is inert.
      `const ctx = { events: { on() {} } };
       export function helper() { ctx.events.on("x", () => {}); }`,
      // Async factory: ctx used after await but still in the factory body — legal (load window
      // = the whole factory run).
      `import { plugin } from "@s2script/sdk/plugin";
       export default plugin(async (ctx) => {
         await Promise.resolve();
         ctx.events.on("round_start", () => {});
       });`,
    ],
    invalid: [
      {
        code: `import { plugin } from "@s2script/sdk/plugin";
               export default plugin((ctx) => {
                 ctx.commands.register("late", () => {
                   ctx.events.on("player_death", () => {});
                 });
               });`,
        errors: [{ messageId: "escaped" }],
      },
      {
        // Destructured members of the ctx param are just as load-window-only.
        code: `import { plugin } from "@s2script/sdk/plugin";
               export default plugin(({ events, commands }) => {
                 commands.register("late", () => {
                   events.on("player_death", () => {});
                 });
               });`,
        errors: [{ messageId: "escaped" }],
      },
      {
        // Captured in a returned hook — runs at unload, long after the seal.
        code: `import { plugin } from "@s2script/sdk/plugin";
               export default plugin((ctx) => {
                 return { onUnload() { ctx.events.on("x", () => {}); } };
               });`,
        errors: [{ messageId: "escaped" }],
      },
    ],
  });
});
