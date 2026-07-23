import * as ui from "../ui/ui.ts";
import { COMMANDS } from "./index.ts";

/** The no-arg interactive main menu: pick a command, then hand off to its handler
 *  (which prompts for whatever it needs). */
export async function run(): Promise<void> {
  ui.intro("s2script");
  const name = await ui.select<string>({
    message: "What would you like to do?",
    options: COMMANDS.map((c) => ({ value: c.name, label: c.name, hint: c.summary })),
  });
  const cmd = COMMANDS.find((c) => c.name === name)!;
  await cmd.run([]);
}
