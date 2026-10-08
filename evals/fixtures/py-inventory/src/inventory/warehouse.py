"""The warehouse: stock on hand per item, and the reservations held against it."""

from __future__ import annotations

from collections import defaultdict
from datetime import datetime, timedelta

from inventory.errors import (
    InsufficientStock,
    ReservationExpired,
    UnknownItem,
    UnknownReservation,
)
from inventory.models import DEFAULT_TTL, Item, Reservation, StockLevel


class Warehouse:
    """Tracks on-hand stock and time-limited reservations for a set of items.

    Every time-dependent call takes `now` explicitly, so callers (and tests)
    control the clock.
    """

    def __init__(self) -> None:
        self._items: dict[str, Item] = {}
        self._on_hand: dict[str, int] = {}
        self._reservations: dict[str, Reservation] = {}
        self._next_id = 1

    # Items and stock

    def add_item(self, sku: str, name: str) -> Item:
        if sku in self._items:
            raise ValueError(f"item {sku!r} already exists")
        item = Item(sku=sku, name=name)
        self._items[sku] = item
        self._on_hand[sku] = 0
        return item

    def item(self, sku: str) -> Item:
        try:
            return self._items[sku]
        except KeyError:
            raise UnknownItem(sku) from None

    def items(self) -> list[Item]:
        return sorted(self._items.values(), key=lambda i: i.sku)

    def receive(self, sku: str, quantity: int) -> int:
        """Add delivered stock. Returns the new on-hand quantity."""
        self.item(sku)
        if quantity <= 0:
            raise ValueError("quantity must be positive")
        self._on_hand[sku] += quantity
        return self._on_hand[sku]

    def on_hand(self, sku: str) -> int:
        self.item(sku)
        return self._on_hand[sku]

    # Reservations

    def reservations(self, sku: str | None = None) -> list[Reservation]:
        """Every reservation the warehouse holds, expired or not, oldest first."""
        found = self._reservations.values()
        if sku is not None:
            self.item(sku)
            found = [r for r in found if r.sku == sku]
        return sorted(found, key=lambda r: r.id)

    def reservation(self, reservation_id: str) -> Reservation:
        try:
            return self._reservations[reservation_id]
        except KeyError:
            raise UnknownReservation(reservation_id) from None

    def reserved(self, sku: str, now: datetime) -> int:
        """Quantity held by reservations that have not expired at `now`."""
        return sum(r.quantity for r in self._active(sku, now))

    def available(self, sku: str, now: datetime) -> int:
        """Quantity that can still be reserved at `now`."""
        return self.on_hand(sku) - self.reserved(sku, now)

    def reserve(
        self,
        sku: str,
        quantity: int,
        now: datetime,
        ttl: timedelta = DEFAULT_TTL,
    ) -> Reservation:
        """Hold `quantity` of `sku` until `now + ttl`."""
        if quantity <= 0:
            raise ValueError("quantity must be positive")
        if ttl <= timedelta(0):
            raise ValueError("ttl must be positive")
        available = self.available(sku, now)
        if quantity >= available:
            raise InsufficientStock(sku, quantity, available)
        reservation = Reservation(
            id=self._new_id(),
            sku=sku,
            quantity=quantity,
            created_at=now,
            expires_at=now + ttl,
        )
        self._reservations[reservation.id] = reservation
        return reservation

    def cancel(self, reservation_id: str) -> Reservation:
        """Drop a reservation, returning its quantity to available stock."""
        reservation = self.reservation(reservation_id)
        del self._reservations[reservation_id]
        return reservation

    def fulfil(self, reservation_id: str, now: datetime) -> Reservation:
        """Ship a reservation: remove its quantity from stock on hand."""
        reservation = self.reservation(reservation_id)
        if reservation.is_expired(now):
            raise ReservationExpired(reservation_id)
        self._on_hand[reservation.sku] -= reservation.quantity
        del self._reservations[reservation_id]
        return reservation

    # Reporting

    def stock_levels(self, now: datetime) -> list[StockLevel]:
        """On-hand and reserved figures for every item, as of `now`."""
        reserved_by_sku: dict[str, int] = defaultdict(int)
        for r in self._reservations.values():
            reserved_by_sku[r.sku] += r.quantity
        return [
            StockLevel(
                sku=item.sku,
                on_hand=self._on_hand[item.sku],
                reserved=reserved_by_sku[item.sku],
                as_of=now,
            )
            for item in self.items()
        ]

    # Persistence

    def to_dict(self) -> dict[str, object]:
        return {
            "next_id": self._next_id,
            "items": [
                {"sku": i.sku, "name": i.name, "on_hand": self._on_hand[i.sku]}
                for i in self.items()
            ],
            "reservations": [r.to_dict() for r in self.reservations()],
        }

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> Warehouse:
        warehouse = cls()
        for entry in data.get("items", []):  # type: ignore[union-attr]
            warehouse.add_item(entry["sku"], entry["name"])
            warehouse._on_hand[entry["sku"]] = int(entry["on_hand"])
        for entry in data.get("reservations", []):  # type: ignore[union-attr]
            reservation = Reservation.from_dict(entry)
            warehouse.item(reservation.sku)
            warehouse._reservations[reservation.id] = reservation
        warehouse._next_id = int(data.get("next_id", 1))  # type: ignore[arg-type]
        return warehouse

    # Internals

    def _active(self, sku: str, now: datetime) -> list[Reservation]:
        return [
            r
            for r in self._reservations.values()
            if r.sku == sku and not r.is_expired(now)
        ]

    def _new_id(self) -> str:
        reservation_id = f"R{self._next_id:04d}"
        self._next_id += 1
        return reservation_id
