import { test } from "node:test";
import assert from "node:assert";
import { defaultRegistryUrl, normalizeRegistryUrl } from "../src/registry/credentials.ts";

test("normalizeRegistryUrl rewrites the apex to www and strips trailing slash", () => {
  assert.equal(normalizeRegistryUrl("https://s2script.com"), "https://www.s2script.com");
  assert.equal(normalizeRegistryUrl("https://s2script.com/"), "https://www.s2script.com");
  assert.equal(normalizeRegistryUrl("https://www.s2script.com/"), "https://www.s2script.com");
  assert.equal(normalizeRegistryUrl("http://localhost:5173/"), "http://localhost:5173");
});

test("defaultRegistryUrl is www, and env overrides are normalized", () => {
  const prev = process.env.S2SCRIPT_REGISTRY_URL;
  try {
    delete process.env.S2SCRIPT_REGISTRY_URL;
    assert.equal(defaultRegistryUrl(), "https://www.s2script.com");
    process.env.S2SCRIPT_REGISTRY_URL = "https://s2script.com/";
    assert.equal(defaultRegistryUrl(), "https://www.s2script.com");
  } finally {
    if (prev === undefined) delete process.env.S2SCRIPT_REGISTRY_URL;
    else process.env.S2SCRIPT_REGISTRY_URL = prev;
  }
});
