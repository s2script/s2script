const S = JSON.stringify;

/**
 * Per-field offset slots for the generated accessors.
 *
 * Each distinct (class, field) pair gets one index into an `Int32Array` that starts at -1. The
 * emitted expression reads the slot and only calls the `__s2_schema_offset` native while it is
 * still negative, so a hit costs an array load instead of a native call that converts two strings
 * and hashes them. A miss stores -1 and is retried on the next access, which keeps the old
 * boot-window behaviour (the schema may not be ready yet). Hits never change within a process:
 * offsets come from the running binary, and the native's own cache holds them for good as well.
 */
export class OffsetSlots {
  private readonly index = new Map<string, number>();

  /** The JS expression yielding the offset of `cls::field` (or -1 while it does not resolve). */
  expr(cls: string, field: string): string {
    const key = `${cls}\0${field}`;
    let i = this.index.get(key);
    if (i === undefined) {
      i = this.index.size;
      this.index.set(key, i);
    }
    return `(O[${i}] >= 0 ? O[${i}] : (O[${i}] = off(${S(cls)},${S(field)})))`;
  }

  /** The declaration of the slot table; emit it once, before any accessor can run. */
  declaration(): string {
    return `  var O = new Int32Array(${Math.max(this.index.size, 1)}).fill(-1);`;
  }
}
