/**
 * `semver` ships no types of its own and `@types/semver` is not installed. Rather than add a
 * second dependency (and a second thing to keep in step) for the one function the SDK calls,
 * declare exactly that surface here. If the call site ever grows, this file grows with it —
 * which is the point: an implicit `any` on a version gate is precisely the silent degradation
 * this slice exists to remove.
 */
declare module "semver" {
  export function satisfies(
    version: string,
    range: string,
    options?: { includePrerelease?: boolean; loose?: boolean },
  ): boolean;
}
