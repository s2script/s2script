// Deliberately broken: package.json declares no "types", so buildLibrary's own fail-fast rejects
// it (a library must set "types" to a .d.ts — consumers vendor it to typecheck against). The
// SOURCE is fine — the point of this fixture is that a --filter naming an UNRELATED plugin must
// neither build nor fail on this library (fix round 1, finding #3).
export function broken(): string {
  return "broken";
}
