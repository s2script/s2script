// http-demo — proves fetch works, is concurrent, and never blocks the tick. Fires N concurrent
// requests at a public API and logs how many resolved; a frame handler proves the tick advanced
// throughout (the fetches did not stall the game).
import { fetch } from "@s2script/sdk/http";
import { OnGameFrame } from "@s2script/sdk/frame";

let frames = 0;

export async function onLoad(): Promise<void> {
  OnGameFrame.subscribe(() => { frames++; });
  const startFrames = frames;
  const N = 10;
  console.log("[http-demo] firing " + N + " concurrent fetches (frames=" + startFrames + ")");
  const results = await Promise.all(
    Array.from({ length: N }, (_unused, i) =>
      fetch("https://httpbin.org/get?i=" + i, { timeoutMs: 15000 })
        .then((r) => r.status)
        .catch((e) => "ERR:" + String(e))
    )
  );
  const ok = results.filter((s) => s === 200).length;
  // a single-request detail: status + a body snippet
  let detail = "";
  try { const r = await fetch("https://httpbin.org/get", { timeoutMs: 15000 }); detail = r.status + " len=" + r.text().length; }
  catch (e) { detail = "ERR:" + String(e); }
  console.log("[http-demo] " + ok + "/" + N + " ok; tick advanced " + (frames - startFrames) + " frames during the fetches; single=" + detail);
}

export function onUnload(): void { console.log("[http-demo] onUnload"); }
