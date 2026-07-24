import { plugin } from "@s2script/sdk/plugin";

// Dev/treadmill plugin: once a map is live and the SchemaSystem is populated, dump the whole
// class/field/type catalog to JSON via the __s2_schema_dump native, then stop. The committed
// games/cs2/gamedata/schema-catalog.json is the source of truth the 5B.3 codegen consumes; this
// plugin is how it's regenerated after each CS2 update (the treadmill).
//
// __s2_schema_dump is a dev/treadmill native (drives the shim's schema_enumerate SDK walk). It is
// NOT part of the typed @s2script/* surface, so we declare it ambiently here.
declare const __s2_schema_dump: (path: string) => boolean;

export default plugin((ctx) => {
  console.log("[schema-dump] onLoad — will dump once the schema is live");
  let done = false;
  let ticks = 0;
  ctx.server.onGameFrame(() => {
    if (done) return;
    if (ticks++ < 128) return;                 // let a map load + the schema populate
    // Path is relative to the server process CWD; the native writes it and returns true only when
    // the schema is warm (classes enumerated) AND the file was written. Retries until then.
    const ok = __s2_schema_dump("/tmp/schema-catalog.json");
    console.log("[schema-dump] dump " + (ok ? "OK -> /tmp/schema-catalog.json" : "not ready, retrying"));
    if (ok) done = true;
  });
});
