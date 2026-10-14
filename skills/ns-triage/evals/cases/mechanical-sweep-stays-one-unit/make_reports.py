"""Write 30 report modules that each format dates by hand, and a golden test for each."""

from pathlib import Path

NAMES = [
    "shelf_audit", "cycle_count", "dock_schedule", "pick_backlog", "putaway_queue",
    "returns_hold", "damaged_goods", "supplier_lead", "carrier_pickup", "bin_capacity",
    "slow_movers", "fast_movers", "dead_stock", "safety_stock", "kit_assembly",
    "quality_hold", "recall_trace", "lot_expiry", "cold_chain", "hazmat_stock",
    "transfer_out", "transfer_in", "backorder_age", "customer_holds", "staff_picks",
    "zone_balance", "seasonal_stock", "promo_stock", "vendor_returns", "year_end",
]
FMT = "%d/%m/%Y"

MODULE = '''"""{title} report."""

from __future__ import annotations

from datetime import datetime, timedelta

from inventory.warehouse import Warehouse

TITLE = "{title}"
PERIOD = timedelta(days={days})


def render(warehouse: Warehouse, now: datetime) -> list[str]:
    """The report's lines at `now`."""
    start = now - PERIOD
    lines = [
        f"{{TITLE}} report",
        f"Generated: {{now.strftime('{fmt}')}}",
        f"Period: {{start.strftime('{fmt}')}} to {{now.strftime('{fmt}')}}",
        f"Next run: {{(now + PERIOD).strftime('{fmt}')}}",
        f"Stock counted: {{now.strftime('{fmt}')}}",
    ]
    for r in warehouse.reservations():
        lines.append(
            f"{{r.sku}} x{{r.quantity}} held {{r.created_at.strftime('{fmt}')}}"
            f" until {{r.expires_at.strftime('{fmt}')}}"
        )
    lines.append(f"End of {{TITLE.lower()}} report, {{now.strftime('{fmt}')}}")
    return lines
'''

TEST = '''from datetime import datetime, timedelta

from inventory.reports import {name}

NOW = datetime(2026, 3, 9, 9, 0, 0)


def test_{name}_report(warehouse):
    warehouse.reserve("BOLT-M6", 2, now=datetime(2026, 3, 2, 9, 0, 0), ttl=timedelta(days=7))
    assert {name}.render(warehouse, NOW) == [
        "{title} report",
        "Generated: 09/03/2026",
        "Period: {start} to 09/03/2026",
        "Next run: {next}",
        "Stock counted: 09/03/2026",
        "BOLT-M6 x2 held 02/03/2026 until 09/03/2026",
        "End of {lower} report, 09/03/2026",
    ]
'''


def main() -> None:
    from datetime import datetime, timedelta

    now = datetime(2026, 3, 9, 9, 0, 0)
    pkg = Path("src/inventory/reports")
    tests = Path("tests/reports")
    pkg.mkdir(parents=True)
    tests.mkdir(parents=True)
    (pkg / "__init__.py").write_text('"""One module per report; each has `render(warehouse, now)`."""\n')
    for i, name in enumerate(NAMES):
        title = name.replace("_", " ").capitalize()
        days = 7 * (1 + i % 4)
        (pkg / f"{name}.py").write_text(MODULE.format(title=title, days=days, fmt=FMT))
        (tests / f"test_{name}.py").write_text(
            TEST.format(
                name=name,
                title=title,
                lower=title.lower(),
                start=(now - timedelta(days=days)).strftime(FMT),
                next=(now + timedelta(days=days)).strftime(FMT),
            )
        )


main()
