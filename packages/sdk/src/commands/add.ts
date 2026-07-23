import { addPackage } from "../registry/add.ts";
import { parseFlag, positionals } from "../cli/args.ts";
import * as ui from "../ui/ui.ts";

export async function run(argv: string[]): Promise<void> {
  const interactive = ui.isInteractive();
  const pos = positionals(argv, ["--dir", "--registry"]);
  let spec = pos[0];
  if (!spec && interactive) {
    spec = await ui.text({ message: "Package to add (name[@range])" });
  }
  if (!spec) {
    console.error("Usage: s2s add <pkg>[@range]");
    process.exit(1);
  }
  try {
    const result = await ui.task(
      `Adding ${spec}`,
      () =>
        addPackage({
          pluginDir: parseFlag(argv, "--dir") || ".",
          spec: spec!,
          registryUrl: parseFlag(argv, "--registry"),
        }),
      { interactive, done: (r) => `Added ${r.name}@${r.version} → ${r.typesDir}` },
    );
    if (interactive) {
      if (result.npmrcLine) {
        ui.log.info(`npmrc: ${result.npmrcLine}  (types-only; npm install ${result.name} also works)`);
      }
      if (result.reviewState === "unreviewed") {
        ui.log.warn("Not reviewed by s2script — types pulled; use at your own risk.");
      }
    } else {
      console.log(`added ${result.name}@${result.version} → ${result.typesDir}`);
      if (result.npmrcLine) {
        console.log(`npmrc: ${result.npmrcLine}  (types-only; npm install ${result.name} also works)`);
      }
      if (result.reviewState === "unreviewed") {
        console.log("note: Not reviewed by s2script — types pulled; use at your own risk.");
      }
    }
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}
