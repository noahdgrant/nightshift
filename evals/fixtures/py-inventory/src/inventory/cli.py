"""Command-line interface: `inventory <command> ...` against a JSON state file."""

from __future__ import annotations

import argparse
import sys
from datetime import datetime, timedelta
from pathlib import Path
from typing import Sequence, TextIO

from inventory import storage
from inventory.errors import InventoryError
from inventory.warehouse import Warehouse

DEFAULT_STATE = Path("inventory.json")


def _parse_now(value: str) -> datetime:
    try:
        return datetime.fromisoformat(value)
    except ValueError:
        raise argparse.ArgumentTypeError(f"not an ISO 8601 timestamp: {value!r}") from None


def _positive_int(value: str) -> int:
    try:
        n = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError(f"not an integer: {value!r}") from None
    if n <= 0:
        raise argparse.ArgumentTypeError(f"must be positive: {value!r}")
    return n


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="inventory", description=__doc__)
    parser.add_argument(
        "--state", type=Path, default=DEFAULT_STATE, help="state file (default: %(default)s)"
    )
    parser.add_argument(
        "--now",
        type=_parse_now,
        default=None,
        help="ISO 8601 timestamp to act at (default: the current time)",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("add-item", help="register a new item")
    p.add_argument("sku")
    p.add_argument("name")

    p = sub.add_parser("receive", help="add delivered stock")
    p.add_argument("sku")
    p.add_argument("quantity", type=_positive_int)

    p = sub.add_parser("reserve", help="hold stock for a while")
    p.add_argument("sku")
    p.add_argument("quantity", type=_positive_int)
    p.add_argument("--ttl", type=_positive_int, default=30, help="minutes (default: 30)")

    p = sub.add_parser("cancel", help="drop a reservation")
    p.add_argument("reservation_id")

    p = sub.add_parser("fulfil", help="ship a reservation")
    p.add_argument("reservation_id")

    p = sub.add_parser("status", help="show stock levels")
    p.add_argument("sku", nargs="?", default=None)

    return parser


def _status(warehouse: Warehouse, sku: str | None, now: datetime) -> list[str]:
    levels = warehouse.stock_levels(now)
    if sku is not None:
        warehouse.item(sku)
        levels = [lvl for lvl in levels if lvl.sku == sku]
    lines = [f"{'SKU':<12} {'ON HAND':>8} {'RESERVED':>8} {'AVAILABLE':>9}"]
    for lvl in levels:
        lines.append(f"{lvl.sku:<12} {lvl.on_hand:>8} {lvl.reserved:>8} {lvl.available:>9}")
    return lines


def run(args: argparse.Namespace, out: TextIO | None = None) -> None:
    out = out or sys.stdout
    now = args.now or datetime.now()
    warehouse = storage.load(args.state)
    changed = True

    if args.command == "add-item":
        item = warehouse.add_item(args.sku, args.name)
        print(f"added {item.sku} ({item.name})", file=out)
    elif args.command == "receive":
        total = warehouse.receive(args.sku, args.quantity)
        print(f"{args.sku}: {total} on hand", file=out)
    elif args.command == "reserve":
        r = warehouse.reserve(args.sku, args.quantity, now, timedelta(minutes=args.ttl))
        print(f"{r.id}: {r.quantity} x {r.sku} until {r.expires_at.isoformat()}", file=out)
    elif args.command == "cancel":
        r = warehouse.cancel(args.reservation_id)
        print(f"cancelled {r.id}", file=out)
    elif args.command == "fulfil":
        r = warehouse.fulfil(args.reservation_id, now)
        print(f"fulfilled {r.id}: {r.quantity} x {r.sku}", file=out)
    elif args.command == "status":
        changed = False
        for line in _status(warehouse, args.sku, now):
            print(line, file=out)

    if changed:
        storage.save(warehouse, args.state)


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        run(args)
    except (InventoryError, storage.StateFileError, ValueError) as e:
        print(f"inventory: error: {e}", file=sys.stderr)
        return 1
    return 0
