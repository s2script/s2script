import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { HOST_API_VERSION_MAJOR, STAMPED_API_VERSION } from "../src/api-version.ts";

test("SDK host-major equals core's HOST_API_VERSION_MAJOR (drift gate)", () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const loaderRs = readFileSync(
    join(here, "..", "..", "..", "core", "src", "loader.rs"),
    "utf8",
  );
  const m = loaderRs.match(/HOST_API_VERSION_MAJOR:\s*u32\s*=\s*(\d+)/);
  assert.ok(m, "HOST_API_VERSION_MAJOR not found in core/src/loader.rs");
  assert.equal(
    Number(m[1]),
    HOST_API_VERSION_MAJOR,
    "core and SDK disagree on the host apiVersion major — bump BOTH in one commit",
  );
});

test("stamped form carries the major in loader-parseable form", () => {
  // loader parse_api_major reads the leading integer: "2.x" -> 2.
  assert.match(STAMPED_API_VERSION, /^\d+\.x$/);
  assert.equal(parseInt(STAMPED_API_VERSION, 10), HOST_API_VERSION_MAJOR);
});
