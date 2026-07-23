// argv parsing helpers shared by cli.ts and every command handler.

export function parseFlag(args: string[], name: string): string | undefined {
  const eq = args.find((a) => a.startsWith(`${name}=`));
  if (eq) return eq.slice(name.length + 1);
  const i = args.indexOf(name);
  if (i >= 0 && args[i + 1] && !args[i + 1]!.startsWith("-")) return args[i + 1];
  return undefined;
}

export function hasFlag(args: string[], name: string): boolean {
  return args.includes(name);
}

/** Positional args, skipping `-`/`--` flags and the value that follows a `--flag value` in
 *  `flagsWithValue` (a `--flag=value` form carries its own value and consumes no positional). */
export function positionals(args: string[], flagsWithValue: string[]): string[] {
  const out: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i]!;
    if (a.startsWith("--")) {
      const name = a.split("=")[0]!;
      if (flagsWithValue.includes(name) && !a.includes("=")) i++; // consume the value token
      continue;
    }
    if (a.startsWith("-")) continue;
    out.push(a);
  }
  return out;
}
