import { publishInterface } from "@s2script/sdk/interfaces";
import type { OtherName } from "../api";

const impl: OtherName = {
  pong(): boolean {
    return true;
  },
};

publishInterface("@demo/other-name", impl);
