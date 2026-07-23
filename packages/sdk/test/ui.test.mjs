import { test } from "node:test";
import assert from "node:assert";
import { isInteractive } from "../src/ui/ui.ts";

function withEnv({ tty, ci }, fn) {
  const origTTY = Object.getOwnPropertyDescriptor(process.stdout, "isTTY");
  const origCI = process.env.CI;
  try {
    Object.defineProperty(process.stdout, "isTTY", { value: tty, configurable: true });
    if (ci === undefined) delete process.env.CI; else process.env.CI = ci;
    fn();
  } finally {
    if (origTTY) Object.defineProperty(process.stdout, "isTTY", origTTY);
    else delete process.stdout.isTTY;
    if (origCI === undefined) delete process.env.CI; else process.env.CI = origCI;
  }
}

test("isInteractive: true only on a TTY, outside CI, with no --ci/-y", () => {
  withEnv({ tty: true, ci: undefined }, () => {
    assert.equal(isInteractive(), true);
    assert.equal(isInteractive({}), true);
    assert.equal(isInteractive({ yes: true }), false, "-y forces non-interactive");
    assert.equal(isInteractive({ ci: true }), false, "--ci forces non-interactive");
  });
  withEnv({ tty: true, ci: "1" }, () => {
    assert.equal(isInteractive(), false, "CI env forces non-interactive");
  });
  withEnv({ tty: false, ci: undefined }, () => {
    assert.equal(isInteractive(), false, "no TTY -> non-interactive");
  });
});
