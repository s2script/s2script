# docs/progress — PROGRESS.md entries, one file per PR

`docs/PROGRESS.md` is the slice-by-slice history, newest first. PRs **do not edit it**: when every
open PR prepended to the same lines at the top of that file, whichever merged second always hit a
conflict. Instead, each PR adds its entry here as its own file, the way a changeset works.

## Adding an entry

Create `docs/progress/YYYY-MM-DD-<slug>.md` (lowercase slug, e.g. `2026-11-09-progress-fragments.md`)
holding the entry exactly as it should appear in PROGRESS.md — normally one paragraph led by the
bold title, date and status:

```markdown
**Mapchooser end-of-map vote (2026-11-08) — offline; live acceptance pending.** Prompted by …
```

No `#`/`##` headings (the entry goes under PROGRESS.md's own heading). `node scripts/progress.mjs
check` (part of `make ci-js`) validates names and contents.

## Folding

`node scripts/progress.mjs fold` moves every fragment to the top of PROGRESS.md's
`## Current state (full history)` list, newest date first, and deletes the fragment files. It runs
inside `npm run version-packages`, so the changesets "version packages" PR carries the fold; since
that PR is regenerated from `main` by CI, nobody else is editing PROGRESS.md when it changes. It is
safe to run by hand in its own PR too. Until an entry is folded, read it here.
