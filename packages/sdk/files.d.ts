/**
 * @s2script/files — sandboxed, off-thread file IO. Reads, writes (optionally gzip-compressed),
 * lists and removes run on the shared async runtime's worker threads; the game thread only copies
 * the payload and later receives the result on a frame drain. NO runtime code (injected at load).
 *
 * Every path is relative to this plugin's own root (`<data dir>/files/<plugin id>/`), `/`-separated,
 * with segments of `[A-Za-z0-9._-]` (no `.`/`..`, at most 8 deep, 256 bytes). Anything else rejects
 * with an `InvalidPath` error before any IO. One payload is capped by the async policy's
 * `input_item_bytes` (8 MiB by default, measured after decompression) — see docs/ASYNC_LIMITS.md.
 */

/** Options for {@link Files.read} / {@link Files.readText}. */
export interface FileReadOptions {
  /** Decompress a gzip file (written with `{ gzip: true }`) off-thread. Default false. */
  gzip?: boolean;
}

/** Options for {@link Files.write}. */
export interface FileWriteOptions {
  /** Compress with gzip off-thread before writing. Default false. Cannot combine with `append`. */
  gzip?: boolean;
  /**
   * Append instead of replace. Default false: a replacing write is atomic (temp file + rename), so
   * a crash or a concurrent read never observes a torn file. Appends are not atomic.
   */
  append?: boolean;
}

/**
 * Off-thread file IO in this plugin's sandbox. Promises reject with `AsyncQueueFull`,
 * `AsyncPayloadTooLarge`, `AsyncCancelled` (the plugin unloaded first) or a descriptive IO error.
 * @example
 * import { Files } from "@s2script/sdk/files";
 * // persist a binary blob compressed, then read it back — neither step blocks the game thread
 * await Files.write("replays/surf_utopia/wr.bin", frames, { gzip: true });
 * const back = await Files.read("replays/surf_utopia/wr.bin", { gzip: true });
 * if (back) console.log(`replay is ${back.length} bytes`);
 */
export declare const Files: {
  /** Read a file as bytes (a fresh `Uint8Array`). Resolves `null` when the file does not exist. */
  read(path: string, opts?: FileReadOptions): Promise<Uint8Array | null>;
  /** Read a file as UTF-8 text (invalid sequences become U+FFFD). Resolves `null` when absent. */
  readText(path: string, opts?: FileReadOptions): Promise<string | null>;
  /**
   * Write `data` (bytes, or a string as UTF-8), creating parent directories. The payload is copied
   * synchronously, so the caller may reuse its buffer as soon as this returns.
   */
  write(path: string, data: Uint8Array | string, opts?: FileWriteOptions): Promise<void>;
  /** Delete a file. Resolves `true` if it existed. */
  remove(path: string): Promise<boolean>;
  /**
   * The sorted entry names in a directory (default: the plugin root). Subdirectories end in `/`.
   * A missing directory resolves `[]`.
   */
  list(dir?: string): Promise<string[]>;
};
