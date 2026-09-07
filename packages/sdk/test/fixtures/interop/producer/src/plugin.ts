import { publish } from "@s2script/sdk/plugin";
const counter = publish("@demo/counter", { getCount: () => 1, setCount: (count: number) => { console.log(count); } });
counter.emit("OnCountChanged", { count: 1 });
