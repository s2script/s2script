import { use } from "@s2script/sdk/plugin";
import { HookResult } from "@s2script/sdk/events";
const service = use("@demo/counter");
service.on("OnFormat", () => ({result: HookResult.Changed, extra: true}));
