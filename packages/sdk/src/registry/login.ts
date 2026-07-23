import {
  defaultRegistryUrl,
  saveCredentials,
  type Credentials,
} from "./credentials.ts";
import * as ui from "../ui/ui.ts";

export async function loginInteractive(opts?: {
  token?: string;
  registryUrl?: string;
  ci?: boolean;
}): Promise<Credentials> {
  const registryUrl = (opts?.registryUrl || defaultRegistryUrl()).replace(/\/$/, "");
  let token = opts?.token || process.env.S2SCRIPT_TOKEN || process.env.S2SCRIPT_DEPLOY_TOKEN;

  if (!token) {
    if (!ui.isInteractive({ ci: opts?.ci })) {
      throw new Error(
        "no deploy token: set S2SCRIPT_TOKEN or run `s2s login` interactively"
      );
    }
    ui.intro("s2script login");
    ui.log.info(`Registry: ${registryUrl}`);
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

  if (!token || !token.startsWith("s2s_")) {
    throw new Error('token must start with "s2s_"');
  }

  const creds = { registryUrl, token };
  saveCredentials(creds);
  return creds;
}
