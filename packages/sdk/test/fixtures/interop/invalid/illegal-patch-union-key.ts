import { use } from "@s2script/sdk/plugin";
import { HookResult } from "@s2script/sdk/events";
const service = use("@demo/counter");
declare const patch: { text: string } | { text: string; identity: string };
service.on("OnFormat", () => ({ result: HookResult.Changed, patch }));
