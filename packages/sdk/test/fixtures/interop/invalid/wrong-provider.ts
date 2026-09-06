import { publish } from "@s2script/sdk/plugin";
publish("@demo/wrong", { getCount: () => 1, setCount: (_count: number) => {} });
