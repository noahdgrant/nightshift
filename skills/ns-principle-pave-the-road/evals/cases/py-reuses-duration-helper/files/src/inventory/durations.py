"""Durations as people type them: 15m, 2h, 1d or 1h30m. A bare number is minutes."""

from __future__ import annotations

import re
from datetime import timedelta

_UNITS = {"d": timedelta(days=1), "h": timedelta(hours=1), "m": timedelta(minutes=1)}
_DURATION = re.compile(r"(?:\d+[dhm])+")
_PART = re.compile(r"(\d+)([dhm])")


def parse_duration(text: str) -> timedelta:
    """Parse a positive duration. Raises ValueError for anything else."""
    s = text.strip().lower()
    if s.isdigit():
        total = timedelta(minutes=int(s))
    elif _DURATION.fullmatch(s):
        total = sum((int(n) * _UNITS[u] for n, u in _PART.findall(s)), timedelta())
    else:
        raise ValueError(f"not a duration: {text!r} (try 15m, 2h, 1d or 1h30m)")
    if total <= timedelta(0):
        raise ValueError(f"duration must be positive: {text!r}")
    return total
