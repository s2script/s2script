/** The @fixture/ws-greeter contract. A sibling consumer resolves THIS file in place — no copy. */
export interface Greeter {
  greet(name: string): string;
}
