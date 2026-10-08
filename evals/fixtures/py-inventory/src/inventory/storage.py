"""Load and save a warehouse as a JSON state file."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path

from inventory.warehouse import Warehouse

FORMAT_VERSION = 1


class StateFileError(Exception):
    """The state file exists but can't be read as a warehouse."""


def load(path: Path) -> Warehouse:
    """Read the warehouse at `path`. A missing file is an empty warehouse."""
    if not path.exists():
        return Warehouse()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise StateFileError(f"{path}: not valid JSON ({e.msg} at line {e.lineno})") from e
    if not isinstance(data, dict):
        raise StateFileError(f"{path}: expected a JSON object")
    version = data.get("version")
    if version != FORMAT_VERSION:
        raise StateFileError(f"{path}: unsupported state version {version!r}")
    try:
        return Warehouse.from_dict(data["warehouse"])
    except (KeyError, TypeError, ValueError) as e:
        raise StateFileError(f"{path}: malformed warehouse data ({e})") from e


def save(warehouse: Warehouse, path: Path) -> None:
    """Write the warehouse to `path` atomically."""
    payload = {"version": FORMAT_VERSION, "warehouse": warehouse.to_dict()}
    text = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(text)
        os.replace(tmp, path)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
