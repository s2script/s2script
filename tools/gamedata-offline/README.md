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

With `--old --all-vtables` it also diffs the slot count of **every** RTTI vtable in every module
(old vs new), with the classes s2script names listed first. That catches a class s2script calls
through a compiled s2sdk header but has no pin for, which is how the Nov 11 INetworkMessages
break showed up. `--markdown <file>` writes the summary used in the update issue, and `--strict`
exits 1 when something needs a look: a newly failing signature, a pin mismatch, a moved index, or
a size change in a tracked class.

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

## Update day, automated (`update-day.sh`, `fetch-depot.sh`, `baseline.json`)

`.github/workflows/cs2-update.yml` polls Steam every 5 minutes. When depot 2347773's public
manifest differs from `baseline.json`, it fetches both builds and runs the checks, then files a
`cs2-update` issue with the report. It then starts Claude on the same files to open a draft fix
PR, if the `CLAUDE_CODE_OAUTH_TOKEN` or `ANTHROPIC_API_KEY` secret is set. The PR that fixes a
build bumps `baseline.json`, and that bump is what stops the watcher. The same steps by hand:

```bash
tools/gamedata-offline/fetch-depot.sh <new manifest> /tmp/new    # needs DepotDownloader
tools/gamedata-offline/fetch-depot.sh <baseline manifest> /tmp/old
tools/gamedata-offline/update-day.sh --new /tmp/new --old /tmp/old --out /tmp/report
# exit 0 = nothing to fix (regenerate the schema anyway), 1 = see /tmp/report/report.md
```

Run the workflow by hand (Actions → cs2-update → Run workflow) with `manifest` to check a
specific build, or with `force` to file the issue again.
