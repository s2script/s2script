// @ts-ignore — inter-plugin import resolved by the runtime (no .d.ts codegen yet)
const greeter = require("@demo/greeter");
export function onLoad(): void { console.log(greeter.greet(1)); }
