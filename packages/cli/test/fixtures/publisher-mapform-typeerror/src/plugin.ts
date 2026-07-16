import { publishInterface } from "@s2script/interfaces";
import type { Contract } from "../api";

const impl: Contract = {
  ping(): boolean {
    return true;
  },
};

// Deliberate type error: this fixture exists to prove the RANGE publishes is rejected BEFORE the
// typecheck gate runs. A string is not assignable to number, so tsc would fail here — but the range
// rejection must fire first (fail fast), so this error must NOT surface.
const deliberateTypeError: number = "not a number";
void deliberateTypeError;

publishInterface("@community/contract", impl);
