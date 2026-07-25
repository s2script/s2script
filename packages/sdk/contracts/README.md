# Standard interface contracts

These are **type-only contracts for inter-plugin interfaces** — not SDK capability modules.

The framework deliberately ships **no implementation** for anything in here. Each contract describes
a shape that a *community plugin* implements and publishes, so that consumers can depend on one
agreed interface instead of a different ad-hoc shape per plugin:

```ts
// producer
import type { EconService } from "@s2script/cs2/econ";
const impl: EconService = { /* … */ };
ctx.publish("econ", impl);          // manifest: s2script.publishes

// consumer
import type { EconService } from "@s2script/cs2/econ";
const econ = ctx.tryUse<EconService>("econ");   // null while unpublished
```

**Why contracts and not features.** Skins and workshop integration are Valve-backend and
game-content concerns, not engine touchpoints. Implementing them in the framework would drag the
core toward a specific game's economy and Steam's services, against *the core is engine-generic* and
*dependencies point one way: game → core*. Publishing the contract gets the ecosystem interop
without the framework taking on that surface.

**Rules that apply to every contract here** (they cross the plugin boundary, see
`docs/ARCHITECTURE.md`):

- Arguments and return values cross by **structured copy as JSON**. A `BigInt` throws and silently
  drops the whole payload, so every 64-bit value (Steam IDs, workshop IDs, item IDs) is typed as a
  **decimal string**, never `number` or `bigint`.
- `EntityRef` is the one live-ish exception: it crosses tagged and is revived serial-gated on the
  other side, so it stays `T | null`-safe.
- A hard dependency (`ctx.use`) throws `InterfaceUnavailable` if the producer is absent; an optional
  one (`ctx.tryUse`) returns `null`. For anything in here, prefer `tryUse` — these are third-party.
- Contracts are semver-governed. Adding an optional member is a minor; changing or removing one is a
  major.
