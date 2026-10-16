/**
 * `s2s deploy --access` / publishConfig.access: npm's access levels, and the registry
 * capability check that keeps a restricted package from going out public.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/access.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { assertAccessAllowed, assertRegistrySupportsAccess, resolveAccess } from "../src/registry/access.ts";
import { RegistryClient } from "../src/registry/client.ts";

const json = (status, body) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const clientWith = (fetchImpl) =>
  new RegistryClient({ baseUrl: "https://www.example.com", token: "s2s_x", fetch: fetchImpl });

test("resolveAccess: --access beats publishConfig.access, and neither leaves it to the registry", () => {
  const pkg = { name: "@edge/x", publishConfig: { access: "restricted" } };
  assert.equal(resolveAccess(pkg), "restricted");
  assert.equal(resolveAccess(pkg, "public"), "public");
  assert.equal(resolveAccess({ name: "@edge/x" }), undefined);
  assert.equal(resolveAccess({ name: "@edge/x", publishConfig: { registry: "x" } }), undefined);
});

test("resolveAccess refuses values npm does not have", () => {
  assert.throws(() => resolveAccess({}, "private"), /--access must be "public" or "restricted", not "private"/);
  assert.throws(
    () => resolveAccess({ name: "@edge/x", publishConfig: { access: true } }),
    /@edge\/x publishConfig\.access must be "public" or "restricted"/,
  );
});

test("assertAccessAllowed: only a scoped package can be restricted", () => {
  assert.throws(() => assertAccessAllowed("greeter", "restricted"), /greeter is unscoped/);
  assertAccessAllowed("@edge/greeter", "restricted");
  assertAccessAllowed("greeter", "public");
  assertAccessAllowed("greeter", undefined);
});

test("assertRegistrySupportsAccess passes when the registry advertises restricted", async () => {
  let asked;
  const client = clientWith(async (url) => {
    asked = String(url);
    return json(200, { access: ["public", "restricted"] });
  });
  await assertRegistrySupportsAccess(client, "restricted");
  assert.equal(asked, "https://www.example.com/api/v1/capabilities");
});

test("assertRegistrySupportsAccess refuses a registry that would publish it public", async () => {
  const old = clientWith(async () => json(404, { message: "Not Found" }));
  await assert.rejects(() => assertRegistrySupportsAccess(old, "restricted"), /does not support restricted packages yet, so nothing was published/);
  const unbound = clientWith(async () => json(200, { access: ["public"] }));
  await assert.rejects(() => assertRegistrySupportsAccess(unbound, "restricted"), /would make the package public/);
});

test("assertRegistrySupportsAccess asks nothing for public or unset access", async () => {
  const client = clientWith(async () => assert.fail("no request expected"));
  await assertRegistrySupportsAccess(client, "public");
  await assertRegistrySupportsAccess(client, undefined);
});

test("a 404 on a read hints at restricted packages, which answer like missing ones", async () => {
  const client = clientWith(async () => json(404, { message: "no version of @edge/secret satisfies *" }));
  await assert.rejects(() => client.resolve("@edge/secret"), /hint: if the package is restricted, log in/);
  await assert.rejects(() => client.plan("@edge/secret"), /hint: if the package is restricted, log in/);
});
