"""Errors raised by the inventory package."""


class InventoryError(Exception):
    """Base class for every inventory error."""


class UnknownItem(InventoryError):
    def __init__(self, sku: str) -> None:
        super().__init__(f"unknown item {sku!r}")
        self.sku = sku


class UnknownReservation(InventoryError):
    def __init__(self, reservation_id: str) -> None:
        super().__init__(f"unknown reservation {reservation_id!r}")
        self.reservation_id = reservation_id


class InsufficientStock(InventoryError):
    def __init__(self, sku: str, requested: int, available: int) -> None:
        super().__init__(
            f"cannot reserve {requested} of {sku!r}: only {available} available"
        )
        self.sku = sku
        self.requested = requested
        self.available = available


class ReservationExpired(InventoryError):
    def __init__(self, reservation_id: str) -> None:
        super().__init__(f"reservation {reservation_id!r} has expired")
        self.reservation_id = reservation_id
