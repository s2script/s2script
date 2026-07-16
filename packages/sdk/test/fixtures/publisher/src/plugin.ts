import { publishInterface } from "@s2script/sdk/interfaces";
import type { Publisher } from "../api";

const impl: Publisher = {
  ping(): boolean {
    return true;
  },
};

publishInterface("@demo/publisher", impl);
