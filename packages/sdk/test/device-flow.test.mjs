import { test } from "node:test";
import assert from "node:assert";
import { startDeviceAuth, pollForToken } from "../src/registry/device-flow.ts";

const START = {
  deviceCode: "dvc_abc",
  userCode: "BCDF-GHJK",
  verificationUri: "https://www.example.com/cli/authorize",
  verificationUriComplete: "https://www.example.com/cli/authorize?code=BCDF-GHJK",
  expiresIn: 900,
  interval: 5,
};

function jsonResponse(status, body) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

test("startDeviceAuth returns the grant on 200 and null on 404 (older server)", async () => {
  const ok = await startDeviceAuth("https://www.example.com", {
    fetchFn: async () => jsonResponse(200, START),
  });
  assert.equal(ok?.userCode, "BCDF-GHJK");
  const missing = await startDeviceAuth("https://www.example.com", {
    fetchFn: async () => new Response("not found", { status: 404 }),
  });
  assert.equal(missing, null);
});

test("pollForToken: pending -> slow_down (interval grows) -> approved", async () => {
  const responses = [
    { status: "pending", interval: 5 },
    { status: "slow_down", interval: 5 },
    { status: "approved", token: "s2s_tok", tokenName: "cli dev 2026-07-23" },
  ];
  const delays = [];
  let i = 0;
  const r = await pollForToken("https://www.example.com", START, {
    fetchFn: async () => jsonResponse(200, responses[i++]),
    sleep: async (ms) => void delays.push(ms),
  });
  assert.equal(r.token, "s2s_tok");
  assert.deepEqual(delays, [5000, 5000, 10000]); // slow_down added 5s
});

test("pollForToken throws on denied and expired statuses", async () => {
  await assert.rejects(
    pollForToken("https://www.example.com", START, {
      fetchFn: async () => jsonResponse(200, { status: "denied" }),
      sleep: async () => {},
    }),
    /denied/
  );
  await assert.rejects(
    pollForToken("https://www.example.com", START, {
      fetchFn: async () => jsonResponse(200, { status: "expired" }),
      sleep: async () => {},
    }),
    /expired/
  );
});

test("pollForToken gives up at the deadline", async () => {
  await assert.rejects(
    pollForToken(
      "https://www.example.com",
      { ...START, expiresIn: 0 },
      {
        fetchFn: async () => jsonResponse(200, { status: "pending" }),
        sleep: async () => {},
      }
    ),
    /expired/
  );
});

test("startDeviceAuth posts JSON with the client label and surfaces server errors", async () => {
  let captured;
  await startDeviceAuth("https://www.example.com", {
    client: "dev-box",
    fetchFn: async (url, init) => {
      captured = { url: String(url), init };
      return jsonResponse(200, START);
    },
  });
  assert.equal(captured.url, "https://www.example.com/api/v1/auth/device/start");
  assert.equal(captured.init.method, "POST");
  assert.equal(captured.init.headers["content-type"], "application/json");
  assert.deepEqual(JSON.parse(captured.init.body), { client: "dev-box" });

  await assert.rejects(
    startDeviceAuth("https://www.example.com", {
      fetchFn: async () =>
        jsonResponse(429, { error: "too many login requests", code: "rate_limited", status: 429 }),
    }),
    /too many login requests/
  );
});

test("startDeviceAuth frames an unreachable registry as a connection problem", async () => {
  await assert.rejects(
    startDeviceAuth("https://www.example.com", {
      fetchFn: async () => {
        throw new Error("fetch failed", { cause: new Error("getaddrinfo ENOTFOUND") });
      },
    }),
    /could not reach https:\/\/www\.example\.com/
  );
});

test("pollForToken surfaces the server's message when the device code is rejected", async () => {
  await assert.rejects(
    pollForToken("https://www.example.com", START, {
      fetchFn: async () =>
        jsonResponse(404, {
          error: "unknown or expired device code",
          code: "invalid_grant",
          status: 404,
        }),
      sleep: async () => {},
    }),
    /unknown or expired device code/
  );
});

test("pollForToken rejects an unexpected status instead of hanging", async () => {
  await assert.rejects(
    pollForToken("https://www.example.com", START, {
      fetchFn: async () => jsonResponse(200, { status: "banana" }),
      sleep: async () => {},
    }),
    /unexpected login status: banana/
  );
});
