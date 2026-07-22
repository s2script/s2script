// @demo/crash-test — the deliberate-crash harness (crash-reporter spec §10). DEV-ONLY:
// every native kind is refused by core unless configs/crashreporter.json sets dev_test:true.
//
//   sm_crashtest segv   — real SIGSEGV in the shim → Breakpad minidump + .s2meta, server dies
//   sm_crashtest abort  — SIGABRT, same path
//   sm_crashtest panic  — Rust panic: recovered by catch_unwind, REPORTED by the panic hook
//   sm_crashtest js     — synchronous JS throw from this handler (kind=js incident)
//   sm_crashtest reject — unhandled promise rejection (kind=js incident, "unhandled-rejection")
import { plugin } from "@s2script/sdk/plugin";

declare function __s2_crash_test(kind: string): boolean;

export default plugin((ctx) => {
  ctx.commands.registerServer("crashtest", (cmd) => {
    const kind = cmd.args[0] ?? "";
    console.log(`[crash-test] sm_crashtest ${kind}`);
    if (kind === "js") {
      throw new Error("deliberate crash-harness js throw (sm_crashtest js)");
    }
    if (kind === "reject") {
      Promise.reject(new Error("deliberate crash-harness rejection (sm_crashtest reject)"));
      cmd.reply("crash-test: rejection queued (reported at end of frame)");
      return;
    }
    if (kind === "segv" || kind === "abort" || kind === "panic") {
      const armed = __s2_crash_test(kind);
      cmd.reply(`crash-test: ${kind} ${armed ? "raised" : "REFUSED (set dev_test:true in configs/crashreporter.json)"}`);
      return;
    }
    cmd.reply("usage: sm_crashtest <segv|abort|panic|js|reject>");
  });
  console.log("[crash-test] armed: sm_crashtest <segv|abort|panic|js|reject>");
});
