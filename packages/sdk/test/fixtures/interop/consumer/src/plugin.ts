import { use } from "@s2script/sdk/plugin";
const counter = use("@demo/counter");
counter.on("OnCountChanged", event => { const n: number = event.count; console.log(n); });
const count: number = counter.getCount();
console.log(count);
