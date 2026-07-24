import { spawn } from "node:child_process";

/** Best-effort: opens url in the default browser. Never throws; false when it can't. */
export function tryOpenBrowser(url: string): boolean {
  // No DISPLAY (bare TTY, SSH session, container) means nothing can render a browser;
  // claiming otherwise would leave the user waiting on a window that never opens.
  if (process.platform === "linux" && !process.env.DISPLAY && !process.env.WAYLAND_DISPLAY) {
    return false;
  }
  let cmd: string;
  let args: string[];
  if (process.platform === "darwin") {
    cmd = "open";
    args = [url];
  } else if (process.platform === "win32") {
    cmd = "cmd";
    args = ["/c", "start", '""', url];
  } else {
    cmd = "xdg-open";
    args = [url];
  }
  try {
    const child = spawn(cmd, args, { stdio: "ignore", detached: true });
    child.on("error", () => {});
    child.unref();
    return true;
  } catch {
    return false;
  }
}
