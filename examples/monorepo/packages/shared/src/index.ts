/**
 * A tiny workspace LIBRARY — not a plugin. It carries no `s2script.publishes`, so nothing can
 * `ctx.use()` it; instead `plugins/producer` and `plugins/consumer` each `import` it directly and
 * esbuild bundles it straight into their `.s2sp`, the same build-time factoring
 * examples/monorepo-plugin used to demonstrate for a single plugin (see this workspace's README).
 *
 * The difference worth noticing: BOTH plugins here import this exact file, and each build inlines
 * its OWN copy. Bundling is a build-time COPY, never a runtime share — so the `Tally` each plugin
 * constructs below counts independently, even though the source is one file on disk. If two
 * plugins instead need to observe the SAME live count, that is exactly what a published interface
 * is for (see plugins/producer/api.d.ts) — not a workspace library.
 */

/** A trivial formatter both plugins bundle independently. */
export function shout(name: string): string {
  return `HELLO, ${name.toUpperCase()}!`;
}

/** A per-bundle counter — see the module doc above: two plugins importing this get two
 *  independent Tallies, never one shared instance. */
export class Tally {
  #n = 0;

  bump(): number {
    return ++this.#n;
  }

  get count(): number {
    return this.#n;
  }
}
