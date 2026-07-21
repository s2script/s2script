import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  ensureScopeNpmrc,
  npmCompatRegistryUrl,
  scopeFromPackageName,
} from "../src/registry/npmrc.ts";

describe("npmrc scope mapping", () => {
  it("extracts scope", () => {
    assert.equal(scopeFromPackageName("@acme/heal"), "acme");
    assert.equal(scopeFromPackageName("rtv"), null);
    assert.equal(scopeFromPackageName("@s2script/zones"), "s2script");
  });

  it("builds compat URL", () => {
    assert.equal(npmCompatRegistryUrl("https://s2script.com"), "https://s2script.com/npm/");
    assert.equal(npmCompatRegistryUrl("http://localhost:5173/"), "http://localhost:5173/npm/");
  });

  it("writes and upserts .npmrc", () => {
    const dir = mkdtempSync(join(tmpdir(), "s2-npmrc-"));
    try {
      const line = ensureScopeNpmrc(dir, "@acme/heal", "https://s2script.com");
      assert.equal(line, "@acme:registry=https://s2script.com/npm/");
      const body = readFileSync(join(dir, ".npmrc"), "utf8");
      assert.match(body, /@acme:registry=https:\/\/s2script\.com\/npm\//);

      // upsert with new registry
      ensureScopeNpmrc(dir, "@acme/other", "http://localhost:5173");
      const body2 = readFileSync(join(dir, ".npmrc"), "utf8");
      assert.match(body2, /@acme:registry=http:\/\/localhost:5173\/npm\//);
      assert.equal((body2.match(/@acme:registry=/g) || []).length, 1);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("skips @s2script and unscoped", () => {
    const dir = mkdtempSync(join(tmpdir(), "s2-npmrc-"));
    try {
      assert.equal(ensureScopeNpmrc(dir, "@s2script/zones", "https://s2script.com"), null);
      assert.equal(ensureScopeNpmrc(dir, "rtv", "https://s2script.com"), null);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("preserves other .npmrc lines", () => {
    const dir = mkdtempSync(join(tmpdir(), "s2-npmrc-"));
    try {
      writeFileSync(join(dir, ".npmrc"), "engine-strict=true\n");
      ensureScopeNpmrc(dir, "@acme/heal", "https://s2script.com");
      const body = readFileSync(join(dir, ".npmrc"), "utf8");
      assert.match(body, /engine-strict=true/);
      assert.match(body, /@acme:registry=/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
