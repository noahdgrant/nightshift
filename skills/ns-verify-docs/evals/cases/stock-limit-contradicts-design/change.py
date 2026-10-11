"""The unit's build: tighten STOCK-3 from 10% to 5% in the requirements doc and log it."""

import pathlib


def edit(path, pairs):
    p = pathlib.Path(path)
    s = p.read_text()
    for old, new in pairs:
        assert old in s, f"{path}: {old!r} not found"
        s = s.replace(old, new)
    p.write_text(s)


edit("docs/requirements/stock/v1.md", [
    ("take no more than 10% longer", "take no more than 5% longer"),
    ("10% decided by Priya, 2026-09-28", "5% decided by Priya, 2026-10-05, after the night batch ran late"),
    ("| Last updated | 2026-10-01 |", "| Last updated | 2026-10-05 |"),
])
edit("docs/requirements/stock/changelog.md", [
    ("|---|---|---|---|\n", "|---|---|---|---|\n| 2026-10-05 | 1 | STOCK-3 tightened from 10% to 5% | Priya |\n"),
])
