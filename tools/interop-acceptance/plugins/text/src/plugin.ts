import type { Report } from "../api";
import { publish } from "@s2script/sdk/plugin";
// Deliberately bypass author-time typing only for host rejection probes.
declare function __s2_iface_emit(name: string, event: string, payload: unknown): void;
export function OnPluginStart(): void {
  const service = publish("@interop/text", {
    probe(mode: string): Report {
      const payload = { text: "seed", mode };
      service.emit("OnSignal", { ...payload });
      const action = service.dispatch("OnRequest", payload);
      const transformed = service.dispatch("OnFormat", payload);
      return { action, result: transformed.result, original: JSON.stringify(payload), final: JSON.stringify(transformed.payload) };
    },
    malformed() {
      let rejected = 0;
      for (const payload of [{ text: null, mode: "bad" }, { text: "seed", mode: "bad", extra: true }, { text: "seed", mode: undefined }]) {
        try { __s2_iface_emit("@interop/text", "OnSignal", payload); } catch { rejected++; }
      }
      return rejected;
    },
    ping(depth: number) { service.emit("OnSignal", { text: "seed", mode: `recurse:${depth}` }); },
  });
}
