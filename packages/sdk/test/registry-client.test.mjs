import { test } from "node:test";
import assert from "node:assert";
import { unzipSync, strFromU8 } from "fflate";
import { RegistryClient, RegistryError } from "../src/registry/client.ts";

function jsonResponse(status, body) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function clientWith(fetchImpl) {
  return new RegistryClient({
    baseUrl: "https://www.example.com",
    token: "s2s_x",
    fetch: fetchImpl,
  });
}

test("deploy posts an octet-stream zip with manifest + s2sp + types", async () => {
  let captured;
  const client = clientWith(async (url, init) => {
    captured = { url: String(url), init };
    return jsonResponse(200, { name: "rtv", version: "1.0.0", reviewState: "unreviewed" });
  });
  const r = await client.deploy({
    manifest: { id: "rtv", version: "1.0.0" },
    s2sp: Buffer.from([1, 2, 3]),
    types: Buffer.from([9]),
  });
  assert.equal(r.name, "rtv");
  assert.equal(captured.url, "https://www.example.com/api/v1/deploy");
  assert.equal(captured.init.headers["content-type"], "application/octet-stream");
  assert.equal(captured.init.headers.Authorization, "Bearer s2s_x");
  const entries = unzipSync(new Uint8Array(captured.init.body));
  assert.equal(strFromU8(entries["manifest.json"]), JSON.stringify({ id: "rtv", version: "1.0.0" }));
  assert.deepEqual([...entries["plugin.s2sp"]], [1, 2, 3]);
  assert.deepEqual([...entries["types.tgz"]], [9]);
});

test("deploy omits types.tgz when types is null", async () => {
  let captured;
  const client = clientWith(async (_url, init) => {
    captured = init;
    return jsonResponse(200, { name: "rtv", version: "1.0.0", reviewState: "unreviewed" });
  });
  await client.deploy({ manifest: { id: "rtv" }, s2sp: Buffer.from([1]), types: null });
  const entries = unzipSync(new Uint8Array(captured.body));
  assert.equal(entries["types.tgz"], undefined);
});

test("error extraction: {error} envelope", async () => {
  const client = clientWith(async () =>
    jsonResponse(403, { error: "package is owned by another user", code: "forbidden", status: 403 })
  );
  const err = await client.deploy({ manifest: {}, s2sp: Buffer.from([1]) }).catch((e) => e);
  assert.ok(err instanceof RegistryError);
  assert.equal(err.status, 403);
  assert.match(err.message, /owned by another user/);
});

test("error extraction: SvelteKit {message} shape, with 401 hint", async () => {
  const client = clientWith(async () =>
    jsonResponse(401, { message: "missing or invalid Bearer deploy token", status: 401 })
  );
  const err = await client.resolve("rtv").catch((e) => e);
  assert.match(err.message, /missing or invalid Bearer deploy token/);
  assert.match(err.message, /s2s login/);
});

test("error extraction: plain-text CSRF 403 gets the upgrade hint", async () => {
  const client = clientWith(
    async () => new Response("Cross-site POST form submissions are forbidden", { status: 403 })
  );
  const err = await client.deploy({ manifest: {}, s2sp: Buffer.from([1]) }).catch((e) => e);
  assert.match(err.message, /Cross-site POST form submissions are forbidden/);
  assert.match(err.message, /npm i -g @s2script\/sdk/);
});

test("error extraction: HTML error page", async () => {
  const client = clientWith(
    async () => new Response("<html><body>bad gateway</body></html>", { status: 502 })
  );
  const err = await client.meta("rtv").catch((e) => e);
  assert.match(err.message, /HTML error page/);
  assert.match(err.message, /502/);
});

test("error extraction: empty body falls back to status", async () => {
  const client = clientWith(async () => new Response("", { status: 500 }));
  const err = await client.resolve("rtv").catch((e) => e);
  assert.match(err.message, /HTTP 500/);
});

test("network failure surfaces the root cause with status 0", async () => {
  const client = clientWith(async () => {
    throw new Error("fetch failed", { cause: new Error("connect ECONNREFUSED 1.2.3.4:443") });
  });
  const err = await client.resolve("rtv").catch((e) => e);
  assert.ok(err instanceof RegistryError);
  assert.equal(err.status, 0);
  assert.match(err.message, /network error/);
  assert.match(err.message, /ECONNREFUSED/);
});

test("2xx with a non-JSON body is a clear error", async () => {
  const client = clientWith(async () => new Response("not json", { status: 200 }));
  const err = await client.deploy({ manifest: {}, s2sp: Buffer.from([1]) }).catch((e) => e);
  assert.match(err.message, /invalid JSON/);
});

test("a timed-out request is a status-0 RegistryError naming the host and budget", async () => {
  const client = clientWith(async () => {
    const e = new Error("The operation was aborted due to timeout");
    e.name = "TimeoutError";
    throw e;
  });
  const err = await client.deploy({ manifest: {}, s2sp: Buffer.from([1]) }).catch((e) => e);
  assert.ok(err instanceof RegistryError);
  assert.equal(err.status, 0);
  assert.match(err.message, /timed out/);
  assert.match(err.message, /www\.example\.com/);
});

test("downloadTypes reports server errors instead of returning a bogus buffer", async () => {
  const client = clientWith(async () => jsonResponse(404, { message: "no types for rtv@1.0.0" }));
  const err = await client.downloadTypes("rtv", "1.0.0").catch((e) => e);
  assert.ok(err instanceof RegistryError);
  assert.equal(err.status, 404);
  assert.match(err.message, /no types for rtv@1\.0\.0/);
  assert.match(err.message, /types download/);
});

test("error extraction: a JSON body with no error/message still shows the payload", async () => {
  const client = clientWith(async () => jsonResponse(500, { unexpected: "shape" }));
  const err = await client.meta("rtv").catch((e) => e);
  assert.equal(err.status, 500);
  assert.match(err.message, /unexpected/);
  assert.deepEqual(err.body, { unexpected: "shape" });
});
