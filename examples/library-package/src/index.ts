/**
 * A base64 codec, written by hand instead of calling `Buffer`/`atob`/`btoa` — because none of
 * those exist in the bare V8 isolate plugins actually run in. That absence is exactly why this
 * example exists: a library is ordinary TypeScript esbuild bundles into a consumer's `.s2sp`, so
 * anything it imports (a Node builtin, an npm package that shells out to one) fails at RUNTIME on
 * the server, not at build time, unless it happens to be one of the `@s2script/*` builtins the
 * host injects. See README.md for the longer version of this point.
 */

const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/** Encode UTF-16-free ASCII/Latin-1 bytes as base64. */
export function encode(input: string): string {
  let out = "";
  for (let i = 0; i < input.length; i += 3) {
    const a = input.charCodeAt(i);
    const b = i + 1 < input.length ? input.charCodeAt(i + 1) : NaN;
    const c = i + 2 < input.length ? input.charCodeAt(i + 2) : NaN;
    out += ALPHABET[a >> 2];
    out += ALPHABET[((a & 3) << 4) | (Number.isNaN(b) ? 0 : b >> 4)];
    out += Number.isNaN(b) ? "=" : ALPHABET[((b & 15) << 2) | (Number.isNaN(c) ? 0 : c >> 6)];
    out += Number.isNaN(c) ? "=" : ALPHABET[c & 63];
  }
  return out;
}

/** Decode base64 produced by `encode`. Throws on a character outside the alphabet. */
export function decode(input: string): string {
  const clean = input.replace(/=+$/, "");
  let bits = 0;
  let acc = 0;
  let out = "";
  for (const ch of clean) {
    const idx = ALPHABET.indexOf(ch);
    if (idx < 0) throw new Error(`invalid base64 character ${JSON.stringify(ch)}`);
    acc = (acc << 6) | idx;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out += String.fromCharCode((acc >> bits) & 0xff);
    }
  }
  return out;
}
