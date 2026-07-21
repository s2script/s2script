import { createInterface } from "node:readline/promises";
import { stdin as input, stdout as output } from "node:process";
import {
  defaultRegistryUrl,
  saveCredentials,
  type Credentials,
} from "./credentials.ts";

export async function loginInteractive(opts?: {
  token?: string;
  registryUrl?: string;
  ci?: boolean;
}): Promise<Credentials> {
  const registryUrl = (opts?.registryUrl || defaultRegistryUrl()).replace(/\/$/, "");
  let token = opts?.token || process.env.S2SCRIPT_TOKEN || process.env.S2SCRIPT_DEPLOY_TOKEN;

  if (!token) {
    if (opts?.ci || !input.isTTY) {
      throw new Error(
        "no deploy token: set S2SCRIPT_TOKEN or run `s2script login` interactively"
      );
    }
    const rl = createInterface({ input, output });
    try {
      console.log(`Registry: ${registryUrl}`);
      console.log("Create a deploy token at /account/tokens on the registry site, then paste it:");
      token = (await rl.question("token: ")).trim();
    } finally {
      rl.close();
    }
  }

  if (!token || !token.startsWith("s2s_")) {
    throw new Error('token must start with "s2s_"');
  }

  const creds = { registryUrl, token };
  saveCredentials(creds);
  return creds;
}
