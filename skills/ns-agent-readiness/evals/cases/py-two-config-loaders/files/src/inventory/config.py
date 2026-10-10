"""Read optional settings from inventory.toml in the working directory."""

from __future__ import annotations

import tomllib
from pathlib import Path

CONFIG_FILE = Path("inventory.toml")


def load_config(path: Path = CONFIG_FILE) -> dict:
    """Return the [inventory] table of `path`, or {} when the file is missing."""
    if not path.exists():
        return {}
    with path.open("rb") as f:
        return tomllib.load(f).get("inventory", {})
