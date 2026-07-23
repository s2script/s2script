// The single styling + interactivity layer over @clack/prompts. No command imports clack directly,
// so the gate and the look live in one place. clack is bundled into dist/cli.js (not a runtime dep).
import * as clack from "@clack/prompts";

export interface InteractivityFlags {
  ci?: boolean;
  yes?: boolean;
}

/** Interactive iff stdout is a TTY, we are not in CI, and neither --ci nor -y was passed. */
export function isInteractive(flags: InteractivityFlags = {}): boolean {
  return Boolean(process.stdout.isTTY) && !process.env.CI && !flags.ci && !flags.yes;
}

/** Ctrl-C from any prompt returns a cancel symbol → exit cleanly, never a stack trace. */
function guard<T>(value: T | symbol): T {
  if (clack.isCancel(value)) {
    clack.cancel("Cancelled.");
    process.exit(130);
  }
  return value as T;
}

export const intro = (msg: string): void => clack.intro(msg);
export const outro = (msg: string): void => clack.outro(msg);
export const note = (body: string, title?: string): void => clack.note(body, title);
export const log = {
  info: (m: string): void => clack.log.info(m),
  success: (m: string): void => clack.log.success(m),
  warn: (m: string): void => clack.log.warn(m),
  error: (m: string): void => clack.log.error(m),
  step: (m: string): void => clack.log.step(m),
  message: (m: string): void => clack.log.message(m),
};

export async function select<T extends string>(opts: {
  message: string;
  options: { value: T; label: string; hint?: string }[];
  initialValue?: T;
}): Promise<T> {
  return guard(await clack.select(opts)) as T;
}

export async function text(opts: {
  message: string;
  placeholder?: string;
  defaultValue?: string;
  initialValue?: string;
  validate?: (v: string) => string | undefined;
}): Promise<string> {
  return guard(await clack.text(opts));
}

export async function password(opts: {
  message: string;
  validate?: (v: string) => string | undefined;
}): Promise<string> {
  return guard(await clack.password(opts));
}

export async function confirm(opts: { message: string; initialValue?: boolean }): Promise<boolean> {
  return guard(await clack.confirm(opts));
}

/** Run an async op under a spinner when interactive; otherwise just run it. Returns fn's result. */
export async function task<T>(
  label: string,
  fn: () => Promise<T>,
  opts: { interactive: boolean; done?: (r: T) => string },
): Promise<T> {
  if (!opts.interactive) return fn();
  const s = clack.spinner();
  s.start(label);
  try {
    const r = await fn();
    s.stop(opts.done ? opts.done(r) : label);
    return r;
  } catch (e) {
    s.stop(`${label} — failed`, 1);
    throw e;
  }
}
