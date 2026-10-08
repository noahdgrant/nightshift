from datetime import timedelta

import pytest

from conftest import T0
from inventory import (
    InsufficientStock,
    ReservationExpired,
    UnknownItem,
    UnknownReservation,
    Warehouse,
)


def test_new_item_has_no_stock():
    w = Warehouse()
    w.add_item("WASHER-M6", "M6 washer")
    assert w.on_hand("WASHER-M6") == 0


def test_adding_an_item_twice_is_rejected(warehouse):
    with pytest.raises(ValueError):
        warehouse.add_item("BOLT-M6", "duplicate")


def test_receive_adds_to_stock_on_hand(warehouse):
    assert warehouse.receive("BOLT-M6", 5) == 15
    assert warehouse.on_hand("BOLT-M6") == 15


def test_receive_rejects_non_positive_quantity(warehouse):
    with pytest.raises(ValueError):
        warehouse.receive("BOLT-M6", 0)


def test_unknown_item_is_reported(warehouse):
    with pytest.raises(UnknownItem):
        warehouse.on_hand("SCREW-M3")


def test_reservation_reduces_available_stock(warehouse):
    warehouse.reserve("BOLT-M6", 4, now=T0)
    assert warehouse.available("BOLT-M6", T0) == 6
    assert warehouse.on_hand("BOLT-M6") == 10


def test_reservation_ids_are_sequential(warehouse):
    first = warehouse.reserve("BOLT-M6", 1, now=T0)
    second = warehouse.reserve("BOLT-M6", 1, now=T0)
    assert (first.id, second.id) == ("R0001", "R0002")


def test_reserving_more_than_available_is_rejected(warehouse):
    with pytest.raises(InsufficientStock) as exc:
        warehouse.reserve("BOLT-M6", 11, now=T0)
    assert exc.value.available == 10


def test_reserving_against_an_item_with_no_stock_is_rejected(warehouse):
    with pytest.raises(InsufficientStock):
        warehouse.reserve("NUT-M6", 1, now=T0)


def test_reservation_expires_after_its_ttl(warehouse):
    r = warehouse.reserve("BOLT-M6", 4, now=T0, ttl=timedelta(minutes=10))
    assert r.expires_at == T0 + timedelta(minutes=10)
    assert warehouse.available("BOLT-M6", T0 + timedelta(minutes=9)) == 6
    assert warehouse.available("BOLT-M6", T0 + timedelta(minutes=10)) == 10


def test_expired_reservation_no_longer_blocks_new_reservations(warehouse):
    warehouse.reserve("BOLT-M6", 8, now=T0, ttl=timedelta(minutes=5))
    later = T0 + timedelta(minutes=6)
    r = warehouse.reserve("BOLT-M6", 6, now=later)
    assert r.quantity == 6


def test_cancel_returns_stock(warehouse):
    r = warehouse.reserve("BOLT-M6", 4, now=T0)
    warehouse.cancel(r.id)
    assert warehouse.available("BOLT-M6", T0) == 10
    with pytest.raises(UnknownReservation):
        warehouse.cancel(r.id)


def test_fulfil_ships_reserved_stock(warehouse):
    r = warehouse.reserve("BOLT-M6", 4, now=T0)
    warehouse.fulfil(r.id, now=T0 + timedelta(minutes=1))
    assert warehouse.on_hand("BOLT-M6") == 6
    assert warehouse.reservations() == []


def test_expired_reservation_cannot_be_fulfilled(warehouse):
    r = warehouse.reserve("BOLT-M6", 4, now=T0, ttl=timedelta(minutes=10))
    with pytest.raises(ReservationExpired):
        warehouse.fulfil(r.id, now=T0 + timedelta(minutes=11))
    assert warehouse.on_hand("BOLT-M6") == 10


def test_stock_levels_report_every_item(warehouse):
    warehouse.reserve("BOLT-M6", 3, now=T0)
    levels = {lvl.sku: lvl for lvl in warehouse.stock_levels(T0)}
    assert (levels["BOLT-M6"].on_hand, levels["BOLT-M6"].reserved) == (10, 3)
    assert levels["BOLT-M6"].available == 7
    assert levels["NUT-M6"].available == 0
