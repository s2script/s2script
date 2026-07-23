/** Shared types and helpers every feature package in this plugin depends on. */

export interface Greeting {
  readonly text: string;
  readonly at: number;
}

/** A tiny in-memory store, the sort of thing a feature package shares. */
export class GreetingLog {
  readonly #entries: Greeting[] = [];

  add(text: string, at: number): void {
    this.#entries.push({ text, at });
  }

  get count(): number {
    return this.#entries.length;
  }

  latest(): Greeting | null {
    return this.#entries.length ? this.#entries[this.#entries.length - 1] : null;
  }
}
