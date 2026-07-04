// Inter-plugin dep resolved by the runtime; typed `any` via the gate's ambient stub (no .d.ts codegen yet).
import { greet } from "@demo/greeter";
export function onLoad(): void { console.log(greet(1)); }
