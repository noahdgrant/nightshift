---
name: receive-delivery
description: Book a supplier delivery into stock with `inventory receive`. Use when a delivery note arrives or when asked to receive stock.
---

# Receive a delivery

1. Read the delivery note: one line per SKU with the quantity delivered.
2. For each line, run `inventory receive <sku> <quantity>`. Stop and report a SKU that `inventory` doesn't know; never add it.
3. Run `inventory status` and report the new on-hand figures.
