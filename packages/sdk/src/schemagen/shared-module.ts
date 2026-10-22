import { createHash } from "node:crypto";

/**
 * Wrap a generated module so the host builds it once, in its shared framework realm, for every
 * plugin (core/src/v8host/realm.rs). `body` is the factory's statements and must `return` the
 * module object; `bind` then publishes that object (`M`) into the plugin's own globals.
 *
 * The factory runs in another realm, so it can use only `__s2_schema_offset` and the receiver's
 * own methods: no `__s2require`, no plugin globals, and no state that belongs to one plugin. The
 * key carries a hash of the body, so a regenerated module never reuses an older instance. Where
 * there is no framework realm (the SDK's own unit tests), the factory just runs in place.
 */
export function sharedModule(header: string, name: string, body: string[], bind: string[]): string {
  const hash = createHash("sha256").update(body.join("\n")).digest("hex").slice(0, 16);
  return [
    header,
    "(function () {",
    "  var shared = typeof __s2_shared_module === \"function\" ? __s2_shared_module : function (key, factory) { return factory(); };",
    `  var M = shared(${JSON.stringify(`${name}:${hash}`)}, function () {`,
    ...body.map((line) => (line === "" ? line : `  ${line}`)),
    "  });",
    ...bind.map((line) => `  ${line}`),
    "})();",
  ].join("\n");
}
