import json
from datetime import timedelta

import pytest

from conftest import T0
from inventory import storage


def test_missing_state_file_is_an_empty_warehouse(tmp_path):
    assert storage.load(tmp_path / "none.json").items() == []


def test_round_trip_keeps_items_stock_and_reservations(tmp_path, warehouse):
    r = warehouse.reserve("BOLT-M6", 2, now=T0, ttl=timedelta(minutes=15))
    path = tmp_path / "state.json"
    storage.save(warehouse, path)

    loaded = storage.load(path)
    assert [i.sku for i in loaded.items()] == ["BOLT-M6", "NUT-M6"]
    assert loaded.on_hand("BOLT-M6") == 10
    assert loaded.reservation(r.id) == r
    assert loaded.reserve("BOLT-M6", 1, now=T0).id == "R0002"


def test_corrupt_state_file_is_reported(tmp_path):
    path = tmp_path / "state.json"
    path.write_text("{not json")
    with pytest.raises(storage.StateFileError):
        storage.load(path)


def test_unknown_state_version_is_reported(tmp_path):
    path = tmp_path / "state.json"
    path.write_text(json.dumps({"version": 99, "warehouse": {}}))
    with pytest.raises(storage.StateFileError, match="version"):
        storage.load(path)
