// Live-gate fixture for Server.setCvar on a numeric cvar. Not shipped. Prefix [CVARPROBE].
//
// `cvar_probe [name] [values...]` (default: sv_infinite_ammo 1 2 0) sets each value with
// Server.setCvar, then asks the ENGINE for the cvar through the console (`Server.commandEx(name)`
// runs the engine's own cvar query and returns what it printed). The engine's text is the truth
// here: Server.getCvar reads the slot through the same layout setCvar writes, so it cannot catch a
// bad write. Each step logs PASS when the engine prints exactly the value just set and the default
// it printed before the run is unchanged (the old 16-byte write leaked into the default value).
import { HookResult } from "@s2script/sdk/events";
import { command } from "@s2script/sdk/commands";
import { Server } from "@s2script/sdk/server";

const L = (m: string) => console.log(`[CVARPROBE] ${m}`);

// The engine's console line for a cvar, e.g. `sv_infinite_ammo = 0 ...`. CS2 also prints the
// default (`def. "0"`) on some builds; read it when present.
function engineReport(name: string, out: string | null): { value: string | null; def: string | null; raw: string } {
  const raw = (out ?? "").trim();
  const esc = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const v = new RegExp(`${esc}\\s*=\\s*"?(-?[0-9.]+)`).exec(raw);
  const d = /def(?:ault)?\.?\s*[:=]?\s*"?(-?[0-9.]+)/i.exec(raw);
  return { value: v ? v[1] : null, def: d ? d[1] : null, raw: raw.split("\n").slice(0, 3).join(" | ") };
}

async function run(name: string, values: string[]): Promise<void> {
  const before = engineReport(name, await Server.commandEx(name));
  L(`before: engine value=${before.value} default=${before.def} getCvar=${Server.getCvar(name)} raw="${before.raw}"`);
  if (before.value === null) { L(`FAIL cannot read ${name} from the engine console (commandEx output above)`); return; }
  let fails = 0;
  for (const want of values) {
    const ok = Server.setCvar(name, want);
    const now = engineReport(name, await Server.commandEx(name));
    const valueOk = ok && now.value !== null && Number(now.value) === Number(want);
    const defOk = before.def === null || now.def === before.def;
    if (!valueOk || !defOk) fails += 1;
    L(`${valueOk && defOk ? "PASS" : "FAIL"} setCvar(${name}, ${want})=${ok} engine value=${now.value} `
      + `default=${now.def} (before ${before.def}) getCvar=${Server.getCvar(name)}`);
  }
  L(fails === 0 ? `DONE all ${values.length} steps PASS` : `DONE ${fails} of ${values.length} steps FAIL`);
}

export function OnPluginStart(): void {
  L("loaded");
  command.server("cvar_probe", (cmd) => {
    const name = cmd.argCount > 0 ? cmd.arg(0) : "sv_infinite_ammo";
    const values: string[] = [];
    for (let i = 1; i < cmd.argCount; i++) values.push(cmd.arg(i));
    void run(name, values.length > 0 ? values : ["1", "2", "0"]).catch((e) => L(`FAIL threw ${String(e)}`));
    return HookResult.Handled;
  });
}
