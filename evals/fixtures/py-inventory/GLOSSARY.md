# Glossary

**Item**: a stock-keeping unit the warehouse tracks, identified by its SKU.
_Avoid_: product, article.

**On hand**: the physical quantity of an item in the warehouse. Only receiving and fulfilling change it.
_Avoid_: in stock, inventory count.

**Reservation**: a hold on a quantity of one item for a customer order, valid until its expiry. It does not change on-hand stock.
_Avoid_: booking, allocation, lock.

**Expiry**: the instant a reservation stops holding stock (`expires_at`). A reservation is expired at and after that instant, and an expired reservation holds nothing.
_Avoid_: timeout.

**Reserved**: the total quantity held by unexpired reservations of an item at a given time.

**Available**: on hand minus reserved, at a given time. The most a new reservation can take.
_Avoid_: free stock, spare.

**Fulfil**: ship an unexpired reservation, removing its quantity from on-hand stock.
_Avoid_: complete, consume.

**Stock level**: a point-in-time report row for one item: on hand, reserved and available.
