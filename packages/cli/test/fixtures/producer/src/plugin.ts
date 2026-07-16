import { publishInterface } from "@s2script/interfaces";
import type { Greeter } from "../api";

export function onLoad(): void {
  const impl: Greeter = { greet: (n: number) => `hi ${n}` };
  publishInterface("@demo/greeter", impl);
}
