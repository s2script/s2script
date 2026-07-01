import { publishInterface } from "@s2script/std";
export function onLoad(): void { publishInterface("@demo/greeter", "1.0.0", { greet: (n: number) => `hi ${n}` }); }
