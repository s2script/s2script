import { publish } from "@s2script/sdk/plugin";
publish("@demo/counter", { getCount: () => 1, setCount: (_count: number) => {} }).emit("OnCountChanged", { count: "1" });
