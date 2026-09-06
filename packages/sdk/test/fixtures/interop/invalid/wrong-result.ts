import { publish } from "@s2script/sdk/plugin";
publish("@demo/counter", { getCount: () => "wrong", setCount: (_count: number) => {} });
