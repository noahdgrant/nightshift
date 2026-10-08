from datetime import datetime

import pytest

from inventory import Warehouse

T0 = datetime(2026, 3, 2, 9, 0, 0)


@pytest.fixture
def warehouse() -> Warehouse:
    w = Warehouse()
    w.add_item("BOLT-M6", "M6 hex bolt")
    w.add_item("NUT-M6", "M6 hex nut")
    w.receive("BOLT-M6", 10)
    return w
