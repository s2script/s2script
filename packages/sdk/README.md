# @s2script/sdk

The **s2script SDK** — types + CLI for building [Source 2](https://developer.valvesoftware.com/wiki/Source_2) / CS2 plugins in TypeScript.

> **`0.0.1` is a placeholder.** This release exists only to claim the package name and bootstrap npm OIDC trusted publishing. The real content lands in **`0.1.0`**:
>
> - Per-capability type subpaths — `import { Entity } from "@s2script/sdk/entity"`, `@s2script/sdk/math`, `@s2script/sdk/timers`, `@s2script/sdk/events`, … (the framework's standard library, consolidated from the former `@s2script/*` packages).
> - The build CLI, exposed as the **`s2s`** command (`s2s build`; cold-start `npx @s2script/sdk build`).

Game-specific schema/types ship separately in `@s2script/cs2` (and future `@s2script/<game>` packages).
