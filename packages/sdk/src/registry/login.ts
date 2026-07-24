import { hostname } from "node:os";
import {
  defaultRegistryUrl,
  normalizeRegistryUrl,
  saveCredentials,
  type Credentials,
} from "./credentials.ts";
import { pollForToken, startDeviceAuth } from "./device-flow.ts";
import { tryOpenBrowser } from "./open-browser.ts";
import * as ui from "../ui/ui.ts";

export async function loginInteractive(opts?: {
  token?: string;
  registryUrl?: string;
  ci?: boolean;
  noBrowser?: boolean;
}): Promise<Credentials> {
  const registryUrl = normalizeRegistryUrl(opts?.registryUrl || defaultRegistryUrl());
  let token = opts?.token || process.env.S2SCRIPT_TOKEN || process.env.S2SCRIPT_DEPLOY_TOKEN;

  if (!token) {
    if (!ui.isInteractive({ ci: opts?.ci })) {
      throw new Error("no deploy token: set S2SCRIPT_TOKEN or run `s2s login` interactively");
    }
    ui.intro("s2script login");
    ui.log.info(`Registry: ${registryUrl}`);
    const start = await startDeviceAuth(registryUrl, { client: hostname() });
    if (start) {
      ui.log.info(`Confirmation code: ${start.userCode}`);
      ui.log.info(`Approve at: ${start.verificationUriComplete}`);
      if (!opts?.noBrowser && tryOpenBrowser(start.verificationUriComplete)) {
        ui.log.info("Opened your browser — approve the request there.");
      } else {
        ui.log.info("Open that link in any browser (it can be on another machine).");
      }
      const approved = await ui.task(
        "Waiting for approval in the browser…",
        () => pollForToken(registryUrl, start),
        { interactive: true, done: (r) => `Approved (token "${r.tokenName ?? "cli"}")` }
      );
      token = approved.token;
    } else {
      // Older server without device login: original paste flow.
      ui.log.info(
        `Sign in (or create an account), then mint a deploy token at:\n  ${registryUrl}/account/tokens`
      );
      token = (
        await ui.password({
          message: "Paste your deploy token",
          validate: (v) =>
            (v ?? "").trim().startsWith("s2s_") ? undefined : 'token must start with "s2s_"',
        })
      ).trim();
    }
  }

  if (!token || !token.startsWith("s2s_")) {
    throw new Error('token must start with "s2s_"');
  }

  const creds = { registryUrl, token };
  saveCredentials(creds);
  return creds;
}
