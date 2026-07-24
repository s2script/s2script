import { loginInteractive } from "../registry/login.ts";
import { parseFlag, hasFlag } from "../cli/args.ts";
import { defaultRegistryUrl } from "../registry/credentials.ts";
import * as ui from "../ui/ui.ts";

export async function run(argv: string[]): Promise<void> {
  const interactive = ui.isInteractive({ ci: hasFlag(argv, "--ci") });
  try {
    // loginInteractive owns the token prompt (intro + masked password) when interactive.
    const creds = await loginInteractive({
      token: parseFlag(argv, "--token"),
      registryUrl: parseFlag(argv, "--registry") || defaultRegistryUrl(),
      ci: hasFlag(argv, "--ci"),
      noBrowser: hasFlag(argv, "--no-browser"),
    });
    const msg = `logged in → ${creds.registryUrl} (credentials saved)`;
    if (interactive) ui.outro(msg);
    else console.log(msg);
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}
