# gamedata-offline

Checks the shipped gamedata against CS2 binaries on disk, without a server. Run it on update day
against the new depot files, before a live boot.

```bash
pip install pyelftools capstone
python3 -I tools/gamedata-offline/check.py --bin <new dir> --old <previous dir>
```

`<dir>` holds `libserver.so`, `libengine2.so`, `libtier0.so`, `libnetworksystem.so`,
`libmatchmaking.so` and `libsoundsystem.so` (from depot 2347773).

It reports three things:

- **Signatures.** Each entry in `gamedata/core`, `gamedata/sdkhooks` and `games/cs2/gamedata` is
  resolved the way the shim resolves it at boot (`direct`, `lea-disp`, `ctor-body-xref`,
  `validated-call`, then the `prologue` / `string-xref` / `vtable-member` validators), against
  the file with its relocations applied.
- **Vtable pins.** Each `<Interface>_vtableSize` is compared with the slot count of the
  implementing class's vtable, found by RTTI like `s2vtable::GetVTableByName`.
- **Index alignment** (with `--old`). Each tracked class's vtable is aligned old-vs-new by a
  normalised fingerprint of each slot's first instructions, so an inserted virtual shows up as
  `MOVE Iface_Fn: Class[19] -> 20`.

It reads the files and never runs them. It does not check schema fields (those resolve by name at
runtime) or the few non-schema struct offsets (`CMoveData_*`, `GameEntitySystem`,
`CheckTransmitInfo_clientEntityIndex`, `CServerSideClient_m_nClientSlot`), which still need the
live gate.

## Schema catalog (`schema.py`)

Rebuilds `games/cs2/gamedata/schema-catalog.json` and `schema-enums.json` from the depot files,
so a CS2 update can be regenerated without booting a server and running `tools/schema-dump`.

```bash
python3 -I tools/gamedata-offline/schema.py --bin <dir>
node packages/sdk/dist/cli.js gen-schema && node packages/sdk/dist/cli.js gen-nav
```

`<dir>` needs the sixteen modules listed in `MODULES` (libserver, libanimationsystem,
libengine2, libmaterialsystem2, libmeshsystem, libnetworksystem, libparticles, libpulse_system,
librendersystemempty, libresourcesystem, libscenefilecache, libscenesystem, libschemasystem,
libsoundsystem, libvphysics2, libworldrenderer), all from depot 2347773.

It reads each module's static `SchemaClassInfoData_t` / `SchemaEnumInfoData_t` records and
replays every class's install callback symbolically to recover the field types the type scope
fills in at load. The output has the live dump's shape and ordering, so the diff is the real
change. About 40 fieldless classes (`*_API` script shims, Pulse function libraries) are built by
static constructors instead and have no record on disk; those are kept from the previous catalog
while their name is still in a module. It reports any field whose type it could not decode
(written as `{"kind":"unknown"}`); there should be none.
