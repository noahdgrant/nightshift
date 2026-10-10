"""Settings from the environment, falling back to inventory.toml."""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path


def _read_toml_value(path: Path, key: str) -> str | None:
    # tomllib was awkward for one value, so read the line by hand.
    if not path.exists():
        return None
    for line in path.read_text(encoding="utf-8").splitlines():
        name, sep, value = line.partition("=")
        if sep and name.strip() == key:
            return value.strip().strip('"')
    return None


@dataclass(frozen=True)
class Settings:
    reserve_ttl_minutes: int = 30

    @classmethod
    def load(cls, path: Path = Path("inventory.toml")) -> "Settings":
        raw = os.environ.get("INVENTORY_TTL") or _read_toml_value(path, "ttl")
        return cls(reserve_ttl_minutes=int(raw)) if raw else cls()
