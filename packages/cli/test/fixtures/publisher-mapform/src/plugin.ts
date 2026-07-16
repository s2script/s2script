import { publishInterface } from "@s2script/interfaces";
import type { Contract } from "../api";

const impl: Contract = {
  ping(): boolean {
    return true;
  },
};

publishInterface("@community/contract", impl);
