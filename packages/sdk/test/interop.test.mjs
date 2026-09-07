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
function check(kind, mutate, match, diagnosticLine) {
  const dir = copy(kind);
  try {
    if (mutate) mutate(dir);
    const result = typecheckPlugin(dir, { packagesDir: packages });
    if (match) {
      assert.equal(result.ok, false);
      assert.ok(
        result.diagnostics.some(
          (d) =>
            d.file === join(dir, "src/plugin.ts") &&
            (diagnosticLine ? d.line === diagnosticLine : d.line > 0) &&
            match.test(d.message)
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
      pattern,
      1
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
      pattern,
      2
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

for (const expression of [
  'const { use: acquire } = Plugin; acquire<any>("@demo/counter");',
  'const acquire = Plugin["use"]; acquire<any>("@demo/counter");',
  'const { api: { use: acquire } } = { api: Plugin }; acquire<any>("@demo/counter");',
]) {
  test(`resolved SDK authority rejects indirect explicit generics: ${expression}`, async () => {
    const dir = copy("consumer");
    try {
      source(
        dir,
        `import * as Plugin from "@s2script/sdk/plugin"; ${expression}`
      );
      await assert.rejects(
        buildPlugin(dir, packages),
        /explicit generic arguments are forbidden/
      );
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
}
test("destructured second publication cannot bypass producer agreement", async () => {
  const dir = copy("producer");
  try {
    source(
      dir,
      `import * as Plugin from "@s2script/sdk/plugin";
Plugin.publish("@demo/counter", {getCount: () => 1, setCount: (n: number) => { console.log(n); }});
const {publish: provide} = Plugin;
provide<any>("@demo/counter", {getCount: () => "bad", setCount: (n: number) => { console.log(n); }});`
    );
    await assert.rejects(
      buildPlugin(dir, packages),
      /explicit generic|producer implementation/
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
for (const implementation of [
  "async (n: number) => { console.log(n); }",
  "(n: number) => n",
  "(n: 1) => { console.log(n); }",
]) {
  test(`producer signature rejects ${implementation}`, () =>
    check(
      "producer",
      (d) =>
        source(
          d,
          `import {publish} from "@s2script/sdk/plugin"; publish("@demo/counter", {getCount: () => 1, setCount: ${implementation}});`
        ),
      /producer.*signature|producer.*result|producer.*input/
    ));
}
test("authored prototype names survive metadata for fields, methods and forwards", async () => {
  const { extractContract } = await import("../src/interop.ts");
  const dir = copy("producer");
  try {
    const path = join(dir, "contract.d.ts");
    writeFileSync(
      path,
      `import type {Notification} from "@s2script/sdk/interfaces";
export interface Contract { methods: { __proto__(): number }; forwards: { __proto__: Notification<{__proto__: number}> } }`
    );
    const result = extractContract(path, packages);
    const m = result.metadata;
    assert.equal(Object.getPrototypeOf(m.methods), null);
    assert.equal(Object.getPrototypeOf(m.forwards), null);
    assert.equal(
      Object.getPrototypeOf(m.forwards.__proto__.payload.fields),
      null
    );
    assert.equal(Object.hasOwn(m.methods, "__proto__"), true);
    assert.equal(Object.hasOwn(m.forwards, "__proto__"), true);
    assert.equal(
      Object.hasOwn(m.forwards.__proto__.payload.fields, "__proto__"),
      true
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
test("RFC 8785 canonical vectors agree with the shared host fixture", async () => {
  const { extractContract, canonical } = await import("../src/interop.ts");
  const fixture = JSON.parse(
    readFileSync(join(fixtures, "canonical.json"), "utf8")
  );
  const contract = extractContract(join(fixtures, "canonical.d.ts"), packages);
  assert.deepEqual(JSON.parse(JSON.stringify(contract)), fixture.contract);
  assert.equal(canonical(contract.metadata), fixture.canonical);
  assert.ok(
    fixture.canonical.indexOf("😀") < fixture.canonical.indexOf("\uE000")
  );
});
test("canonical metadata rejects non-finite numbers and lone surrogates", async () => {
  const { canonical } = await import("../src/interop.ts");
  for (const value of [
    Infinity,
    -Infinity,
    NaN,
    "\uD800",
    "\uDFFF",
    { "\uD800": 1 },
  ])
    assert.throws(() => canonical(value), /canonical metadata requires/);
  assert.equal(canonical({ pair: "😀", zero: -0 }), '{"pair":"😀","zero":0}');
});
test("a void annotation cannot hide an async producer implementation", () =>
  check(
    "producer",
    (d) =>
      source(
        d,
        `import {publish} from "@s2script/sdk/plugin";
import type {Contract} from "../api";
const methods: Contract["methods"] = { getCount: () => 1, setCount: async (n: number) => { console.log(n); } };
publish("@demo/counter", methods);`
      ),
    /producer.*result/
  ));
test("producer signature accepts a default for an optional contract input", () =>
  check("producer", (d) => {
    const path = join(d, "api.d.ts");
    writeFileSync(
      path,
      readFileSync(path, "utf8").replace(
        "setCount(count: number)",
        "setCount(count?: number)"
      )
    );
    source(
      d,
      `import {publish} from "@s2script/sdk/plugin";
publish("@demo/counter", {getCount: () => 1, setCount: (count: number = 0) => { console.log(count); }});`
    );
  }));
for (const alias of [
  "const {setCount} = methods;",
  "const {setCount: alias} = methods; const setCount = alias;",
  "const setCount = methods.setCount;",
  "const {api: {setCount}} = {api: methods};",
]) {
  test(`annotated async producer cannot hide through ${alias}`, () =>
    check(
      "producer",
      (d) =>
        source(
          d,
          `import {publish} from "@s2script/sdk/plugin";
import type {Contract} from "../api";
const methods: Contract["methods"] = {getCount: () => 1, setCount: async (n: number) => { console.log(n); }};
${alias}
publish("@demo/counter", {getCount: () => 1, setCount});`
        ),
      /producer.*result/
    ));
}
test("destructured annotated synchronous producer remains accepted", () =>
  check("producer", (d) =>
    source(
      d,
      `import {publish} from "@s2script/sdk/plugin";
import type {Contract} from "../api";
const methods: Contract["methods"] = {getCount: () => 1, setCount: (n: number) => { console.log(n); }};
const {setCount} = methods;
publish("@demo/counter", {getCount: () => 1, setCount});`
    )
  ));

const decisionContract = readFileSync(join(fixtures, "decisions.d.ts"), "utf8");
function decisions(dir, kind) {
  writeFileSync(
    join(
      dir,
      kind === "producer"
        ? "api.d.ts"
        : ".s2script/types/@demo/counter/index.d.ts"
    ),
    decisionContract
  );
}
test("decision forwards infer synchronous handlers and mode-specific producer results", () => {
  check("producer", (d) => {
    decisions(d, "producer");
    source(
      d,
      `import {publish} from "@s2script/sdk/plugin";
import {HookResult, type HookResultValue} from "@s2script/sdk/events";
const service=publish("@demo/counter",{getCount:()=>1,setCount:(n:number)=>{console.log(n)}});
const decision:HookResultValue=service.dispatch("OnRequest",{identity:"a"});
const formatted:{result:HookResultValue;payload:{identity:string;text:string}}=service.dispatch("OnFormat",{identity:"a",text:"before"});
console.log(decision,formatted,HookResult.Continue);`
    );
  });
  check("consumer", (d) => {
    decisions(d, "consumer");
    source(
      d,
      `import {use} from "@s2script/sdk/plugin";
import {HookResult} from "@s2script/sdk/events";
const service=use("@demo/counter");
service.on("OnRequest",p=>{console.log(p.identity);return HookResult.Handled;});
service.on("OnFormat",p=>({result:HookResult.Changed,patch:{text:p.text+"!"}}));`
    );
  });
});
for (const [name, kind, statement, pattern] of [
  [
    "async transform",
    "consumer",
    'service.on("OnFormat",async()=>({result:HookResult.Changed,patch:{text:"x"}}));',
    /Promise/,
  ],
  [
    "async hook",
    "consumer",
    'service.on("OnRequest",async()=>HookResult.Stop);',
    /Promise/,
  ],
  [
    "invalid hook result",
    "consumer",
    'service.on("OnRequest",()=>4);',
    /4|HookResult/,
  ],
  [
    "readonly patch",
    "consumer",
    'service.on("OnFormat",()=>({result:HookResult.Changed,patch:{identity:"b"}}));',
    /identity|patch/,
  ],
  [
    "extra patch key",
    "consumer",
    'service.on("OnFormat",()=>({result:HookResult.Changed,patch:{text:"b",identity:"b"}}));',
    /never|identity|patch/,
  ],
  [
    "extra result key",
    "consumer",
    'service.on("OnFormat",()=>({result:HookResult.Changed,extra:1}));',
    /never|extra|assignable/,
  ],
  [
    "wrong patch value",
    "consumer",
    'service.on("OnFormat",()=>({result:HookResult.Changed,patch:{text:1}}));',
    /string/,
  ],
  [
    "patch without Changed",
    "consumer",
    'service.on("OnFormat",()=>({result:HookResult.Handled,patch:{text:"b"}}));',
    /patch|result|assignable/,
  ],
  [
    "emit hook",
    "producer",
    'service.emit("OnRequest",{identity:"a"});',
    /OnRequest/,
  ],
  [
    "union dispatch assumed transform",
    "producer",
    'const event:"OnRequest"|"OnFormat"=Math.random()>0.5?"OnRequest":"OnFormat";console.log(service.dispatch(event,{identity:"a",text:"b"}).payload);',
    /payload/,
  ],
  [
    "dispatch notification",
    "producer",
    'service.dispatch("OnCountChanged",{count:1});',
    /OnCountChanged/,
  ],
])
  test(`decision forwards reject ${name}`, () =>
    check(
      kind,
      (d) => {
        decisions(d, kind);
        source(
          d,
          `import {publish,use} from "@s2script/sdk/plugin";import {HookResult} from "@s2script/sdk/events";
const service=${
            kind === "producer"
              ? 'publish("@demo/counter",{getCount:()=>1,setCount:(n:number)=>{console.log(n)}})'
              : 'use("@demo/counter")'
          };${statement}`
        );
      },
      pattern
    ));
test("decision metadata includes kind and sorted writable keys in its digest", async () => {
  const { extractContract } = await import("../src/interop.ts");
  const dir = copy("producer");
  try {
    decisions(dir, "producer");
    const path = join(dir, "api.d.ts");
    const first = extractContract(path, packages);
    assert.deepEqual(
      JSON.parse(JSON.stringify(first)),
      JSON.parse(readFileSync(join(fixtures, "decisions.json"), "utf8"))
    );
    assert.equal(first.metadata.forwards.OnRequest.kind, "hook");
    assert.deepEqual(first.metadata.forwards.OnFormat.writable, ["text"]);
    writeFileSync(
      path,
      decisionContract.replace(/OnRequest:\s*Hook</, "OnRequest:Notification<")
    );
    const changed = extractContract(path, packages);
    assert.deepEqual(
      first.metadata.forwards.OnRequest.payload,
      changed.metadata.forwards.OnRequest.payload
    );
    assert.notEqual(first.sha256, changed.sha256);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

for (const [name, pattern] of [
  ["async-hook", /Promise/],
  ["illegal-hook-result", /4|HookResult/],
  ["illegal-patch-key", /never|patch/],
  ["illegal-response-key", /never|extra/],
]) {
  test(`separately compiled decision fixture: ${name}`, () =>
    check(
      "consumer",
      (d) => {
        decisions(d, "consumer");
        source(
          d,
          readFileSync(join(fixtures, "invalid", name + ".ts"), "utf8")
        );
      },
      pattern,
      4
    ));
}
for (const descriptor of [
  'Transform<{text:string},"missing">',
  "Transform<string,never>",
  "Transform<string[],never>",
  "Hook<any>",
  "{readonly __hookPayload: string}",
]) {
  test(`decision metadata rejects unsupported descriptor ${descriptor}`, async () => {
    const { extractContract } = await import("../src/interop.ts");
    const dir = copy("producer");
    try {
      const path = join(dir, "api.d.ts");
      writeFileSync(
        path,
        `import type {Hook,Transform} from "@s2script/sdk/interfaces"; export interface Contract {methods:{};forwards:{OnRequest:${descriptor}}}`
      );
      assert.throws(
        () => extractContract(path, packages),
        /constraint|finite object|unsupported|SDK/
      );
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
}

test("decision patch union rejects an illegal key present in only one member", () =>
  check(
    "consumer",
    (d) => {
      decisions(d, "consumer");
      source(
        d,
        readFileSync(
          join(fixtures, "invalid", "illegal-patch-union-key.ts"),
          "utf8"
        )
      );
    },
    /never/,
    5
  ));
test("decision patch union accepts distinct writable fields in every member", () =>
  check("consumer", (d) => {
    writeFileSync(
      join(d, ".s2script/types/@demo/counter/index.d.ts"),
      `import type {Transform} from "@s2script/sdk/interfaces";
export interface Contract {methods:{};forwards:{OnFormat:Transform<{identity:string;text:string;suffix?:string},"text"|"suffix">}}`
    );
    source(
      d,
      `import {use} from "@s2script/sdk/plugin";
import {HookResult} from "@s2script/sdk/events";
const service=use("@demo/counter");
declare const patch: {text:string} | {suffix:string};
service.on("OnFormat",()=>({result:HookResult.Changed,patch}));`
    );
  }));
