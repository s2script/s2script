/** @demo/greeter — the contract this example publishes. The impl in src/plugin.ts is
 *  declared `: Greeter`, so `s2script build` fails if a method drifts from this file. */
export interface Greeter {
  /** Greet the player in `slot`. */
  greet(slot: number): string;
}
