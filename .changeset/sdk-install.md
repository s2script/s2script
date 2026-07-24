---
'@s2script/sdk': minor
---

Add `s2s install` — download plugins and their dependencies onto a server.

Reads an `s2script-plugins.json` manifest (and/or names on the command line),
resolves each plugin's full non-`@s2script/*` dependency tree from the registry,
verifies every `.s2sp` by sha256, and writes them into the server's plugins
directory. Needs no credentials (registry reads are public), so it drops cleanly
into a Dockerfile:

    COPY s2script-plugins.json .
    RUN s2s install --dir /cs2/game/csgo/addons/s2script/plugins

`@s2script/*` base plugins are skipped (they ship with the runtime). Unreviewed
plugins install with a warning; `--reviewed-only` blocks them. `--dry-run` prints
the resolved plan without downloading.
