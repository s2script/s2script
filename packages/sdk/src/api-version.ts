/**
 * The host apiVersion major THIS SDK types — the TS-side single source of truth.
 *
 * `s2s build` STAMPS the manifest `apiVersion` from this constant (north-star §5.2, locked
 * decision #6): the field is derived, never author-input, so "green build / refused load"
 * apiVersion drift is impossible to author. The loader's major gate (core/src/loader.rs
 * `HOST_API_VERSION_MAJOR` / `api_version_compatible`) stays as the runtime backstop.
 *
 * MUST equal core/src/loader.rs `HOST_API_VERSION_MAJOR` — test/api-version.test.mjs fails
 * the suite when they drift. Bump BOTH in the same commit.
 */
export const HOST_API_VERSION_MAJOR = 2;

/** Exactly what `s2s build` writes into manifest.apiVersion ("2.x": major-pinned, minor-open). */
export const STAMPED_API_VERSION = `${HOST_API_VERSION_MAJOR}.x`;
