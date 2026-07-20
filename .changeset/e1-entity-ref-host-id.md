---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

BREAKING (pre-1.0 minor): `EntityRef` is now `{index, id}` — `id` is a host-minted
liveness id replacing the raw engine `serial` on the public surface. Liveness is
decided by the host's books (listener-fed, cleared per map), never by entity memory;
stale refs — including across a changelevel — deterministically resolve to
`null`/`false`. The inter-plugin/handoff wire format is `{__s2ref: [index, id]}`;
pre-E1 `{__entref__}` blobs revive as inert data. The `EntityRef` constructor is no
longer part of the public typed surface — the framework mints every ref.
