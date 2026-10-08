"""Value types: items, reservations and stock levels."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta

DEFAULT_TTL = timedelta(minutes=30)


@dataclass(frozen=True)
class Item:
    """A stock-keeping unit the warehouse knows about."""

    sku: str
    name: str

    def __post_init__(self) -> None:
        if not self.sku or not self.sku.strip():
            raise ValueError("sku must not be empty")
        if self.sku != self.sku.strip():
            raise ValueError("sku must not have surrounding whitespace")


@dataclass(frozen=True)
class Reservation:
    """A hold on some quantity of one item until `expires_at`."""

    id: str
    sku: str
    quantity: int
    created_at: datetime
    expires_at: datetime

    def is_expired(self, now: datetime) -> bool:
        """A reservation stops holding stock at the instant it expires."""
        return now >= self.expires_at

    def to_dict(self) -> dict[str, object]:
        return {
            "id": self.id,
            "sku": self.sku,
            "quantity": self.quantity,
            "created_at": self.created_at.isoformat(),
            "expires_at": self.expires_at.isoformat(),
        }

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> Reservation:
        return cls(
            id=str(data["id"]),
            sku=str(data["sku"]),
            quantity=int(data["quantity"]),  # type: ignore[arg-type]
            created_at=datetime.fromisoformat(str(data["created_at"])),
            expires_at=datetime.fromisoformat(str(data["expires_at"])),
        )


@dataclass(frozen=True)
class StockLevel:
    """Point-in-time stock figures for one item."""

    sku: str
    on_hand: int
    reserved: int
    as_of: datetime

    @property
    def available(self) -> int:
        return self.on_hand - self.reserved
