// Hand-written ambient type for the @demo/greeter inter-plugin interface.
// Interface .d.ts codegen is deferred (Slice 5+); until then a consumer declares the
// producer's published shape by hand so `import greeter = require("@demo/greeter")`
// type-checks. This mirrors the producer's publishInterface(...) impl plus the
// runtime-injected on/off event API every consumed interface proxy carries.
declare module "@demo/greeter" {
  interface Greeter {
    greet(slot: number): string;
    on(event: "greeted", handler: (payload: { slot: number; tick: number }) => void): number;
    off(event: string, handler: (...args: any[]) => void): void;
  }
  const _default: Greeter;
  export = _default;
}
