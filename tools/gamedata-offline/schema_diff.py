#!/usr/bin/env python3
"""Summarise what changed between two schema catalogs, as Markdown.

    python3 -I tools/gamedata-offline/schema_diff.py <old catalog.json> <new catalog.json> [--max N]

Field offsets are what plugins read, so a moved field is the line that matters; added and removed
classes and fields are listed after it. Exits 0 either way: a schema change regenerates, it never
blocks.
"""
import argparse, json, pathlib


def fields(c):
    return {f['name']: f for f in c.get('fields', [])}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('old')
    ap.add_argument('new')
    ap.add_argument('--max', type=int, default=60, help='rows shown per table')
    a = ap.parse_args()
    old = json.loads(pathlib.Path(a.old).read_text())
    new = json.loads(pathlib.Path(a.new).read_text())

    moved, retyped, added, removed = [], [], [], []
    for k in sorted(set(old) & set(new)):
        of, nf = fields(old[k]), fields(new[k])
        for n in sorted(set(of) & set(nf)):
            if of[n]['offset'] != nf[n]['offset']:
                moved.append((k, n, of[n]['offset'], nf[n]['offset']))
            elif of[n].get('type') != nf[n].get('type'):
                retyped.append((k, n, of[n].get('type'), nf[n].get('type')))
        added += [(k, n, nf[n]['offset']) for n in sorted(set(nf) - set(of))]
        removed += [(k, n, of[n]['offset']) for n in sorted(set(of) - set(nf))]
    cls_added = sorted(set(new) - set(old))
    cls_removed = sorted(set(old) - set(new))

    L = ['### Schema catalog\n']
    if not (moved or retyped or added or removed or cls_added or cls_removed):
        L.append('No schema change.')
        print('\n'.join(L))
        return
    L.append(f'- **{len(moved)} field offsets moved**, {len(retyped)} field types changed')
    L.append(f'- {len(added)} fields added, {len(removed)} fields removed')
    L.append(f'- {len(cls_added)} classes added, {len(cls_removed)} classes removed\n')

    def table(title, head, rows, fmt):
        if not rows:
            return
        L.append(f'**{title}**\n')
        L.append(head)
        for r in rows[:a.max]:
            L.append(fmt(r))
        if len(rows) > a.max:
            L.append(f'| … and {len(rows) - a.max} more | | | |')
        L.append('')

    table('Moved', '| Class | Field | Old | New |\n|---|---|---|---|', moved,
          lambda r: f'| {r[0]} | `{r[1]}` | {r[2]} | {r[3]} |')
    table('Type changed', '| Class | Field | Old | New |\n|---|---|---|---|', retyped,
          lambda r: f"| {r[0]} | `{r[1]}` | `{json.dumps(r[2])}` | `{json.dumps(r[3])}` |")
    table('Removed fields', '| Class | Field | Old offset | |\n|---|---|---|---|', removed,
          lambda r: f'| {r[0]} | `{r[1]}` | {r[2]} | |')
    table('Added fields', '| Class | Field | Offset | |\n|---|---|---|---|', added,
          lambda r: f'| {r[0]} | `{r[1]}` | {r[2]} | |')
    if cls_removed:
        L.append('**Removed classes:** ' + ', '.join(f'`{c}`' for c in cls_removed[:a.max]) + '\n')
    if cls_added:
        L.append('**Added classes:** ' + ', '.join(f'`{c}`' for c in cls_added[:a.max]) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    main()
