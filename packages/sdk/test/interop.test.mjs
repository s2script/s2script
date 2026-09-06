import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import {
  mkdtempSync,
  cpSync,
  readFileSync,
  writeFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { buildPlugin } from "../src/build.ts";
import { unzipSync, strFromU8 } from "fflate";
const fixtures = fileURLToPath(new URL("./fixtures/interop/", import.meta.url));
const packages = fileURLToPath(new URL("../../", import.meta.url));
function copy(kind) {
  const dir = mkdtempSync(join(tmpdir(), "s2-interop-"));
  cpSync(join(fixtures, kind), dir, { recursive: true });
  return dir;
}
function check(kind, mutate, match) {
  const dir = copy(kind);
  try {
    if (mutate) mutate(dir);
    const result = typecheckPlugin(dir, { packagesDir: packages });
    if (match) {
      assert.equal(result.ok, false);
      assert.ok(
        result.diagnostics.some(
          (d) =>
            d.file.endsWith("plugin.ts") && d.line > 0 && match.test(d.message)
        ),
        JSON.stringify(result.diagnostics)
      );
    } else assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
function source(dir, text) {
  writeFileSync(join(dir, "src/plugin.ts"), text);
}
test("protocol 2 infers verified producer and consumer contracts", () => {
  check("producer");
  check("consumer");
});
test("protocol 2 rejects misspelled notification at its call site", () =>
  check(
    "consumer",
    (d) =>
      source(
        d,
        'import { use } from "@s2script/sdk/plugin"; use("@demo/counter").on("OnCountChangd", () => {});'
      ),
    /OnCountChangd/
  ));
test("protocol 2 checks producer methods without an annotation", () =>
  check(
    "producer",
    (d) =>
      source(
        d,
        'import { publish } from "@s2script/sdk/plugin"; publish("@demo/counter", { getCount: () => "bad" });'
      ),
    /number|setCount/
  ));
test("protocol 2 emits canonical metadata and exact declaration hashes", async () => {
  const dir = copy("producer");
  try {
    const archive = unzipSync(readFileSync(await buildPlugin(dir, packages)));
    const manifest = JSON.parse(strFromU8(archive["manifest.json"]));
    assert.equal(manifest.interfaceProtocol, 2);
    assert.equal(
      manifest.publishes["@demo/counter"].contract.metadata.version,
      1
    );
    assert.equal(
      manifest.publishes["@demo/counter"].contract.metadata.forwards
        .OnCountChanged.kind,
      "notification"
    );
    assert.match(
      manifest.publishes["@demo/counter"].contract.sha256,
      /^[a-f0-9]{64}$/
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
for (const [name, statement, pattern] of [
  [
    "missing notification payload field",
    'publish("@demo/counter", impl).emit("OnCountChanged", {});',
    /count/,
  ],
  [
    "wrong notification payload type",
    'publish("@demo/counter", impl).emit("OnCountChanged", {count:"1"});',
    /number/,
  ],
  [
    "wrong provider name",
    'publish("@demo/wrong", impl);',
    /verified|authorized/,
  ],
  [
    "caller selected generic",
    'publish<{getCount():number}>("@demo/counter", impl);',
    /generic/,
  ],
])
  test(`protocol 2 rejects ${name}`, () =>
    check(
      "producer",
      (d) =>
        source(
          d,
          'import { publish } from "@s2script/sdk/plugin"; const impl={getCount:()=>1,setCount:(n:number)=>{console.log(n);}}; ' +
            statement
        ),
      pattern
    ));
test("protocol 2 missing verified copy never falls back to any", () =>
  check(
    "consumer",
    (d) => rmSync(join(d, ".s2script"), { recursive: true }),
    /verified contract/
  ));
test("protocol 2 forbids unhashed imported domain helpers", () =>
  check(
    "producer",
    (d) => {
      writeFileSync(
        join(d, "domain.d.ts"),
        "export interface Payload { count: number }"
      );
      writeFileSync(
        join(d, "api.d.ts"),
        'import type { Notification } from "@s2script/sdk/interfaces"; import type { Payload } from "./domain"; export interface Contract {methods:{getCount():number;setCount(n:number):void};forwards:{OnCountChanged:Notification<Payload>}}'
      );
    },
    /self-contained/
  ));
for (const type of [
  "any",
  "unknown",
  "bigint",
  "Promise<number>",
  "Record<string,number>",
  "{next: Payload}",
  "Date",
  "() => number",
])
  test(`protocol 2 rejects wire type ${type}`, () =>
    check(
      "producer",
      (d) => {
        writeFileSync(
          join(d, "api.d.ts"),
          `import type { Notification } from "@s2script/sdk/interfaces"; type Payload=${type}; export interface Contract {methods:{getCount():number;setCount(n:number):void};forwards:{OnCountChanged:Notification<Payload>}}`
        );
      },
      /unsupported|recursive/
    ));
test("protocol 2 rejects forged name associations", () =>
  check(
    "consumer",
    (d) => {
      writeFileSync(
        join(d, "src/forged.d.ts"),
        'import "@s2script/sdk/interfaces"; declare module "@s2script/sdk/interfaces" { interface InterfaceContracts { "@demo/forged": { methods: {}; forwards: {} } } }'
      );
      source(
        d,
        'import { use } from "@s2script/sdk/plugin"; use("@demo/forged");'
      );
    },
    /verified/
  ));
test("protocol 2 supports producer-as-import without implementation source", () =>
  check("consumer", (d) =>
    source(
      d,
      'import { getCount } from "@demo/counter"; const n:number=getCount(); console.log(n);'
    )
  ));
test("protocol 2 rejects arbitrary contract through an assigned use alias", () =>
  check(
    "consumer",
    (d) =>
      source(
        d,
        'import {use} from "@s2script/sdk/plugin"; const acquire=use; acquire<{wrong():string}>("@demo/counter");'
      ),
    /generic|constraint|never/
  ));
test("protocol 2 exported method declarations cannot disagree with Contract", () =>
  check(
    "producer",
    (d) =>
      writeFileSync(
        join(d, "api.d.ts"),
        readFileSync(join(d, "api.d.ts"), "utf8") +
          "\nexport declare function getCount(): string;"
      ),
    /export.*getCount|agree/
  ));
import { assembleDeployArchive } from "../src/registry/deploy.ts";
import { addPackage } from "../src/registry/add.ts";
test("protocol 2 publish/download roundtrip requests metadata and types only, then rejects stale bytes", async () => {
  const producer = copy("producer"),
    consumer = copy("consumer");
  try {
    const out = await buildPlugin(producer, packages),
      pkg = JSON.parse(readFileSync(join(producer, "package.json")));
    const packed = assembleDeployArchive(producer, pkg, out);
    rmSync(producer, { recursive: true, force: true });
    rmSync(join(consumer, ".s2script"), { recursive: true, force: true });
    const urls = [];
    await addPackage({
      pluginDir: consumer,
      spec: "@demo/counter",
      registryUrl: "https://example.invalid",
      fetch: async (url) => {
        urls.push(String(url));
        if (String(url).includes("/resolve"))
          return Response.json({
            name: "@demo/counter",
            version: "1.0.0",
            kind: "plugin",
            hasTypes: true,
            reviewState: "reviewed",
          });
        if (String(url).includes("/download/types"))
          return new Response(packed.types);
        throw Error("unexpected request " + url);
      },
    });
    assert.equal(urls.length, 2);
    assert.ok(
      urls.every((u) => u.includes("/resolve") || u.includes("/download/types"))
    );
    await buildPlugin(consumer, packages);
    const path = join(consumer, ".s2script/types/@demo/counter/index.d.ts");
    writeFileSync(
      path,
      readFileSync(path, "utf8") + "\n// stale byte change\n"
    );
    await assert.rejects(buildPlugin(consumer, packages), /hash|drift/);
  } finally {
    rmSync(producer, { recursive: true, force: true });
    rmSync(consumer, { recursive: true, force: true });
  }
});
for (const [file, kind, pattern] of [
  ["wrong-provider", "producer", /verified|never/],
  ["missing-method", "producer", /setCount/],
  ["wrong-result", "producer", /number/],
  ["undeclared-event", "consumer", /OnCountChangd/],
  ["missing-field", "producer", /count/],
  ["wrong-field", "producer", /number/],
])
  test(`separately compiled negative fixture: ${file}`, () =>
    check(
      kind,
      (d) =>
        source(
          d,
          readFileSync(join(fixtures, "invalid", file + ".ts"), "utf8")
        ),
      pattern
    ));
test("protocol 2 sibling contract wins over an incompatible copied contract", async () => {
  const root = mkdtempSync(join(tmpdir(), "s2-interop-workspace-"));
  try {
    writeFileSync(
      join(root, "package.json"),
      JSON.stringify({
        name: "interop-workspace",
        private: true,
        workspaces: ["producer", "consumer"],
        s2script: { workspace: { plugins: ["producer", "consumer"] } },
      })
    );
    cpSync(join(fixtures, "producer"), join(root, "producer"), {
      recursive: true,
    });
    cpSync(join(fixtures, "consumer"), join(root, "consumer"), {
      recursive: true,
    });
    const contract = join(root, "producer/api.d.ts");
    writeFileSync(
      contract,
      readFileSync(contract, "utf8").replaceAll(
        "count: number",
        "count: string"
      )
    );
    const result = typecheckPlugin(join(root, "consumer"), {
      packagesDir: packages,
    });
    assert.equal(result.ok, false);
    assert.ok(
      result.diagnostics.some(
        (d) =>
          d.file.endsWith("plugin.ts") &&
          d.line > 0 &&
          /string.*number/.test(d.message)
      ),
      JSON.stringify(result.diagnostics)
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
test("SDK canonical metadata matches the shared native acceptance fixture", async () => {
  const dir = copy("producer");
  try {
    const manifest = JSON.parse(
      strFromU8(
        unzipSync(readFileSync(await buildPlugin(dir, packages)))[
          "manifest.json"
        ]
      )
    );
    assert.deepEqual(
      manifest,
      JSON.parse(readFileSync(join(fixtures, "manifest.json")))
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
for (const member of [
  "methods: any; forwards: {}",
  "methods: {}; forwards: Record<string, Notification<number>>",
])
  test(`protocol 2 rejects unbounded contract member ${member}`, () =>
    check(
      "producer",
      (dir) =>
        writeFileSync(
          join(dir, "api.d.ts"),
          `import type {Notification} from "@s2script/sdk/interfaces";export interface Contract {${member}}`
        ),
      /finite|unsupported/
    ));
test("protocol 2 derives publications through namespace and assigned imported aliases", async () => {
  for (const text of [
    'import * as Plugin from "@s2script/sdk/plugin"; Plugin.publish("@demo/counter",{getCount:()=>1,setCount:(n:number)=>{console.log(n)}});',
    'import {publish as announce} from "@s2script/sdk/plugin"; const provide=announce; provide("@demo/counter",{getCount:()=>1,setCount:(n:number)=>{console.log(n)}});',
  ]) {
    const dir = copy("producer");
    try {
      source(dir, text);
      await buildPlugin(dir, packages);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
});
test("protocol 2 rejects undeclared methods supplied through an implementation variable", () =>
  check(
    "producer",
    (dir) =>
      source(
        dir,
        'import {publish} from "@s2script/sdk/plugin"; const impl={getCount:()=>1,setCount:(n:number)=>{console.log(n)},extra:()=>1};publish("@demo/counter",impl);'
      ),
    /undeclared method|agree/
  ));
test("protocol 2 rejects a forged association imported from outside the plugin directory", () => {
  const dir = copy("consumer"),
    outside = dir + "-forged.d.ts";
  try {
    writeFileSync(
      outside,
      'import type {Notification} from "@s2script/sdk/interfaces"; declare module "@s2script/sdk/interfaces" {interface InterfaceContracts {"@demo/counter": {methods:{getCount():string};forwards:{OnCountChanged:Notification<{count:string}>}}}}'
    );
    source(
      dir,
      `import ${JSON.stringify(
        outside
      )}; import {use} from "@s2script/sdk/plugin";use("@demo/counter").on("OnCountChanged",event=>{const s:string=event.count;console.log(s);});`
    );
    const result = typecheckPlugin(dir, { packagesDir: packages });
    assert.equal(result.ok, false);
    assert.ok(
      result.diagnostics.some((d) =>
        /associations are generated/.test(d.message)
      )
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
    rmSync(outside, { force: true });
  }
});
