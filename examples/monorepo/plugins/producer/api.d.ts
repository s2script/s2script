/**
 * @monorepo-example/producer — the contract this workspace plugin publishes.
 *
 * Same shape of thing as examples/greeter-plugin's @demo/greeter: an impl in src/plugin.ts
 * declared `: Greeter`, so `s2s build` fails if a method drifts from this file, and the build
 * hashes these exact bytes into manifest.publishes / the consumer's manifest.compiledAgainst.
 *
 * What's different is WHO consumes it and how. plugins/consumer in THIS SAME repo depends on it —
 * a workspace sibling, not a registry package — so `s2s build` resolves this file in place through
 * the npm workspace symlink (node_modules/@monorepo-example/producer -> ../plugins/producer) and
 * no copy of it exists anywhere in plugins/consumer. See ../../README.md for the full contrast
 * against examples/greeter-consumer, which keeps exactly that kind of copy.
 *
 * `s2script.publishes` is set explicitly to `"self"` in this package's package.json rather than
 * left for `s2s build` to auto-derive from `ctx.publish("@monorepo-example/producer", …)` (which
 * examples/greeter-plugin relies on): a CONSUMER's build reads a sibling's package.json directly,
 * before that sibling has been built itself, so there is no scanned code to derive from yet. An
 * authored `publishes` is what makes the contract resolvable from the outside.
 */
export interface Greeter {
  /** Greet `name`. Delegates to @monorepo-example/shared's `shout`. */
  greet(name: string): string;
  /** How many times THIS plugin's own bundled copy of `Tally` has counted a greeting — see
   *  src/plugin.ts and ../../packages/shared/src/index.ts. */
  greetCount(): number;
}
